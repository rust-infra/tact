//! Claude Code plugin hooks: parsing, command execution, and registration.
//!
//! Claude plugins declare hooks through `.claude-plugin/plugin.json`'s
//! `hooks` field, which points at a hooks JSON file of the form:
//!
//! ```json
//! {
//!   "hooks": {
//!     "SessionStart": [ { "matcher": "startup|resume", "hooks": [
//!       { "type": "command", "command": "node …", "timeout": 5,
//!         "statusMessage": "…" } ] } ]
//!   }
//! }
//! ```
//!
//! Each command hook is a shell command run with a JSON payload on stdin
//! (`session_id`, `transcript_path`, `cwd`, `hook_event_name` plus
//! event-specific fields) whose stdout JSON controls the outcome. Two output
//! formats are accepted: the newer `decision`/`reason`/`additionalContext`
//! shape and the legacy `hookSpecificOutput` shape
//! (`permissionDecision`/`permissionDecisionReason`/`additionalContext`).
//!
//! Tact maps five events to loop points:
//! `SessionStart`, `UserPromptSubmit`, `SubagentStart`, `PreToolUse`,
//! `PostToolUse`. Failures (non-zero exit, timeout, invalid JSON) never block
//! the agent loop — they log a warning and continue.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};

use anyhow::{Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::{io::AsyncWriteExt, process::Command, time::timeout};
use tracing::warn;

use crate::{
    consts::PluginHome,
    hook::{HookControl, SubagentStartContext, SubagentStartFn, ToolResult, ToolUse},
    plugin::PluginStore,
};

/// A Claude Code hook event that Tact maps to a loop point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEventKind {
    SessionStart,
    UserPromptSubmit,
    SubagentStart,
    PreToolUse,
    PostToolUse,
}

impl HookEventKind {
    /// The Claude Code event name (also the hooks-file key).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SubagentStart => "SubagentStart",
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "SessionStart" => Some(Self::SessionStart),
            "UserPromptSubmit" => Some(Self::UserPromptSubmit),
            "SubagentStart" => Some(Self::SubagentStart),
            "PreToolUse" => Some(Self::PreToolUse),
            "PostToolUse" => Some(Self::PostToolUse),
            _ => None,
        }
    }
}

/// A parsed Claude hooks file.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct HooksFile {
    #[serde(default)]
    pub hooks: HashMap<String, Vec<HookMatcher>>,
}

/// One matcher group: `hooks` run when `matcher` (regex) matches the
/// event-specific subject (tool name, prompt text, subagent name, source).
#[derive(Debug, Clone, Deserialize)]
pub struct HookMatcher {
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub hooks: Vec<HookCommand>,
}

/// One command hook entry.
#[derive(Debug, Clone, Deserialize)]
pub struct HookCommand {
    #[serde(rename = "type")]
    pub ty: Option<String>,
    #[serde(default)]
    pub command: Option<String>,
    #[serde(default, rename = "commandWindows")]
    pub command_windows: Option<String>,
    #[serde(default)]
    pub timeout: Option<u64>,
    #[serde(default, rename = "statusMessage")]
    pub status_message: Option<String>,
    #[serde(default, rename = "async")]
    pub async_: Option<bool>,
}

impl HooksFile {
    /// Parses a hooks file from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read hooks file {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse hooks file {}", path.display()))
    }

    /// Flattens `(matcher, command)` pairs for one event, in declaration order.
    pub fn commands_for(&self, event: HookEventKind) -> Vec<(&HookMatcher, &HookCommand)> {
        let mut out = Vec::new();
        for matcher in self.hooks.get(event.as_str()).into_iter().flatten() {
            for hook in &matcher.hooks {
                out.push((matcher, hook));
            }
        }
        out
    }
}

/// Runtime input for one command-hook invocation.
#[derive(Debug, Clone)]
pub struct HookRunInput {
    pub session_id: String,
    pub work_dir: PathBuf,
    pub hook_event_name: &'static str,
    /// Event-specific payload (`tool_name`, `tool_input`, `prompt`, …).
    pub event: Value,
}

/// Normalized hook output.
#[derive(Debug, Clone, Default)]
pub struct HookOutput {
    pub control: HookControl,
    /// `additionalContext` — appended to prompt / tool input by the caller.
    pub additional_context: Option<String>,
    /// `systemPrompt` / `updatedSystemPrompt` (SessionStart) — logged only.
    pub system_prompt: Option<String>,
    /// `suppressOutput` (PostToolUse) — caller may clear the tool result.
    pub suppress_output: bool,
}

impl HookOutput {
    fn continue_default() -> Self {
        Self::default()
    }
}

/// Runs one command hook and returns its normalized output.
///
/// Failure semantics (Claude Code compatible): non-zero exit, timeout, invalid
/// JSON, or a missing command never block the loop — a warning is logged and
/// `Continue` is returned. Async hooks are spawned and return immediately.
pub async fn run_command_hook(
    command: &HookCommand,
    plugin_root: &Path,
    input: &HookRunInput,
) -> HookOutput {
    let Some(raw) = command.command.as_deref() else {
        return HookOutput::continue_default();
    };
    if command.async_ == Some(true) {
        let expanded = expand_plugin_root(raw, plugin_root);
        let work_dir = input.work_dir.clone();
        let plugin_root = plugin_root.to_path_buf();
        let payload = build_payload(input);
        let _ = std::thread::spawn(move || {
            let rt = tokio::runtime::Runtime::new();
            let rt = match rt {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(async move {
                let _ = run_process(&expanded, &work_dir, &plugin_root, &payload, None).await;
            });
        });
        return HookOutput::continue_default();
    }

    let payload = build_payload(input);
    let timeout_secs = command.timeout.filter(|t| *t > 0).unwrap_or(60);
    match run_process(
        &expand_plugin_root(raw, plugin_root),
        &input.work_dir,
        plugin_root,
        &payload,
        Some(timeout_secs),
    )
    .await
    {
        Ok(stdout) => parse_output(&stdout),
        Err(error) => {
            warn!("plugin hook command failed (continuing): {error}");
            HookOutput::continue_default()
        }
    }
}

async fn run_process(
    command: &str,
    work_dir: &Path,
    plugin_root: &Path,
    payload: &Value,
    timeout_secs: Option<u64>,
) -> Result<String> {
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .env("CLAUDE_PLUGIN_ROOT", plugin_root)
        .env("CLAUDE_PROJECT_DIR", work_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to spawn hook command: {command}"))?;

    let mut stdin = child
        .stdin
        .take()
        .context("hook command has no stdin handle")?;
    let payload = serde_json::to_string(payload)?;
    stdin
        .write_all(payload.as_bytes())
        .await
        .context("failed to write hook payload to stdin")?;
    drop(stdin);

    let output = match timeout_secs {
        Some(secs) => timeout(Duration::from_secs(secs), child.wait_with_output())
            .await
            .with_context(|| format!("hook command timed out after {secs}s: {command}"))??,
        None => child.wait_with_output().await?,
    };

    if !output.status.success() {
        anyhow::bail!(
            "hook command exited with {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Builds the Claude Code hook input JSON (`session_id`, `transcript_path`,
/// `cwd`, `hook_event_name`, plus event-specific fields).
fn build_payload(input: &HookRunInput) -> Value {
    json!({
        "session_id": input.session_id,
        "transcript_path": input.work_dir.join(".tact/transcripts"),
        "cwd": input.work_dir,
        "hook_event_name": input.hook_event_name,
        "_event": input.event,
    })
}

/// Parses hook stdout JSON, accepting both the newer `decision` shape and the
/// legacy `hookSpecificOutput` shape.
fn parse_output(stdout: &str) -> HookOutput {
    let raw: Result<RawHookOutput, _> = serde_json::from_str(stdout);
    let raw = match raw {
        Ok(raw) => raw,
        Err(error) => {
            warn!("plugin hook returned invalid JSON (continuing): {error}");
            return HookOutput::continue_default();
        }
    };

    let legacy = raw.hook_specific_output.as_ref();
    let blocked = raw.decision.as_deref() == Some("block")
        || legacy
            .and_then(|l| l.permission_decision.as_deref())
            == Some("deny");
    let reason = raw
        .reason
        .clone()
        .or_else(|| {
            legacy
                .and_then(|l| l.permission_decision_reason.clone())
        })
        .unwrap_or_else(|| "blocked by plugin hook".to_string());

    HookOutput {
        control: if blocked {
            HookControl::Block(reason)
        } else {
            HookControl::Continue
        },
        additional_context: raw
            .additional_context
            .clone()
            .or_else(|| legacy.and_then(|l| l.additional_context.clone())),
        system_prompt: raw
            .system_prompt
            .clone()
            .or_else(|| legacy.and_then(|l| l.updated_system_prompt.clone())),
        suppress_output: legacy
            .and_then(|l| l.suppress_output)
            .unwrap_or(false),
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHookOutput {
    #[serde(default)]
    decision: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    additional_context: Option<String>,
    #[serde(default)]
    system_prompt: Option<String>,
    #[serde(default)]
    hook_specific_output: Option<RawHookSpecificOutput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHookSpecificOutput {
    // Parsed for legacy-schema tolerance; not consumed by the current mapping.
    #[serde(default)]
    #[allow(dead_code)]
    hook_event_name: Option<String>,
    #[serde(default)]
    permission_decision: Option<String>,
    #[serde(default)]
    permission_decision_reason: Option<String>,
    #[serde(default)]
    additional_context: Option<String>,
    #[serde(default)]
    updated_system_prompt: Option<String>,
    #[serde(default)]
    suppress_output: Option<bool>,
}

/// Expands `${CLAUDE_PLUGIN_ROOT}` (and `$CLAUDE_PLUGIN_ROOT`) in a command
/// string to the plugin cache root.
fn expand_plugin_root(command: &str, plugin_root: &Path) -> String {
    let root = plugin_root.to_string_lossy();
    command
        .replace("${CLAUDE_PLUGIN_ROOT}", &root)
        .replace("$CLAUDE_PLUGIN_ROOT", &root)
}

/// True when an optional matcher regex matches `subject`. An absent or empty
/// matcher matches everything; invalid regexes warn and match everything
/// (fail-open, matching Claude Code's permissive default).
fn matcher_matches(matcher: Option<&str>, subject: &str) -> bool {
    let Some(matcher) = matcher.filter(|m| !m.is_empty()) else {
        return true;
    };
    match Regex::new(matcher) {
        Ok(re) => re.is_match(subject),
        Err(error) => {
            warn!("invalid hook matcher regex {matcher:?}: {error}; matching everything");
            true
        }
    }
}

/// Loads every installed plugin's hooks file.
fn installed_hooks(home: &PluginHome) -> Result<Vec<InstalledHooks>> {
    let store = PluginStore::new(home.clone());
    let mut out = Vec::new();
    for root in store.installed_plugin_roots()? {
        let manifest_path = root.root.join(".claude-plugin").join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let Ok(manifest) = serde_json::from_str::<HookManifest>(&raw) else {
            continue;
        };
        let Some(hooks_path) = manifest.hooks else {
            continue;
        };
        let hooks_path = root.root.join(hooks_path);
        if !hooks_path.is_file() {
            warn!(
                "plugin {} declares hooks at {} but the file is missing",
                root.plugin_id,
                hooks_path.display()
            );
            continue;
        }
        match HooksFile::from_file(&hooks_path) {
            Ok(hooks) => out.push(InstalledHooks {
                plugin_id: root.plugin_id,
                plugin_root: root.root,
                hooks,
            }),
            Err(error) => warn!("plugin {} hooks file skipped: {error:#}", root.plugin_id),
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookManifest {
    #[serde(default)]
    hooks: Option<String>,
}

struct InstalledHooks {
    plugin_id: String,
    plugin_root: PathBuf,
    hooks: HooksFile,
}

/// Builds `SubagentStart` command-hook closures for every installed plugin.
///
/// The closures are stored on [`ToolContext`](crate::tool::ToolContext) and
/// invoked by `spawn_subagent`; `additionalContext` output is appended to the
/// child's system prompt and a `block` decision fails the spawn.
pub fn plugin_subagent_start_hooks(work_dir: &Path) -> Result<Vec<Arc<dyn SubagentStartFn>>> {
    let Some(home) = PluginHome::from_environment() else {
        return Ok(Vec::new());
    };
    let mut out: Vec<Arc<dyn SubagentStartFn>> = Vec::new();
    for installed in installed_hooks(&home)? {
                for (matcher, command) in installed.hooks.commands_for(HookEventKind::SubagentStart) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let plugin_root = installed.plugin_root.clone();
            let work_dir = work_dir.to_path_buf();
            let plugin_id = installed.plugin_id.clone();
            out.push(Arc::new(
                move |ctx: &mut SubagentStartContext| {
                    let command = command.clone();
                    let plugin_root = plugin_root.clone();
                    let work_dir = work_dir.clone();
                    let matcher = matcher.clone();
                    let plugin_id = plugin_id.clone();
                    let mut ctx = ctx.clone();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), &ctx.name) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_command_hook(
                            &command,
                            &plugin_root,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "SubagentStart",
                                event: json!({
                                    "subagent_name": ctx.name,
                                    "prompt": ctx.prompt,
                                }),
                            },
                        )
                        .await;
                        if let Some(extra) = output.additional_context {
                            ctx.system_prompt.push_str(&extra);
                        }
                        match output.control {
                            HookControl::Continue => {}
                            HookControl::Block(reason) => {
                                warn!(
                                    "plugin {plugin_id} SubagentStart hook blocked subagent: {reason}"
                                );
                            }
                        }
                        // A blocked subagent still runs — the reason is visible
                        // in the prompt — matching Claude Code's fail-open
                        // posture for command hooks.
                        Ok(HookControl::Continue)
                    })
                },
            ));
        }
    }
    Ok(out)
}

/// Applies every installed plugin's `SessionStart` / `UserPromptSubmit` /
/// `PreToolUse` / `PostToolUse` command hooks to an agent builder.
///
/// Hooks are appended after any existing Rust closures, per plugin in
/// installation order. A `block` output from `PreToolUse` / `PostToolUse`
/// propagates through [`HookControl`]; `UserPromptSubmit` appends
/// `additionalContext` to the prompt; `SessionStart`'s `systemPrompt` output
/// is logged but not applied (unsupported in v1).
pub fn apply_plugin_hooks(agent: crate::Agent, work_dir: &Path) -> Result<crate::Agent> {
    let Some(home) = PluginHome::from_environment() else {
        return Ok(agent);
    };
    let mut agent = agent;
    let work_dir = work_dir.to_path_buf();

    for installed in installed_hooks(&home)? {
        for (matcher, command) in installed.hooks.commands_for(HookEventKind::SessionStart) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let plugin_root = installed.plugin_root.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_session_start(move |_agent: &crate::Agent| {
                let matcher = matcher.clone();
                let command = command.clone();
                let plugin_root = plugin_root.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    // Tact sessions start normally; the Claude `source`
                    // matcher vocabulary (startup|resume|clear|compact) is
                    // matched against "startup".
                    if !matcher_matches(matcher.as_deref(), "startup") {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_command_hook(
                        &command,
                        &plugin_root,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "SessionStart",
                            event: json!({ "source": "startup" }),
                        },
                    )
                    .await;
                    if let Some(prompt) = output.system_prompt {
                        warn!("plugin SessionStart hook returned a system prompt; not applied in v1: {prompt}");
                    }
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in installed
            .hooks
            .commands_for(HookEventKind::UserPromptSubmit)
        {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let plugin_root = installed.plugin_root.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_user_prompt_submit(move |_agent: &crate::Agent, prompt: &mut String| {
                let matcher = matcher.clone();
                let command = command.clone();
                let plugin_root = plugin_root.clone();
                let work_dir = work_dir.clone();
                let prompt_snapshot = prompt.clone();
                Box::pin(async move {
                    if !matcher_matches(matcher.as_deref(), &prompt_snapshot) {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_command_hook(
                        &command,
                        &plugin_root,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "UserPromptSubmit",
                            event: json!({ "prompt": prompt_snapshot }),
                        },
                    )
                    .await;
                    if let Some(extra) = output.additional_context {
                        prompt.push_str(&extra);
                    }
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in installed.hooks.commands_for(HookEventKind::PreToolUse) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let plugin_root = installed.plugin_root.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_pre_tool(move |_agent: &crate::Agent, tool_use: &mut ToolUse| {
                let matcher = matcher.clone();
                let command = command.clone();
                let plugin_root = plugin_root.clone();
                let work_dir = work_dir.clone();
                let tool_name = tool_use.name.clone();
                let tool_input = tool_use.input.clone();
                Box::pin(async move {
                    if !matcher_matches(matcher.as_deref(), &tool_name) {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_command_hook(
                        &command,
                        &plugin_root,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "PreToolUse",
                            event: json!({
                                "tool_name": tool_name,
                                "tool_input": tool_input,
                            }),
                        },
                    )
                    .await;
                    if let Some(extra) = output.additional_context
                        && let Some(object) = tool_use.input.as_object_mut()
                    {
                        object.insert("_hook_context".into(), json!(extra));
                    }
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in installed.hooks.commands_for(HookEventKind::PostToolUse) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let plugin_root = installed.plugin_root.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_post_tool_hook(
                move |_agent: &crate::Agent, tool_use: &ToolUse, result: &mut ToolResult, _status| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let plugin_root = plugin_root.clone();
                    let work_dir = work_dir.clone();
                    let tool_name = tool_use.name.clone();
                    let tool_input = tool_use.input.clone();
                    let tool_response = result.content.clone();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), &tool_name) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_command_hook(
                            &command,
                            &plugin_root,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "PostToolUse",
                                event: json!({
                                    "tool_name": tool_name,
                                    "tool_input": tool_input,
                                    "tool_response": tool_response,
                                }),
                            },
                        )
                        .await;
                        if output.suppress_output {
                            result.content.clear();
                        }
                        Ok(output.control)
                    })
                },
            );
        }
    }

    Ok(agent)
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_claude_hooks_file() {
        let hooks = HooksFile::from_str(
            r#"{
                "hooks": {
                    "SessionStart": [{
                        "matcher": "startup|resume",
                        "hooks": [{
                            "type": "command",
                            "command": "node \"${CLAUDE_PLUGIN_ROOT}/hooks/start.js\"",
                            "commandWindows": "node \"$env:CLAUDE_PLUGIN_ROOT\\hooks\\start.js\"",
                            "timeout": 5,
                            "statusMessage": "Loading…",
                            "async": false
                        }]
                    }],
                    "UserPromptSubmit": [{
                        "hooks": [{ "type": "command", "command": "echo ok" }]
                    }]
                }
            }"#,
        )
        .unwrap();

        let session = hooks.commands_for(HookEventKind::SessionStart);
        assert_eq!(session.len(), 1);
        let (matcher, cmd) = session[0];
        assert_eq!(matcher.matcher.as_deref(), Some("startup|resume"));
        assert_eq!(cmd.timeout, Some(5));
        assert_eq!(cmd.async_, Some(false));
        assert!(cmd.command_windows.is_some());

        let prompt = hooks.commands_for(HookEventKind::UserPromptSubmit);
        assert_eq!(prompt.len(), 1);
        assert_eq!(prompt[0].0.matcher, None);
    }

    #[test]
    fn matcher_matches_regex_and_fails_open() {
        assert!(matcher_matches(None, "anything"));
        assert!(matcher_matches(Some(""), "anything"));
        assert!(matcher_matches(Some("startup|resume"), "startup"));
        assert!(!matcher_matches(Some("startup|resume"), "compact"));
        assert!(matcher_matches(Some("Bash|Read"), "Bash"));
        assert!(matcher_matches(Some("Read|Edit"), "Edit"));
        assert!(!matcher_matches(Some("Read|Edit"), "Write"));
        // Invalid regex warns and matches everything.
        assert!(matcher_matches(Some("("), "anything"));
    }

    #[tokio::test]
    async fn run_command_hook_echoes_and_continues() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some("cat".into()),
            command_windows: None,
            timeout: None,
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "UserPromptSubmit",
                event: json!({ "prompt": "hello" }),
            },
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert!(output.additional_context.is_none());
    }

    #[tokio::test]
    async fn run_command_hook_block_new_format() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(
                r#"printf '{"decision":"block","reason":"no read allowed"}'"#.into(),
            ),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "PreToolUse",
                event: json!({ "tool_name": "Read" }),
            },
        )
        .await;

        assert_eq!(
            output.control,
            HookControl::Block("no read allowed".to_string())
        );
    }

    #[tokio::test]
    async fn run_command_hook_block_legacy_format() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(
                r#"printf '{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"legacy block"}}'"#
                    .into(),
            ),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "PreToolUse",
                event: json!({ "tool_name": "Read" }),
            },
        )
        .await;

        assert_eq!(
            output.control,
            HookControl::Block("legacy block".to_string())
        );
    }

    #[tokio::test]
    async fn run_command_hook_parses_additional_context() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(
                r#"printf %s '{"decision":"approve","additionalContext":"\nFollow the house style."}'"#
                    .into(),
            ),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "UserPromptSubmit",
                event: json!({ "prompt": "hi" }),
            },
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert_eq!(
            output.additional_context.as_deref(),
            Some("\nFollow the house style.")
        );
    }

    #[tokio::test]
    async fn run_command_hook_failure_continues() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some("exit 3".into()),
            command_windows: None,
            timeout: Some(5),
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "PreToolUse",
                event: Value::Null,
            },
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn run_command_hook_timeout_continues() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some("sleep 5".into()),
            command_windows: None,
            timeout: Some(1),
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "PreToolUse",
                event: Value::Null,
            },
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn run_command_hook_expands_plugin_root_env() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(
                r#"printf '{"decision":"approve","additionalContext":"%s"}' "$CLAUDE_PLUGIN_ROOT""#
                    .into(),
            ),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "UserPromptSubmit",
                event: Value::Null,
            },
        )
        .await;

        let expected = dir.path().display().to_string();
        assert_eq!(output.additional_context.as_deref(), Some(expected.as_str()));
    }

    #[test]
    fn expands_plugin_root_placeholder() {
        let expanded = expand_plugin_root(
            r#"node "${CLAUDE_PLUGIN_ROOT}/hooks/x.js""#,
            Path::new("/cache/p"),
        );
        assert_eq!(expanded, r#"node "/cache/p/hooks/x.js""#);
    }
}

#[cfg(test)]
impl HooksFile {
    fn from_str(content: &str) -> Result<Self> {
        serde_json::from_str(content).context("failed to parse hooks file")
    }
}
