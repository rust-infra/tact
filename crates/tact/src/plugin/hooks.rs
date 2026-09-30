//! Plugin command hooks: parsing, command execution, and registration.
//!
//! Codex plugins declare hooks through `.codex-plugin/plugin.json`'s
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
//! Tact maps thirteen events to loop points:
//! `SessionStart`, `UserPromptSubmit`, `SubagentStart`, `SubagentStop`,
//! `PreToolUse`, `PostToolUse`, `PostToolUseFailure`, `Notification`,
//! `TaskCompleted`, `Stop`, `SessionEnd`, `PreCompact`, `PostCompact`.
//! Failures (non-zero exit, timeout, invalid JSON) never block the agent
//! loop — they log a warning and continue.

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

use tact_protocol::AgentUpdate;

use crate::{
    compact::CompactTrigger,
    consts::{PluginDirs, PluginHome},
    hook::{
        HookControl, NotificationContext, SessionStartContext, SubagentStartContext,
        SubagentStartFn, SubagentStopContext, SubagentStopFn, ToolResult, ToolUse,
    },
    plugin::PluginStore,
    utils::LockExt,
};

/// A Claude Code hook event that Tact maps to a loop point.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEventKind {
    SessionStart,
    UserPromptSubmit,
    SubagentStart,
    SubagentStop,
    PreToolUse,
    PermissionRequest,
    PostToolUse,
    PostToolUseFailure,
    Notification,
    TaskCompleted,
    Interrupt,
    Stop,
    SessionEnd,
    PreCompact,
    PostCompact,
}

impl HookEventKind {
    /// The Claude Code event name (also the hooks-file key).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionStart => "SessionStart",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::PreToolUse => "PreToolUse",
            Self::PermissionRequest => "PermissionRequest",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::Notification => "Notification",
            Self::TaskCompleted => "TaskCompleted",
            Self::Interrupt => "Interrupt",
            Self::Stop => "Stop",
            Self::SessionEnd => "SessionEnd",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
        }
    }

    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "SessionStart" => Some(Self::SessionStart),
            "UserPromptSubmit" => Some(Self::UserPromptSubmit),
            "SubagentStart" => Some(Self::SubagentStart),
            "SubagentStop" => Some(Self::SubagentStop),
            "PreToolUse" => Some(Self::PreToolUse),
            "PermissionRequest" => Some(Self::PermissionRequest),
            "PostToolUse" => Some(Self::PostToolUse),
            "PostToolUseFailure" => Some(Self::PostToolUseFailure),
            "Notification" => Some(Self::Notification),
            "TaskCompleted" => Some(Self::TaskCompleted),
            "Interrupt" => Some(Self::Interrupt),
            "Stop" => Some(Self::Stop),
            "SessionEnd" => Some(Self::SessionEnd),
            "PreCompact" => Some(Self::PreCompact),
            "PostCompact" => Some(Self::PostCompact),
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

/// What one hook entry declares.
///
/// Resolved from `type` plus the fields present, so a declaration that cannot
/// run is *named* rather than silently inert — the review flow asks a human to
/// approve a definition, and approving one that can never fire is worse than no
/// review at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookKind<'a> {
    /// A shell command (`type: "command"`, or no `type`).
    Command,
    /// `type: "mcp_tool"`: call a tool on a connected MCP server.
    McpTool {
        /// The server half of the `mcp__<server>__<tool>` name.
        server: &'a str,
        /// The tool half.
        tool: &'a str,
    },
    /// Declared, but not runnable. The reason names the missing field.
    Invalid(&'a str),
}

/// One hook entry.
#[derive(Debug, Clone, Default, Deserialize)]
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
    /// Codex `additionalContextLimit`: a per-hook budget for the context this
    /// hook injects, in tokens.
    ///
    /// An upper bound chosen by the hook's author, applied before the client's
    /// own cap in `agent` — a chatty hook is bounded at its own declared size
    /// rather than at whatever the session allows.
    #[serde(default, rename = "additionalContextLimit")]
    pub additional_context_limit: Option<usize>,
    /// `type: "mcp_tool"` — the MCP server whose tool this hook calls.
    ///
    /// Modelled rather than left to serde's unknown-field handling: before this,
    /// `server` / `tool` / `arguments` were dropped without a word, and the
    /// entry still looked admitted.
    #[serde(default)]
    pub server: Option<String>,
    /// `type: "mcp_tool"` — the tool name on that server.
    #[serde(default)]
    pub tool: Option<String>,
    /// `type: "mcp_tool"` — the tool's input. Absent means `{}`.
    ///
    /// Static by design: the hook payload is *not* merged in. A tool call whose
    /// arguments shifted with the event would make the reviewed definition a
    /// lie, and the definition is what the user approved.
    #[serde(default)]
    pub arguments: Option<Value>,
}

impl HookCommand {
    /// What this entry is, and whether it can run at all.
    ///
    /// Deliberately permissive about `type`: an absent or unrecognised value has
    /// always meant "a command with a `command` string", and turning that into
    /// an error would break a working configuration to punish a typo. Only
    /// `mcp_tool` is new, so only `mcp_tool` can be malformed.
    #[must_use]
    pub fn kind(&self) -> HookKind<'_> {
        let is_mcp_tool = matches!(
            self.ty.as_deref().map(str::trim),
            Some("mcp_tool" | "mcpTool")
        );
        if !is_mcp_tool {
            return HookKind::Command;
        }
        let server = self.server.as_deref().unwrap_or("").trim();
        if server.is_empty() {
            return HookKind::Invalid("`server` is required for `type: \"mcp_tool\"`");
        }
        let tool = self.tool.as_deref().unwrap_or("").trim();
        if tool.is_empty() {
            return HookKind::Invalid("`tool` is required for `type: \"mcp_tool\"`");
        }
        HookKind::McpTool { server, tool }
    }
}

impl HooksFile {
    /// Parses a hooks file from disk.
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read hooks file {}", path.display()))?;
        serde_json::from_str(&content)
            .with_context(|| format!("failed to parse hooks file {}", path.display()))
    }

    /// Reads the `[hooks]` table out of a `config.toml`.
    ///
    /// The whole document is parsed as a [`HooksFile`], which has exactly one
    /// field (`hooks`); every other table in the file is ignored by serde. That
    /// keeps the TOML spelling and the JSON one on the same type — same fields,
    /// same validation, same review — instead of a parallel parser that would
    /// drift.
    pub fn from_toml_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read config file {}", path.display()))?;
        toml::from_str(&content)
            .with_context(|| format!("failed to parse [hooks] in {}", path.display()))
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
    /// `additionalContext` — recorded as conversation context by the caller.
    pub additional_context: Option<String>,
    /// `suppressOutput` (PostToolUse) — caller may clear the tool result.
    pub suppress_output: bool,
    /// `continue: false` — Codex's stop. What stopping means depends on the
    /// event: for `SessionStart` the turn does not run at all, and that is the
    /// field a plugin uses (`decision: block` is not in that schema).
    pub stop: bool,
    /// `stopReason` — shown alongside a stop.
    pub stop_reason: Option<String>,
    /// `systemMessage` — surfaced to the reader as a notice; never injected.
    pub system_message: Option<String>,
}

impl HookOutput {
    fn continue_default() -> Self {
        Self::default()
    }

    /// Whether the hook expressed a decision, as opposed to only extra context.
    ///
    /// `additionalContext` and `systemMessage` are additions, not decisions, so
    /// a hook that prints only those still leaves `exit 2` free to block.
    #[must_use]
    fn decided(&self) -> bool {
        !matches!(self.control, HookControl::Continue) || self.stop || self.suppress_output
    }
}

/// Runs one hook entry, whichever kind it declares.
///
/// The two kinds share everything downstream — the same output normalization,
/// the same decision contract, the same `additionalContextLimit` — so only what
/// produces the output differs. This is what every event registration calls;
/// [`run_command_hook`] remains the command implementation.
pub async fn run_hook(
    command: &HookCommand,
    dirs: impl Into<PluginDirs>,
    input: &HookRunInput,
    agent: Option<&crate::Agent>,
) -> HookOutput {
    let dirs = dirs.into();
    match command.kind() {
        HookKind::Command => run_command_hook(command, dirs, input, agent).await,
        HookKind::McpTool { server, tool } => {
            run_mcp_tool_hook(command, server, tool, input, agent).await
        }
        HookKind::Invalid(reason) => {
            // A declaration the user reviewed and approved must not be silently
            // inert — which is exactly what an `mcp_tool` entry was before this.
            report_hook_failure(input, agent, reason);
            HookOutput::continue_default()
        }
    }
}

/// Runs one `mcp_tool` hook: calls a tool on a connected MCP server and
/// normalizes its result exactly as a command hook's stdout is normalized.
///
/// That reuse is the point. A policy can then live in the MCP server that
/// already holds the integration's tools, returning the same `{"decision": …}` /
/// `{"hookSpecificOutput": …}` shapes a shell script would — instead of a script
/// that re-implements the payload, the decision contract and `additionalContext`
/// parsing in whatever language it is written in.
///
/// Failure semantics match every other hook: an unreachable server, an unknown
/// tool or a tool error is reported and continues, because a hook must not be
/// able to halt the loop by being broken.
async fn run_mcp_tool_hook(
    command: &HookCommand,
    server: &str,
    tool: &str,
    input: &HookRunInput,
    agent: Option<&crate::Agent>,
) -> HookOutput {
    if let Some(message) = command.status_message.as_deref() {
        tracing::debug!("plugin hook status: {message}");
    }
    let Some(agent) = agent else {
        report_hook_failure(
            input,
            None,
            "an `mcp_tool` hook needs a live agent to reach the MCP router",
        );
        return HookOutput::continue_default();
    };

    let name = crate::mcp::mcp_tool_name(server, tool);
    let arguments = command.arguments.clone().unwrap_or_else(|| json!({}));
    // The hook's own budget, not the server's `tool_timeout_sec`: this timeout
    // belongs to the definition the user reviewed.
    let call = agent.mcp_router.call(&name, arguments);
    let outcome = match resolve_timeout(command.timeout) {
        Some(secs) => match tokio::time::timeout(Duration::from_secs(secs), call).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("{name} did not answer within {secs}s")),
        },
        None => call.await,
    };

    match outcome {
        Ok(text) => {
            let mut output = parse_output(&text, input.hook_event_name);
            finish_hook_output(&mut output, command, Some(agent));
            output
        }
        Err(error) => {
            report_hook_failure(input, Some(agent), &format!("{error:#}"));
            HookOutput::continue_default()
        }
    }
}

/// Runs one command hook and returns its normalized output.
///
/// Failure semantics (Claude Code compatible): non-zero exit, timeout, invalid
/// JSON, or a missing command never block the loop — a warning is logged and
/// `Continue` is returned. Async hooks are spawned and return immediately.
pub async fn run_command_hook(
    command: &HookCommand,
    dirs: impl Into<PluginDirs>,
    input: &HookRunInput,
    agent: Option<&crate::Agent>,
) -> HookOutput {
    let dirs = dirs.into();
    let Some(raw) = command.command.as_deref() else {
        return HookOutput::continue_default();
    };
    if let Some(message) = command.status_message.as_deref() {
        tracing::debug!("plugin hook status: {message}");
    }
    if command.async_ == Some(true) {
        let expanded = expand_plugin_placeholders(raw, &dirs);
        let work_dir = input.work_dir.clone();
        let dirs = dirs.clone();
        let payload = build_payload(input, agent);
        // Fire-and-forget on the runtime we are already on: a plugin that opts
        // into `async` does not want its result, and building a second runtime
        // per hook just to run one process was pure overhead.
        tokio::spawn(async move {
            let _ = run_process(&expanded, &work_dir, &dirs, &payload, None).await;
        });
        return HookOutput::continue_default();
    }

    let payload = build_payload(input, agent);
    // Claude semantics: default 60s; an explicit `0` disables the timeout.
    let timeout_secs = resolve_timeout(command.timeout);
    match run_process(
        &expand_plugin_placeholders(raw, &dirs),
        &input.work_dir,
        &dirs,
        &payload,
        timeout_secs,
    )
    .await
    {
        Ok(run) => {
            let mut output = parse_output(&run.stdout, input.hook_event_name);
            // `exit 2` is Codex's simple block mechanism: the reason is the
            // stderr text. A JSON decision always wins — a hook that printed one
            // already said what it meant — so this only fills the gap a bare
            // failure leaves.
            if run.exit_code != Some(0) {
                if run.exit_code == Some(2)
                    && !run.stderr.is_empty()
                    && exit_two_blocks(input.hook_event_name)
                    && !output.decided()
                {
                    output.control = HookControl::Block(run.stderr.clone());
                } else if !output.decided() {
                    let detail = if run.stderr.is_empty() {
                        format!("exited with {:?}", run.exit_code)
                    } else {
                        format!("exited with {:?}: {}", run.exit_code, run.stderr)
                    };
                    report_hook_failure(input, agent, &detail);
                }
                // Otherwise the hook printed a decision: honour it. A JSON
                // decision is the richer contract, so a bare exit status must
                // not overwrite what it said.
            }
            finish_hook_output(&mut output, command, agent);
            output
        }
        Err(error) => {
            report_hook_failure(input, agent, &format!("{error}"));
            HookOutput::continue_default()
        }
    }
}

/// Applies the post-processing every hook kind shares.
///
/// Both kinds produce a [`HookOutput`], and both must then honour the same two
/// rules. Factored out so a fix to one cannot miss the other.
fn finish_hook_output(
    output: &mut HookOutput,
    command: &HookCommand,
    agent: Option<&crate::Agent>,
) {
    // A per-hook budget, applied where the context is produced: the hook's
    // author declared how much of it should reach the model. A let-chain, not a
    // tuple pattern: in `if let (Some(limit), Some(context)) = …` the `take()`
    // runs even when there is no limit, and the context is then dropped on the
    // floor.
    if let Some(limit) = command.additional_context_limit
        && let Some(context) = output.additional_context.take()
    {
        let (trimmed, _) =
            crate::utils::truncate::truncate_middle_with_token_budget(&context, limit);
        output.additional_context = Some(if trimmed == context {
            context
        } else {
            format!("{trimmed}\n\n[truncated to {limit} tokens by additionalContextLimit]")
        });
    }
    // Codex records `systemMessage` as a warning entry; it never reaches the
    // conversation.
    if let Some(message) = &output.system_message
        && let Some(agent) = agent
    {
        agent.emit_update(AgentUpdate::Info(message.clone()));
    }
}

/// Reports a hook failure, so it is not a log-only event.
///
/// The warning alone only reaches a log file, and `tact-ui` installs no
/// subscriber unless `RUST_LOG` is set. A hook that failed is otherwise
/// indistinguishable from one that had nothing to say.
fn report_hook_failure(input: &HookRunInput, agent: Option<&crate::Agent>, detail: &str) {
    warn!("plugin hook command failed (continuing): {detail}");
    if let Some(agent) = agent {
        agent.emit_update(AgentUpdate::Info(format!(
            "[plugin hook {} failed] {detail}",
            input.hook_event_name
        )));
    }
}

async fn run_process(
    command: &str,
    work_dir: &Path,
    dirs: &PluginDirs,
    payload: &Value,
    timeout_secs: Option<u64>,
) -> Result<HookRunOutput> {
    // Inject the hook-protocol env vars into the subprocess:
    //   - `CLAUDE_PLUGIN_ROOT` / `PLUGIN_ROOT` = absolute plugin cache root (the
    //     dir holding `.codex-plugin/`, `hooks/`, `skills/`, …). Hook scripts
    //     read it to locate their own resources; the same value is substituted
    //     for the matching placeholder in the command string (see
    //     [`expand_plugin_placeholders`]).
    //   - `CLAUDE_PLUGIN_DATA` / `PLUGIN_DATA` = the plugin's writable directory,
    //     which outlives the revision-hashed package (Agent Plugins §9.1).
    //   - `CLAUDE_PROJECT_DIR` = the agent's current working directory, so a
    //     hook knows which project scope it is running in.
    // Every name is exported in both spellings because two plugin ABIs are in
    // circulation: Claude Code uses the `CLAUDE_`-prefixed pair, while Agent
    // Plugins / Codex bundles use the bare one. Only the manifest dir was
    // renamed to `.codex-plugin`; these env names are part of the cross-tool
    // hook ABI.
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(command)
        .current_dir(work_dir)
        .env("CLAUDE_PLUGIN_ROOT", &dirs.root)
        .env("PLUGIN_ROOT", &dirs.root)
        .env("CLAUDE_PLUGIN_DATA", &dirs.data)
        .env("PLUGIN_DATA", &dirs.data)
        .env("CLAUDE_PROJECT_DIR", work_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // On timeout the `wait_with_output` future is dropped; without this the
        // `sh -c` child is merely detached and keeps running (and holding its
        // stdio pipes) after the hook has been reported as timed out.
        .kill_on_drop(true)
        .spawn()
        .with_context(|| format!("failed to spawn hook command: {command}"))?;

    let mut stdin = child
        .stdin
        .take()
        .context("hook command has no stdin handle")?;
    let payload = serde_json::to_string(payload)?;
    // Best-effort payload delivery: a fast hook that finishes (or closes its
    // stdin) before this write runs — e.g. a one-shot `printf`-style hook on a
    // loaded machine — yields a broken pipe. Its exit status and stdout are
    // authoritative, so that must not fail the run; real I/O errors still do.
    let write_result = stdin.write_all(payload.as_bytes()).await;
    drop(stdin);
    if let Err(error) = write_result
        && error.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(error).context("failed to write hook payload to stdin");
    }

    let output = match timeout_secs {
        Some(secs) => timeout(Duration::from_secs(secs), child.wait_with_output())
            .await
            .with_context(|| format!("hook command timed out after {secs}s: {command}"))??,
        None => child.wait_with_output().await?,
    };

    // A non-zero exit is reported to the caller instead of failing here: Codex
    // gives `exit 2` a per-event meaning that only the caller can apply.
    Ok(HookRunOutput {
        stdout: String::from_utf8_lossy(&output.stdout).trim().to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        exit_code: output.status.code(),
    })
}

/// What one hook subprocess produced.
///
/// A non-zero exit is *not* an `Err` any more: Codex gives `exit 2` a meaning
/// (block, with the reason on stderr), so the caller needs the status and the
/// stderr to apply it. A spawn failure or a timeout is still an `Err`, because
/// there is no status to interpret.
struct HookRunOutput {
    /// Trimmed stdout.
    stdout: String,
    /// Trimmed stderr — Codex reads a block reason from here.
    stderr: String,
    /// The exit code, when the process exited normally.
    exit_code: Option<i32>,
}

/// Whether an `exit 2` from this event blocks anything.
///
/// Codex defines the contract per event: `PreToolUse` / `PermissionRequest`
/// block a tool, `PostToolUse` fails a result, and `Stop` / `SubagentStop` /
/// `UserPromptSubmit` continue with the stderr text as the next prompt. For
/// every other event (`SessionStart`, `PreCompact`, `SessionEnd`, …) a bare
/// `exit 2` has no meaning, so it stays fail-open — but it is still reported.
fn exit_two_blocks(event_name: &str) -> bool {
    matches!(
        event_name,
        "PreToolUse"
            | "PermissionRequest"
            | "PostToolUse"
            | "Stop"
            | "SubagentStop"
            | "UserPromptSubmit"
    )
}

/// Resolves a hook's timeout: an explicit `0` disables the timeout, `None`
/// defaults to 60s (Claude Code semantics).
fn resolve_timeout(timeout: Option<u64>) -> Option<u64> {
    match timeout {
        Some(0) => None,
        other => other.or(Some(60)),
    }
}

/// Builds a hook's stdin payload.
///
/// The base set (`session_id`, `transcript_path`, `cwd`, `hook_event_name`),
/// plus the fields **every** event carries in Codex's schema — `model`,
/// `permission_mode`, `turn_id` — plus the event-specific ones merged at the
/// top level (the protocol puts `tool_name`, `tool_input`, `prompt`, … at the
/// root, not nested).
///
/// `transcript_path` is `null`: Tact keeps a session in SQLite and writes a
/// transcript only when compaction runs, so there is no single live file to
/// name (Codex points at its rollout file). Reporting the transcripts
/// *directory* here — as this did — was worse than null, because a plugin opens
/// what it is given.
fn build_payload(input: &HookRunInput, agent: Option<&crate::Agent>) -> Value {
    let mut payload = json!({
        // Callers historically passed `String::new()`; the live session id is
        // the one a plugin can use to correlate runs.
        "session_id": agent
            .and_then(|agent| agent.runtime.session_id.clone())
            .unwrap_or_else(|| input.session_id.clone()),
        "transcript_path": Value::Null,
        "cwd": input.work_dir,
        "hook_event_name": input.hook_event_name,
    });
    if let Some(agent) = agent {
        payload["model"] = json!(agent.model());
        payload["permission_mode"] = json!(agent.runtime.permission_manager.mode().hook_name());
        payload["turn_id"] = json!(agent.turns_taken.to_string());
    }
    if let Some(object) = input.event.as_object() {
        for (key, value) in object {
            payload[key] = value.clone();
        }
    }
    payload
}

/// Parses hook stdout, accepting JSON in both the newer `decision` shape and
/// the legacy `hookSpecificOutput` shape, plus Claude's plain-text fallback:
/// for `UserPromptSubmit` / `SessionStart`, non-JSON stdout is treated as
/// `additionalContext` verbatim (ponytail, and the reference `basic-memory`
/// plugin, print their context this way).
///
/// Stdout that *looks* like JSON but does not parse is a failure, not context —
/// the same line Codex's `output_parser::looks_like_json` draws. Injecting a
/// half-written payload would poison the conversation with the hook's source.
fn parse_output(stdout: &str, event_name: &str) -> HookOutput {
    let raw: Result<RawHookOutput, _> = serde_json::from_str(stdout);
    let raw = match raw {
        Ok(raw) => raw,
        Err(error) if matches!(event_name, "UserPromptSubmit" | "SessionStart") => {
            let text = stdout.trim();
            if text.is_empty() {
                return HookOutput::continue_default();
            }
            if looks_like_json(stdout) {
                warn!("plugin hook returned invalid JSON (continuing): {error}");
                return HookOutput::continue_default();
            }
            return HookOutput {
                control: HookControl::Continue,
                additional_context: Some(text.to_string()),
                ..HookOutput::default()
            };
        }
        Err(error) => {
            warn!("plugin hook returned invalid JSON (continuing): {error}");
            return HookOutput::continue_default();
        }
    };

    let legacy = raw.hook_specific_output.as_ref();
    let permission_decision = legacy.and_then(|l| l.permission_decision.as_deref());
    let blocked = raw.decision.as_deref() == Some("block") || permission_decision == Some("deny");
    // Codex's `permissionDecision: "allow"` is how a hook answers the approval
    // prompt itself. It is only *meaningful* on `PreToolUse` /
    // `PermissionRequest`; other events treat it as "nothing to say", which is
    // what folding it into the macro's `Allow` arm already does.
    let allowed = permission_decision == Some("allow");
    let reason = raw
        .reason
        .clone()
        .or_else(|| legacy.and_then(|l| l.permission_decision_reason.clone()))
        .unwrap_or_else(|| "blocked by plugin hook".to_string());

    HookOutput {
        control: if blocked {
            HookControl::Block(reason)
        } else if allowed {
            HookControl::Allow
        } else {
            HookControl::Continue
        },
        additional_context: raw
            .additional_context
            .clone()
            .or_else(|| legacy.and_then(|l| l.additional_context.clone())),
        stop: raw.continue_processing == Some(false),
        stop_reason: raw.stop_reason.clone(),
        system_message: raw.system_message.clone(),
        suppress_output: raw
            .suppress_output
            .or_else(|| legacy.and_then(|l| l.suppress_output))
            .unwrap_or(false),
    }
}

/// Routes one completed command hook's output into the session-start context.
///
/// Extracted so the routing is testable without standing up a full `Agent`:
/// this step is the bug it replaces — `additionalContext` was logged as a
/// warning and dropped, which made the reference `basic-memory` plugin's
/// session briefing inert.
fn collect_session_start_output(output: &HookOutput, context: &mut SessionStartContext) {
    if let Some(additional) = &output.additional_context {
        context.push_additional_context(additional);
    }
}

/// Whether stdout advertises itself as JSON — a `{` or `[` after leading
/// whitespace (Codex's `output_parser::looks_like_json`).
fn looks_like_json(stdout: &str) -> bool {
    let trimmed = stdout.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
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
    suppress_output: Option<bool>,
    #[serde(default, rename = "continue")]
    continue_processing: Option<bool>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    system_message: Option<String>,
    #[serde(default)]
    hook_specific_output: Option<RawHookSpecificOutput>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawHookSpecificOutput {
    // `permissionDecision` is not Claude-only: Codex's PreToolUse parser accepts
    // it too (`PreToolUsePermissionDecisionWire`), as an alternative to the
    // top-level `decision: "block"`.
    #[serde(default)]
    permission_decision: Option<String>,
    #[serde(default)]
    permission_decision_reason: Option<String>,
    #[serde(default)]
    additional_context: Option<String>,
    #[serde(default)]
    suppress_output: Option<bool>,
}

/// Expands the plugin directory placeholders in a command string.
///
/// Four names are in circulation and all are accepted, in braced and bare form:
///
/// - `${CLAUDE_PLUGIN_ROOT}` / `${PLUGIN_ROOT}` — the Claude Code hook ABI and
///   the Agent Plugins / Codex bundle ABI respectively, both naming the unpacked
///   package (the reference `basic-memory` plugin runs
///   `uv run --quiet --script "${PLUGIN_ROOT}/hooks/session_start.py"`);
/// - `${CLAUDE_PLUGIN_DATA}` / `${PLUGIN_DATA}` — the writable directory that
///   survives package updates (Agent Plugins §9.1).
///
/// Plugins need the placeholders because the revision-hashed cache path is
/// unknowable ahead of time. We resolve them at spawn time so the hook needs no
/// separate discovery step; the same values are exported as env vars (see
/// [`run_process`]). Accepting only the Claude root spelling used to leave the
/// Codex one to `sh`, where an unset `${PLUGIN_ROOT}` expands to the empty
/// string and the hook silently addressed `/hooks/...`.
fn expand_plugin_placeholders(command: &str, dirs: &PluginDirs) -> String {
    // Placeholder name and the directory it names, one pair per plugin ABI.
    let names = [
        ("CLAUDE_PLUGIN_ROOT", dirs.root.as_path()),
        ("PLUGIN_ROOT", dirs.root.as_path()),
        ("CLAUDE_PLUGIN_DATA", dirs.data.as_path()),
        ("PLUGIN_DATA", dirs.data.as_path()),
    ];
    let mut expanded = command.to_owned();
    for (name, value) in names {
        let value = value.to_string_lossy();
        expanded = expanded.replace(&format!("${{{name}}}"), &value);
        expanded = replace_bare_var(&expanded, name, &value);
    }
    expanded
}

/// Substitutes a bare `$NAME` (no braces), leaving a longer variable name
/// alone.
///
/// A plain `str::replace` would rewrite `$PLUGIN_ROOT_SUFFIX` into
/// `<root>_SUFFIX`, but the shell reads that as the single variable
/// `PLUGIN_ROOT_SUFFIX`; the name has to end where the placeholder does.
fn replace_bare_var(command: &str, name: &str, value: &str) -> String {
    let bare = format!("${name}");
    let mut out = String::with_capacity(command.len());
    let mut rest = command;
    while let Some(at) = rest.find(&bare) {
        let (head, tail) = rest.split_at(at);
        let after = &tail[bare.len()..];
        out.push_str(head);
        match after.chars().next() {
            // A longer variable name: leave it for the shell to expand.
            Some(next) if next.is_alphanumeric() || next == '_' => out.push_str(&bare),
            _ => out.push_str(value),
        }
        rest = after;
    }
    out.push_str(rest);
    out
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

/// Resolves a plugin's hooks file path, honouring the manifest `hooks` string
/// field and the default discovery path `hooks/hooks.json` (several official
/// marketplace plugins omit the manifest field and rely on the default).
fn resolve_hooks_path(root: &Path, manifest: &HookManifest) -> Option<PathBuf> {
    if let Some(serde_json::Value::String(relative)) = manifest.hooks.as_ref() {
        let candidate = root.join(relative);
        if candidate.is_file() {
            return Some(candidate);
        }
        warn!(
            "plugin at {} declares hooks at {} but the file is missing",
            root.display(),
            candidate.display()
        );
    }
    let default = root.join("hooks").join("hooks.json");
    default.is_file().then_some(default)
}

/// Parses an inlined manifest `hooks` map (`"hooks": { "SessionStart": [...] }`).
///
/// Codex plugins may embed the matcher map instead of naming a hooks file; the
/// inline map is the same shape as a hooks file's `hooks` object.
fn inline_hooks(manifest: &HookManifest) -> Option<HooksFile> {
    let serde_json::Value::Object(map) = manifest.hooks.as_ref()? else {
        return None;
    };
    let hooks =
        match serde_json::from_value::<HashMap<String, Vec<HookMatcher>>>(map.clone().into()) {
            Ok(hooks) => hooks,
            Err(error) => {
                warn!("plugin manifest has unparseable inline hooks: {error}");
                return None;
            }
        };
    Some(HooksFile { hooks })
}

/// Loads every installed plugin's hooks file.
fn plugin_hook_sources(home: &PluginHome) -> Result<Vec<HookSource>> {
    let store = PluginStore::new(home.clone());
    let mut out = Vec::new();
    for root in store.installed_plugin_roots()? {
        let manifest_path = root.root.join(".codex-plugin").join("plugin.json");
        if !manifest_path.is_file() {
            continue;
        }
        let raw = std::fs::read_to_string(&manifest_path)
            .with_context(|| format!("failed to read {}", manifest_path.display()))?;
        let manifest = match serde_json::from_str::<HookManifest>(&raw) {
            Ok(manifest) => manifest,
            Err(error) => {
                warn!(
                    "plugin {} manifest skipped (unparseable): {error}",
                    root.plugin_id
                );
                continue;
            }
        };
        let Some(hooks) = load_installed_hooks(&root.root, &manifest) else {
            continue;
        };
        let origin = HookOrigin::Plugin {
            id: root.plugin_id.clone(),
        };
        let dirs = PluginDirs {
            data: home.plugin_data_dir(&root.marketplace, &root.plugin_id),
            root: root.root,
        };
        let label = match &origin {
            HookOrigin::Plugin { id } => format!("plugin {id}"),
            // Neither file origin is reachable from a plugin bundle.
            HookOrigin::UserFile => "~/.tact/hooks.json".to_string(),
            HookOrigin::ProjectFile => format!("{}/.tact/hooks.json", dirs.root.display()),
        };
        out.push(HookSource {
            label,
            origin,
            dirs,
            hooks,
        });
    }
    Ok(out)
}

/// Reads one `.tact/hooks.json`.
///
/// A missing file is normal (most users have none). A file that cannot be read
/// or parsed is reported and skipped rather than aborting startup: a repository
/// can ship the project-scoped one, and one bad file must not stop Tact from
/// starting in that directory — the same rule a project `.mcp.json` gets.
fn hooks_file_source(
    path: &Path,
    label: String,
    origin: HookOrigin,
    dirs: PluginDirs,
) -> Option<HookSource> {
    if !path.is_file() {
        return None;
    }
    match HooksFile::from_file(path) {
        Ok(hooks) => Some(HookSource {
            label,
            origin,
            dirs,
            hooks,
        }),
        Err(error) => {
            warn!("hooks at {} skipped: {error:#}", path.display());
            None
        }
    }
}

/// Every hooks file Tact reads, in registration order.
///
/// Plugins first (their order is unchanged, so this cannot reorder an existing
/// user's `SessionStart` context), then `~/.tact/hooks.json`, then
/// `<workdir>/.tact/hooks.json`. Codex runs user, project and plugin hooks
/// together rather than letting one layer replace another, and so does this.
fn collect_hook_sources(home: Option<&PluginHome>, work_dir: &Path) -> Result<Vec<HookSource>> {
    collect_hook_sources_with(home, crate::consts::TactPath::home_hooks_path(), work_dir)
}

/// [`collect_hook_sources`] with the user file named explicitly, so a test does
/// not have to move the process-wide `HOME` (it would otherwise pick up the
/// developer's real `~/.tact/hooks.json`).
fn collect_hook_sources_with(
    home: Option<&PluginHome>,
    user_file: Option<PathBuf>,
    work_dir: &Path,
) -> Result<Vec<HookSource>> {
    let mut out = match home {
        Some(home) => plugin_hook_sources(home)?,
        None => Vec::new(),
    };

    if let Some(path) = user_file.clone() {
        // The user file's `${PLUGIN_ROOT}` is its own directory — there is no
        // package — and `${PLUGIN_DATA}` is `~/.tact`, which outlives it.
        let dirs = PluginDirs {
            root: path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            data: crate::consts::TactPath::home_tact_dir().unwrap_or_else(|| PathBuf::from(".")),
        };
        let label = "~/.tact/hooks.json".to_string();
        if let Some(source) = hooks_file_source(&path, label, HookOrigin::UserFile, dirs) {
            out.push(source);
        }
    }

    let tact_dir = crate::consts::TactPath::new(work_dir).tact_dir();
    let dirs = PluginDirs {
        root: tact_dir.clone(),
        data: tact_dir,
    };
    let project = crate::consts::TactPath::new(work_dir).hooks_path();
    let label = project.display().to_string();
    if let Some(source) = hooks_file_source(&project, label, HookOrigin::ProjectFile, dirs) {
        out.push(source);
    }

    // `config.toml` last, and one source per file. Hooks are additive here as
    // everywhere else, so the only thing a new source can disturb is
    // registration order — and appending keeps every existing order intact.
    // Merging the config files the way the config *loader* merges its values
    // would be wrong: a hook is reviewed by identity, and the review has to name
    // the file it came from.
    for (path, origin) in config_hook_paths(work_dir, user_file.as_deref()) {
        if !path.is_file() {
            continue;
        }
        let hooks = match HooksFile::from_toml_file(&path) {
            Ok(hooks) => hooks,
            // Same rule as a malformed `hooks.json`: warn, skip, never make the
            // whole load fail over one bad file.
            Err(error) => {
                warn!("[hooks] in {} skipped: {error:#}", path.display());
                continue;
            }
        };
        // A file with no `[hooks]` table contributes nothing, rather than an
        // empty source that `hooks list` would have to explain away.
        if hooks.hooks.is_empty() {
            continue;
        }
        // `${PLUGIN_ROOT}` is the file's own directory and `${PLUGIN_DATA}` the
        // `.tact` directory that holds it — the same rule the `hooks.json`
        // sources above follow, so a command written for one spelling works in
        // the other.
        let parent = path.parent().unwrap_or(Path::new(".")).to_path_buf();
        let data = match origin {
            HookOrigin::UserFile => parent.clone(),
            _ => crate::consts::TactPath::new(work_dir).tact_dir(),
        };
        out.push(HookSource {
            label: path.display().to_string(),
            origin,
            dirs: PluginDirs { root: parent, data },
            hooks,
        });
    }

    Ok(out)
}

/// The `config.toml` files that may carry a `[hooks]` table, in ascending
/// specificity.
///
/// The user file is the `config.toml` beside the user's `hooks.json` rather than
/// a fresh `$HOME` lookup: that keeps the two user-scope sources together, and
/// it makes the pair injectable as one value (see `user_file`), which is also
/// why a caller passing `None` gets no user-scope hooks of either spelling.
///
/// Deliberately not [`crate::config`]'s own search order: that one is ordered so
/// a later file's *values* win, while these are additive and only their order
/// matters. Ascending specificity matches the `hooks.json` pair above it.
fn config_hook_paths(work_dir: &Path, user_file: Option<&Path>) -> Vec<(PathBuf, HookOrigin)> {
    let mut paths = Vec::new();
    if let Some(parent) = user_file.and_then(|path| path.parent()) {
        paths.push((parent.join("config.toml"), HookOrigin::UserFile));
    }
    paths.push((work_dir.join("config.toml"), HookOrigin::ProjectFile));
    paths.push((
        work_dir.join(".tact").join("config.toml"),
        HookOrigin::ProjectFile,
    ));
    paths
}

/// Splits one source's hooks into the reviewed ones and the ones that still
/// need review.
///
/// Returns a copy of the source's hooks containing only reviewed definitions, so
/// the registration code runs unchanged and an unreviewed command is never even
/// reachable — the file is read, and then refused.
fn admit_trusted(source: &HookSource, trust: &HookTrust, report: &mut HookLoadReport) -> HooksFile {
    let mut kept: HashMap<String, Vec<HookMatcher>> = HashMap::new();
    for (event, matchers) in &source.hooks.hooks {
        let mut kept_matchers = Vec::new();
        for matcher in matchers {
            let mut kept_commands = Vec::new();
            for command in &matcher.hooks {
                let command_text = definition_text(command);
                let summary = HookSummary {
                    hash: hook_definition_hash(
                        &source.label,
                        event,
                        matcher.matcher.as_deref(),
                        &command_text,
                    ),
                    source: source.label.clone(),
                    event: event.clone(),
                    matcher: matcher.matcher.clone(),
                    command: command_text.to_string(),
                };
                if trust.is_trusted(&summary.hash) {
                    report.trusted.push(summary);
                    kept_commands.push(command.clone());
                } else {
                    report.pending.push(summary);
                }
            }
            if !kept_commands.is_empty() {
                kept_matchers.push(HookMatcher {
                    matcher: matcher.matcher.clone(),
                    hooks: kept_commands,
                });
            }
        }
        if !kept_matchers.is_empty() {
            kept.insert(event.clone(), kept_matchers);
        }
    }
    HooksFile { hooks: kept }
}

/// Loads one installed plugin's hooks, preferring an inline manifest map and
/// otherwise reading the manifest-named or default hooks file.
fn load_installed_hooks(root: &Path, manifest: &HookManifest) -> Option<HooksFile> {
    if let Some(hooks) = inline_hooks(manifest) {
        return Some(hooks);
    }
    let hooks_path = resolve_hooks_path(root, manifest)?;
    match HooksFile::from_file(&hooks_path) {
        Ok(hooks) => Some(hooks),
        Err(error) => {
            warn!("plugin hooks at {} skipped: {error:#}", root.display());
            None
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HookManifest {
    /// Either a repository-relative hooks file path or an inline matcher map.
    #[serde(default)]
    hooks: Option<serde_json::Value>,
}

/// Where a hooks file came from.
///
/// The origin decides two things: how the review prompt names the hook, and
/// what `${PLUGIN_ROOT}` / `${PLUGIN_DATA}` mean inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookOrigin {
    /// An installed marketplace plugin's bundle.
    Plugin { id: String },
    /// `~/.tact/hooks.json`.
    UserFile,
    /// `<workdir>/.tact/hooks.json`.
    ProjectFile,
}

/// One hooks file to register, whichever origin it came from.
pub struct HookSource {
    /// Human-readable origin, also half of every hook's identity hash.
    pub label: String,
    pub origin: HookOrigin,
    /// The two directories a hook command may expand: for a plugin, its package
    /// and data directory; for a `.tact/hooks.json`, the file's directory and
    /// the `.tact` directory that holds it.
    pub dirs: PluginDirs,
    pub hooks: HooksFile,
}

/// One hook definition as the review surfaces describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookSummary {
    /// [`hook_definition_hash`] of exactly these fields.
    pub hash: String,
    /// The source label (`plugin ponytail`, `~/.tact/hooks.json`, …).
    pub source: String,
    pub event: String,
    pub matcher: Option<String>,
    pub command: String,
}

impl HookSummary {
    /// One line for `hooks list` and the review notice.
    #[must_use]
    pub fn describe(&self) -> String {
        match &self.matcher {
            Some(matcher) => format!(
                "{}  {}  [{}]  {}",
                self.source, self.event, matcher, self.command
            ),
            None => format!("{}  {}  {}", self.source, self.event, self.command),
        }
    }
}

/// What one hook entry actually runs, as text.
///
/// The identity hash and the review listing both read this, and both must see
/// the whole definition. A command hook is its command string; an `mcp_tool`
/// hook has no command, so hashing on `command.unwrap_or_default()` would give
/// every `mcp_tool` entry in one source the *same* identity — approving one
/// would approve the rest, editing `tool` or `arguments` would not invalidate
/// the approval, and the review would show a blank line where the definition
/// should be.
#[must_use]
fn definition_text(command: &HookCommand) -> String {
    match command.kind() {
        HookKind::Command => command.command.clone().unwrap_or_default(),
        HookKind::McpTool { server, tool } => {
            // The arguments are part of the definition: the same tool called
            // with a different input is a different thing to approve.
            let arguments = command
                .arguments
                .as_ref()
                .map(|value| value.to_string())
                .unwrap_or_else(|| "{}".to_string());
            format!("mcp_tool {server}/{tool} {arguments}")
        }
        HookKind::Invalid(reason) => format!("unrunnable: {reason}"),
    }
}

/// Identifies a hook by its **definition**, so editing a command invalidates an
/// earlier approval — the property Codex gets from its `trusted_hash`.
///
/// The source label is part of the identity: the same command in a plugin and in
/// a repository's `.tact/hooks.json` are two different decisions, and approving
/// one must not approve the other.
#[must_use]
pub fn hook_definition_hash(
    source: &str,
    event: &str,
    matcher: Option<&str>,
    command: &str,
) -> String {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in [source, event, matcher.unwrap_or_default(), command] {
        hasher.update(part.as_bytes());
        // A separator that cannot appear in a JSON string, so `"ab" + "c"` and
        // `"a" + "bc"` cannot hash the same.
        hasher.update(b"\0");
    }
    format!("{:x}", hasher.finalize())
}

/// What one load decided about the configured hooks.
#[derive(Debug, Clone, Default)]
pub struct HookLoadReport {
    /// Hooks whose definition has been reviewed; these run.
    pub trusted: Vec<HookSummary>,
    /// Hooks skipped because their definition is new or changed since review.
    ///
    /// Fail-closed: an unreviewed hook is never spawned. A repository that ships
    /// `.tact/hooks.json` therefore cannot execute anything by being cloned.
    pub pending: Vec<HookSummary>,
}

impl HookLoadReport {
    /// True when nothing needs the reader's attention.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.pending.is_empty()
    }

    /// One line per fact, for both the TUI notice and headless stderr.
    #[must_use]
    pub fn notice_lines(&self) -> Vec<String> {
        if self.pending.is_empty() {
            return Vec::new();
        }
        let mut lines = vec![format!(
            "{} hook(s) need review and were not run:",
            self.pending.len()
        )];
        for hook in &self.pending {
            lines.push(format!("  {}", hook.describe()));
        }
        lines.push(
            "Review with `tact-ui hooks list`, then `tact-ui hooks trust --all` \
             (or /hooks trust all)."
                .to_string(),
        );
        lines
    }
}

/// The user's hook review decisions, keyed by [`hook_definition_hash`].
#[derive(Debug, Clone, Default)]
pub struct HookTrust {
    path: Option<PathBuf>,
    trusted: HashMap<String, String>,
    /// Test-only: approve every definition without touching the filesystem.
    ///
    /// A field rather than a constructor side effect, so the exception cannot
    /// leak into a production `HookTrust::load`.
    #[cfg(test)]
    trust_everything: bool,
}

impl HookTrust {
    /// Loads `~/.tact/hooks-state.json`. A missing or unreadable file is an
    /// empty store, which means *nothing* runs until reviewed.
    #[must_use]
    pub fn load() -> Self {
        match crate::consts::TactPath::home_hooks_state_path() {
            Some(path) => Self::from_path(path),
            None => Self::default(),
        }
    }

    /// Loads the store at an explicit path (tests, and the CLI's `--home`).
    #[must_use]
    pub fn from_path(path: PathBuf) -> Self {
        let mut trust = Self {
            path: Some(path.clone()),
            ..Self::default()
        };
        let Ok(raw) = std::fs::read_to_string(&path) else {
            return trust;
        };
        match serde_json::from_str::<HookStateFile>(&raw) {
            Ok(parsed) => trust.trusted = parsed.trusted,
            // A store that cannot be parsed must not be silently trusted: an
            // empty map means everything goes back to needing review.
            Err(error) => warn!("hook review store {} ignored: {error}", path.display()),
        }
        trust
    }

    /// A store that trusts every definition, for tests that are about hook
    /// behaviour rather than about review.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn trusting_everything() -> Self {
        Self {
            trust_everything: true,
            ..Self::default()
        }
    }

    /// The store's file, for the CLI's messages.
    #[must_use]
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Whether this definition has been reviewed and approved.
    #[must_use]
    pub fn is_trusted(&self, hash: &str) -> bool {
        #[cfg(test)]
        if self.trust_everything {
            return true;
        }
        self.trusted.contains_key(hash)
    }

    /// Records approval for `entries` and persists the store.
    ///
    /// Writing only happens when a path is configured: a `--home`-less store is
    /// a read-only view, and silently discarding a decision would be worse than
    /// reporting it.
    pub fn trust(&mut self, entries: &[HookSummary]) -> Result<usize> {
        for entry in entries {
            self.trusted.insert(entry.hash.clone(), entry.describe());
        }
        self.persist()?;
        Ok(entries.len())
    }

    /// Forgets every decision, so every hook needs review again.
    pub fn forget_all(&mut self) -> Result<()> {
        self.trusted.clear();
        self.persist()
    }

    /// Every recorded decision, newest state, sorted for stable output.
    #[must_use]
    pub fn recorded(&self) -> Vec<(&str, &str)> {
        let mut out: Vec<(&str, &str)> = self
            .trusted
            .iter()
            .map(|(hash, label)| (hash.as_str(), label.as_str()))
            .collect();
        out.sort_by(|a, b| a.1.cmp(b.1));
        out
    }

    fn persist(&self) -> Result<()> {
        let Some(path) = self.path.as_deref() else {
            return Ok(());
        };
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        let body = serde_json::to_string_pretty(&HookStateFile {
            version: 1,
            trusted: self.trusted.clone(),
        })?;
        std::fs::write(path, body)
            .with_context(|| format!("failed to write hook review store {}", path.display()))
    }
}

/// On-disk shape of [`HookTrust`].
#[derive(Debug, Clone, Default, Deserialize, serde::Serialize)]
struct HookStateFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    trusted: HashMap<String, String>,
}

/// Builds `SubagentStart` command-hook closures for every installed plugin.
///
/// The closures are stored on [`ToolContext`](crate::tool::ToolContext) and
/// invoked by `spawn_subagent`; `additionalContext` output is appended to the
/// child's system prompt and a `block` decision fails the spawn.
pub fn plugin_subagent_start_hooks(work_dir: &Path) -> Result<Vec<Arc<dyn SubagentStartFn>>> {
    let home = PluginHome::from_environment();
    let sources = collect_hook_sources(home.as_ref(), work_dir)?;
    let trust = HookTrust::load();
    Ok(subagent_start_hooks_from(&sources, &trust, work_dir))
}

/// [`plugin_subagent_start_hooks`] against explicit sources and a store, so
/// tests do not have to move the process-wide `HOME` or write the real review
/// store.
#[cfg(test)]
fn subagent_start_hooks_with(
    home: &PluginHome,
    trust: &HookTrust,
    work_dir: &Path,
) -> Result<Vec<Arc<dyn SubagentStartFn>>> {
    let sources = collect_hook_sources(Some(home), work_dir)?;
    Ok(subagent_start_hooks_from(&sources, trust, work_dir))
}

fn subagent_start_hooks_from(
    sources: &[HookSource],
    trust: &HookTrust,
    work_dir: &Path,
) -> Vec<Arc<dyn SubagentStartFn>> {
    let mut out: Vec<Arc<dyn SubagentStartFn>> = Vec::new();
    for source in sources {
        // The pending list is reported once by the main load; here an
        // unreviewed hook is simply not registered.
        let mut report = HookLoadReport::default();
        let hooks = admit_trusted(source, trust, &mut report);
        for (matcher, command) in hooks.commands_for(HookEventKind::SubagentStart) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.to_path_buf();
            out.push(Arc::new(move |ctx: &mut SubagentStartContext| {
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                let matcher = matcher.clone();
                let name = ctx.name.clone();
                let prompt = ctx.prompt.clone();
                Box::pin(async move {
                    if !matcher_matches(matcher.as_deref(), &name) {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "SubagentStart",
                            event: json!({
                                // Claude protocol: `agent_type` is the
                                // subagent name (ponytail's
                                // PONYTAIL_SUBAGENT_MATCHER scoping reads this
                                // field); `subagent_name` is kept for
                                // readability.
                                "agent_type": name,
                                "subagent_name": name,
                                "prompt": prompt,
                            }),
                        },
                        None,
                    )
                    .await;
                    if let Some(extra) = output.additional_context {
                        ctx.system_prompt.push_str(&extra);
                    }
                    // Propagate the block so `spawn_subagent` can veto the
                    // spawn; only transport failures stay fail-open.
                    Ok(output.control)
                })
            }));
        }
    }
    out
}

/// Builds `SubagentStop` command-hook closures for every installed plugin.
///
/// Stored on [`ToolContext`](crate::tool::ToolContext) and invoked by
/// `spawn_subagent` after the child finishes; the `additionalContext` /
/// `decision` output is read but a `block` is ignored (the child already
/// returned), and the hook may not rewrite the summary in v1 — matching the
/// observational SubagentStop contract.
pub fn plugin_subagent_stop_hooks(work_dir: &Path) -> Result<Vec<Arc<dyn SubagentStopFn>>> {
    let home = PluginHome::from_environment();
    let sources = collect_hook_sources(home.as_ref(), work_dir)?;
    let trust = HookTrust::load();
    Ok(subagent_stop_hooks_from(&sources, &trust, work_dir))
}

fn subagent_stop_hooks_from(
    sources: &[HookSource],
    trust: &HookTrust,
    work_dir: &Path,
) -> Vec<Arc<dyn SubagentStopFn>> {
    let mut out: Vec<Arc<dyn SubagentStopFn>> = Vec::new();
    for source in sources {
        // Reported by the main load; see the start loader above.
        let mut report = HookLoadReport::default();
        let hooks = admit_trusted(source, trust, &mut report);
        for (matcher, command) in hooks.commands_for(HookEventKind::SubagentStop) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.to_path_buf();
            out.push(Arc::new(move |ctx: &mut SubagentStopContext| {
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                let matcher = matcher.clone();
                let agent_type = ctx.agent_type.clone();
                let summary = ctx.summary.clone();
                Box::pin(async move {
                    if !matcher_matches(matcher.as_deref(), &agent_type) {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "SubagentStop",
                            event: json!({
                                "agent_id": agent_type,
                                "agent_type": agent_type,
                                "last_assistant_message": summary,
                            }),
                        },
                        None,
                    )
                    .await;
                    // Observational: a block cannot resume a finished child;
                    // ignore it but log nothing extra (run_command_hook already
                    // warns on transport failures).
                    let _ = output;
                    Ok(HookControl::Continue)
                })
            }));
        }
    }
    out
}

/// Applies every command hook an agent may run: installed plugins, then
/// `~/.tact/hooks.json`, then `<workdir>/.tact/hooks.json`.
///
/// Hooks are appended after any existing Rust closures, per source in that
/// order — plugin order is `<marketplace>/<plugin>` key order, the order of
/// `installed.json`'s `BTreeMap`, not installation time.
///
/// A `block` output from `PreToolUse` / `PostToolUse` /
/// `Stop` / `PreCompact` propagates through [`HookControl`]; `UserPromptSubmit`
/// appends `additionalContext` to the prompt; `SessionStart`'s
/// `additionalContext` is collected for injection before the first turn;
/// `SessionEnd` / `PostCompact` / `SubagentStop` / `PostToolUseFailure` /
/// `Notification` / `TaskCompleted` are observational.
///
/// Only hooks whose definition has been reviewed are registered; the returned
/// report names the ones that were skipped. Prefer
/// [`apply_plugin_hooks_with_report`] at an entry point so the reader learns
/// about them.
pub fn apply_plugin_hooks(agent: crate::Agent, work_dir: &Path) -> Result<crate::Agent> {
    Ok(apply_plugin_hooks_with_report(agent, work_dir)?.0)
}

/// [`apply_plugin_hooks`], keeping the review report.
pub fn apply_plugin_hooks_with_report(
    agent: crate::Agent,
    work_dir: &Path,
) -> Result<(crate::Agent, HookLoadReport)> {
    let home = PluginHome::from_environment();
    let sources = collect_hook_sources(home.as_ref(), work_dir)?;
    let trust = HookTrust::load();
    Ok(apply_hook_sources(sources, &trust, agent, work_dir))
}

/// [`apply_hook_sources`] against an explicit plugin home and store, so tests do
/// not have to move the process-wide `HOME` or write the real review store.
#[cfg(test)]
fn apply_hooks_with(
    home: &PluginHome,
    trust: &HookTrust,
    agent: crate::Agent,
    work_dir: &Path,
) -> Result<(crate::Agent, HookLoadReport)> {
    let sources = collect_hook_sources(Some(home), work_dir)?;
    Ok(apply_hook_sources(sources, trust, agent, work_dir))
}

/// Every configured hook in `work_dir`, with its review status, without
/// registering or running anything.
///
/// This is what `tact-ui hooks list` shows and what [`trust_hooks`] acts on; the
/// split into `trusted` / `pending` is the same one the load path makes, so the
/// listing cannot disagree with what actually runs.
pub fn survey_hooks(work_dir: &Path) -> Result<HookLoadReport> {
    let home = PluginHome::from_environment();
    let sources = collect_hook_sources(home.as_ref(), work_dir)?;
    let trust = HookTrust::load();
    let mut report = HookLoadReport::default();
    for source in &sources {
        admit_trusted(source, &trust, &mut report);
    }
    Ok(report)
}

/// Marks hooks as reviewed, so the next session runs them.
///
/// `all` approves every pending hook; `source` narrows to one source label
/// (`plugin ponytail`, `~/.tact/hooks.json`, …). One of the two is required:
/// approving something by accident is the failure this command exists to avoid.
///
/// Trust is read when hooks are registered, so a running session keeps the
/// decisions it started with.
pub fn trust_hooks(work_dir: &Path, all: bool, source: Option<&str>) -> Result<Vec<HookSummary>> {
    if !all && source.is_none() {
        anyhow::bail!("pass --all, or --source <label> to approve one source");
    }
    let report = survey_hooks(work_dir)?;
    let selected: Vec<HookSummary> = report
        .pending
        .into_iter()
        .filter(|hook| all || source.is_some_and(|wanted| hook.source == wanted))
        .collect();
    if selected.is_empty() {
        return Ok(selected);
    }
    let mut store = HookTrust::load();
    store.trust(&selected)?;
    Ok(selected)
}

/// Forgets every review decision, so every hook needs review again.
pub fn forget_hook_trust() -> Result<()> {
    HookTrust::load().forget_all()
}

/// Registers every reviewed hook from every source on the agent.
///
/// Fail-closed: a hook whose definition is not in the review store is not
/// registered at all, so a repository that ships `.tact/hooks.json` cannot
/// execute anything by being cloned — the file is read, and then refused.
fn apply_hook_sources(
    sources: Vec<HookSource>,
    trust: &HookTrust,
    agent: crate::Agent,
    work_dir: &Path,
) -> (crate::Agent, HookLoadReport) {
    let mut agent = agent;
    let work_dir = work_dir.to_path_buf();
    let mut report = HookLoadReport::default();

    for source in sources {
        let hooks = admit_trusted(&source, trust, &mut report);
        for (matcher, command) in hooks.commands_for(HookEventKind::SessionStart) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_session_start(
                move |agent: &crate::Agent, context: &mut SessionStartContext| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    Box::pin(async move {
                        // The Claude `source` matcher vocabulary
                        // (startup|resume|clear|compact); Tact reports the real
                        // one, so a plugin's `startup|resume|compact` pattern
                        // fires for the case it meant.
                        let source = agent.runtime.session_start_source.as_str();
                        if !matcher_matches(matcher.as_deref(), source) {
                            return Ok(HookControl::Continue);
                        }
                        // The plugin's own status line, surfaced rather than
                        // logged: this hook is a subprocess that can take
                        // seconds (a cold `uv run --script` took ~100s here),
                        // and silence while the first turn waits reads as a
                        // hang.
                        if let Some(status) = command.status_message.as_deref() {
                            agent.emit_update(AgentUpdate::Info(status.to_string()));
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "SessionStart",
                                // `model` and `permission_mode` are required by
                                // Codex's `session-start.command.input` schema;
                                // a plugin that branches on them would otherwise
                                // read `null` and degrade.
                                event: json!({ "source": source }),
                            },
                            Some(agent),
                        )
                        .await;
                        collect_session_start_output(&output, context);
                        // Codex's `SessionStart` schema has no `decision`; the
                        // stop is `continue: false` with an optional reason.
                        Ok(if output.stop {
                            HookControl::Block(output.stop_reason.clone().unwrap_or_else(|| {
                                "stopped by the plugin's SessionStart hook".to_string()
                            }))
                        } else {
                            HookControl::Continue
                        })
                    })
                },
            );
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::UserPromptSubmit) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent =
                agent.with_user_prompt_submit(move |agent: &crate::Agent, prompt: &mut String| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    let prompt_snapshot = prompt.clone();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), &prompt_snapshot) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "UserPromptSubmit",
                                event: json!({ "prompt": prompt_snapshot }),
                            },
                            Some(agent),
                        )
                        .await;
                        if let Some(extra) = output.additional_context {
                            prompt.push_str(&extra);
                        }
                        Ok(output.control)
                    })
                });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::PreToolUse) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_pre_tool(move |agent: &crate::Agent, tool_use: &mut ToolUse| {
                let matcher = matcher.clone();
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                let tool_name = tool_use.name.clone();
                let tool_input = tool_use.input.clone();
                let tool_use_id = tool_use.id.clone();
                Box::pin(async move {
                    if !matcher_matches(matcher.as_deref(), &tool_name) {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "PreToolUse",
                            event: json!({
                                "tool_name": tool_name,
                                "tool_input": tool_input,
                                "tool_use_id": tool_use_id,
                            }),
                        },
                        Some(agent),
                    )
                    .await;
                    if let Some(extra) = output.additional_context {
                        // Injected as conversation context before the next
                        // request, the way Codex records it. The old
                        // `tool_use.input["_hook_context"]` key was written but
                        // never read, so the context was lost — and it left an
                        // extra field in the arguments the permission check and
                        // the tool itself see.
                        agent
                            .runtime
                            .pending_hook_context
                            .lock_recover()
                            .push_back(extra);
                    }
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::PermissionRequest) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_permission_request(
                move |agent: &crate::Agent, tool_use: &mut ToolUse| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), &tool_use.name) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "PermissionRequest",
                                event: json!({
                                    "tool_name": tool_use.name,
                                    "tool_input": tool_use.input,
                                }),
                            },
                            Some(agent),
                        )
                        .await;
                        // `allow` skips the prompt, `block` denies with the
                        // reason, anything else leaves the user to decide.
                        Ok(output.control)
                    })
                },
            );
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::PostToolUse) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_post_tool_hook(
                move |agent: &crate::Agent,
                      tool_use: &ToolUse,
                      result: &mut ToolResult,
                      _status| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    let tool_name = tool_use.name.clone();
                    let tool_input = tool_use.input.clone();
                    let tool_use_id = tool_use.id.clone();
                    let tool_response = result.content.clone();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), &tool_name) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "PostToolUse",
                                event: json!({
                                    "tool_name": tool_name,
                                    "tool_input": tool_input,
                                    "tool_use_id": tool_use_id,
                                    "tool_response": tool_response,
                                }),
                            },
                            Some(agent),
                        )
                        .await;
                        if output.suppress_output {
                            result.content.clear();
                        }
                        if let Some(extra) = output.additional_context {
                            // Same route as `PreToolUse`: collected here, turned
                            // into conversation context before the next request.
                            agent
                                .runtime
                                .pending_hook_context
                                .lock_recover()
                                .push_back(extra);
                        }
                        Ok(output.control)
                    })
                },
            );
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::PostToolUseFailure) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_post_tool_failure(
                move |agent: &crate::Agent, tool_use: &ToolUse, error: &str| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    let tool_name = tool_use.name.clone();
                    let tool_input = tool_use.input.clone();
                    let tool_use_id = tool_use.id.clone();
                    let error = error.to_string();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), &tool_name) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "PostToolUseFailure",
                                event: json!({
                                    "tool_name": tool_name,
                                    "tool_input": tool_input,
                                    "tool_use_id": tool_use_id,
                                    "error": error,
                                    "is_interrupt": false,
                                }),
                            },
                            Some(agent),
                        )
                        .await;
                        // Observational: the tool already failed.
                        let _ = output;
                        Ok(HookControl::Continue)
                    })
                },
            );
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::Notification) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent =
                agent.with_notification(move |agent: &crate::Agent, ctx: &NotificationContext| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    let notification_type = ctx.notification_type.clone();
                    let title = ctx.title.clone();
                    let message = ctx.message.clone();
                    Box::pin(async move {
                        // Claude Code matches Notification against the
                        // notification type (`permission_prompt`).
                        if !matcher_matches(matcher.as_deref(), &notification_type) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "Notification",
                                event: json!({
                                    "notification_type": notification_type,
                                    "title": title,
                                    "message": message,
                                }),
                            },
                            Some(agent),
                        )
                        .await;
                        // Observational.
                        let _ = output;
                        Ok(HookControl::Continue)
                    })
                });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::TaskCompleted) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_task_completed(move |agent: &crate::Agent| {
                let matcher = matcher.clone();
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                let task_description = agent.last_assistant_message().unwrap_or_default();
                Box::pin(async move {
                    // TaskCompleted has no matcher in Claude Code; an explicit
                    // matcher is ignored (matching everything) for parity.
                    if !matcher_matches(matcher.as_deref(), "") {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "TaskCompleted",
                            event: json!({
                                "task_description": task_description,
                            }),
                        },
                        Some(agent),
                    )
                    .await;
                    // Observational.
                    let _ = output;
                    Ok(HookControl::Continue)
                })
            });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::Interrupt) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_interrupt(move |agent: &crate::Agent| {
                let matcher = matcher.clone();
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    // Codex's `Interrupt` has no matcher subject, so a matcher
                    // can only be a constant expression; `interrupt` is what it
                    // is matched against.
                    if !matcher_matches(matcher.as_deref(), "interrupt") {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "Interrupt",
                            event: json!({ "reason": "user_interrupt" }),
                        },
                        Some(agent),
                    )
                    .await;
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::Stop) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_stop(move |agent: &crate::Agent| {
                let matcher = matcher.clone();
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                let last = agent.last_assistant_message();
                Box::pin(async move {
                    // Stop has no matcher in Codex; an explicit matcher is
                    // ignored (matching everything) for parity.
                    if !matcher_matches(matcher.as_deref(), "") {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "Stop",
                            event: json!({
                                "stop_hook_active": false,
                                "last_assistant_message": last,
                            }),
                        },
                        Some(agent),
                    )
                    .await;
                    // `block` means "continue the turn with reason as the next
                    // prompt" (Codex continuation fragment); propagate it.
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::SessionEnd) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_session_end(move |agent: &crate::Agent| {
                let matcher = matcher.clone();
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    // Codex matches SessionEnd against `reason` ("other").
                    if !matcher_matches(matcher.as_deref(), "other") {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "SessionEnd",
                            event: json!({ "reason": "other" }),
                        },
                        Some(agent),
                    )
                    .await;
                    // Observational: a block cannot prevent teardown.
                    let _ = output;
                    Ok(HookControl::Continue)
                })
            });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::PreCompact) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent = agent.with_pre_compact(move |agent: &crate::Agent, trigger: CompactTrigger| {
                let matcher = matcher.clone();
                let command = command.clone();
                let dirs = dirs.clone();
                let work_dir = work_dir.clone();
                Box::pin(async move {
                    if !matcher_matches(matcher.as_deref(), trigger.as_str()) {
                        return Ok(HookControl::Continue);
                    }
                    let output = run_hook(
                        &command,
                        &dirs,
                        &HookRunInput {
                            session_id: String::new(),
                            work_dir,
                            hook_event_name: "PreCompact",
                            event: json!({ "trigger": trigger.as_str() }),
                        },
                        Some(agent),
                    )
                    .await;
                    // `block` vetoes the compaction (Codex `should_stop`).
                    Ok(output.control)
                })
            });
        }

        for (matcher, command) in hooks.commands_for(HookEventKind::PostCompact) {
            let matcher = matcher.matcher.clone();
            let command = command.clone();
            let dirs = source.dirs.clone();
            let work_dir = work_dir.clone();
            agent =
                agent.with_post_compact(move |agent: &crate::Agent, trigger: CompactTrigger| {
                    let matcher = matcher.clone();
                    let command = command.clone();
                    let dirs = dirs.clone();
                    let work_dir = work_dir.clone();
                    Box::pin(async move {
                        if !matcher_matches(matcher.as_deref(), trigger.as_str()) {
                            return Ok(HookControl::Continue);
                        }
                        let output = run_hook(
                            &command,
                            &dirs,
                            &HookRunInput {
                                session_id: String::new(),
                                work_dir,
                                hook_event_name: "PostCompact",
                                event: json!({ "trigger": trigger.as_str() }),
                            },
                            Some(agent),
                        )
                        .await;
                        // Observational: compaction already committed.
                        let _ = output;
                        Ok(HookControl::Continue)
                    })
                });
        }
    }

    (agent, report)
}

#[cfg(test)]
impl HooksFile {
    fn from_str(content: &str) -> Result<Self> {
        serde_json::from_str(content).context("failed to parse hooks file")
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_inline_manifest_hooks() {
        let manifest: HookManifest = serde_json::from_str(
            r#"{
                "hooks": {
                    "SessionStart": [{
                        "hooks": [{ "type": "command", "command": "echo hi" }]
                    }]
                }
            }"#,
        )
        .unwrap();

        let hooks = inline_hooks(&manifest).unwrap();

        assert_eq!(hooks.commands_for(HookEventKind::SessionStart).len(), 1);
    }

    #[test]
    fn empty_inline_manifest_hooks_yield_no_commands() {
        let manifest: HookManifest = serde_json::from_str(r#"{"hooks":{}}"#).unwrap();

        let hooks = inline_hooks(&manifest).unwrap();

        assert!(hooks.commands_for(HookEventKind::SessionStart).is_empty());
    }

    #[test]
    fn string_manifest_hooks_are_not_inline() {
        let manifest: HookManifest =
            serde_json::from_str(r#"{"hooks":"./hooks/hooks.json"}"#).unwrap();

        assert!(inline_hooks(&manifest).is_none());
    }

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
    fn parses_new_hook_event_kinds() {
        assert_eq!(
            HookEventKind::parse("PostToolUseFailure"),
            Some(HookEventKind::PostToolUseFailure)
        );
        assert_eq!(
            HookEventKind::parse("Notification"),
            Some(HookEventKind::Notification)
        );
        assert_eq!(
            HookEventKind::parse("TaskCompleted"),
            Some(HookEventKind::TaskCompleted)
        );
        assert_eq!(HookEventKind::TaskCompleted.as_str(), "TaskCompleted");
        assert_eq!(HookEventKind::parse("Nope"), None);
    }

    /// Codex treats plain `SessionStart` stdout as model context and
    /// invalid-JSON stdout as a failure; Tact must draw the same line, because
    /// the reference `basic-memory` plugin prints its whole briefing as plain
    /// text.
    #[test]
    fn start_hook_stdout_is_context_unless_it_looks_like_json() {
        // Plain Markdown — the `basic-memory` briefing shape.
        let brief = parse_output(
            "# Basic Memory\n\n- resume from checkpoint 7\n",
            "SessionStart",
        );
        assert_eq!(
            brief.additional_context.as_deref(),
            Some("# Basic Memory\n\n- resume from checkpoint 7")
        );
        assert!(matches!(brief.control, HookControl::Continue));

        // JSON that does not parse is a failure: injecting it would put the
        // hook's own source into the conversation.
        let broken = parse_output("{\"hookSpecificOutput\":", "SessionStart");
        assert!(broken.additional_context.is_none());
        assert!(matches!(broken.control, HookControl::Continue));

        // Blank output carries nothing.
        assert!(
            parse_output("  \n", "SessionStart")
                .additional_context
                .is_none()
        );

        // `UserPromptSubmit` shares the plain-text fallback.
        assert_eq!(
            parse_output("extra context", "UserPromptSubmit")
                .additional_context
                .as_deref(),
            Some("extra context")
        );

        // Every other event keeps failing closed on non-JSON.
        assert!(
            parse_output("oops", "PreToolUse")
                .additional_context
                .is_none()
        );
    }

    /// Codex's top-level fields: `continue` / `stopReason` / `systemMessage`.
    /// `continue: false` is what stops a `SessionStart` — that schema has no
    /// `decision`, so a plugin cannot block with one.
    #[test]
    fn continue_stop_reason_and_system_message_are_parsed() {
        let output = parse_output(
            r#"{"continue": false, "stopReason": "no context yet", "systemMessage": "warming up"}"#,
            "SessionStart",
        );
        assert!(output.stop);
        assert_eq!(output.stop_reason.as_deref(), Some("no context yet"));
        assert_eq!(output.system_message.as_deref(), Some("warming up"));

        // Absent means continue, which is the common case.
        let default = parse_output("{}", "SessionStart");
        assert!(!default.stop);
        assert!(default.stop_reason.is_none());
        assert!(default.system_message.is_none());
    }

    /// A `SessionStart` run's context reaches the agent. Regression guard: this
    /// routing used to be a `warn!`, so a plugin's briefing never reached the
    /// model.
    #[test]
    fn session_start_output_routes_its_context_to_the_agent() {
        let mut context = SessionStartContext::default();
        collect_session_start_output(
            &HookOutput {
                control: HookControl::Continue,
                additional_context: Some("graph: resume from checkpoint 7".to_string()),
                ..HookOutput::default()
            },
            &mut context,
        );
        assert_eq!(
            context.additional_contexts,
            vec!["graph: resume from checkpoint 7".to_string()]
        );

        // A blocking run still hands over the context it produced.
        let mut blocked = SessionStartContext::default();
        collect_session_start_output(
            &HookOutput {
                control: HookControl::Block("stop".to_string()),
                additional_context: Some("ctx".to_string()),
                ..HookOutput::default()
            },
            &mut blocked,
        );
        assert_eq!(blocked.additional_contexts, vec!["ctx".to_string()]);
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
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert!(output.additional_context.is_none());
    }

    /// End-to-end through the command runner, in the shape the reference
    /// `basic-memory` plugin uses (plain-text stdout): the briefing becomes
    /// session context instead of a dropped warning.
    #[tokio::test]
    async fn a_plugin_hook_command_reaches_the_session_start_context() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some("echo graph: resume from checkpoint 7".into()),
            command_windows: None,
            timeout: None,
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "SessionStart",
                event: json!({ "source": "startup" }),
            },
            None,
        )
        .await;

        let mut context = SessionStartContext::default();
        collect_session_start_output(&output, &mut context);
        assert_eq!(
            context.additional_contexts,
            vec!["graph: resume from checkpoint 7".to_string()]
        );
    }

    #[tokio::test]
    async fn run_command_hook_block_new_format() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(r#"printf '{"decision":"block","reason":"no read allowed"}'"#.into()),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
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
            None,
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
            additional_context_limit: None,
            ..Default::default()
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
            None,
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
            additional_context_limit: None,
            ..Default::default()
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
            None,
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
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    /// A hook that fails is fail-open (the turn continues) but no longer
    /// silent: the warning alone only reaches a log file, and `tact-ui` installs
    /// no subscriber unless `RUST_LOG` is set.
    #[tokio::test]
    async fn a_failing_hook_is_surfaced_to_the_ui() {
        crate::config::test_support::install_default();

        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some("echo 'stderr says why' >&2; exit 3".into()),
            command_windows: None,
            timeout: None,
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let agent = crate::Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(Vec::new())),
            crate::tool::test_support::test_context("failing_hook"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);

        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "SessionStart",
                event: json!({ "source": "startup" }),
            },
            Some(&agent),
        )
        .await;

        assert!(
            matches!(output.control, HookControl::Continue),
            "a hook failure stays fail-open"
        );
        let mut notice = None;
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::Info(text) = update {
                notice = Some(text);
            }
        }
        let notice = notice.expect("the failure reaches the UI");
        assert!(notice.contains("SessionStart"), "{notice}");
        assert!(notice.contains("stderr says why"), "{notice}");
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
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn run_command_hook_timeout_zero_disables_timeout() {
        let dir = tempdir().unwrap();
        // Explicit timeout 0 must NOT apply a 60s default: the command takes
        // 2s and must complete (a 1s accidental timeout would kill it).
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some("sleep 2; printf %s '{\"decision\":\"approve\"}'".into()),
            command_windows: None,
            timeout: Some(0),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn run_command_hook_suppress_output_new_format() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(r#"printf %s '{"decision":"approve","suppressOutput":true}'"#.into()),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "PostToolUse",
                event: json!({ "tool_name": "Bash" }),
            },
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert!(
            output.suppress_output,
            "new-format suppressOutput must parse"
        );
    }

    #[tokio::test]
    async fn run_command_hook_stdin_broken_pipe_still_honors_stdout() {
        // Regression: a fast hook that never reads stdin can exit before the
        // parent delivers the payload, so the stdin write ends in EPIPE.
        // Inflate the payload past the OS pipe buffer (64 KiB on Linux) so the
        // write is guaranteed to block and then break once the hook exits —
        // deterministic regardless of scheduling. The hook's stdout must still
        // be parsed: exit status and stdout are authoritative, stdin is a
        // best-effort delivery.
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(r#"printf %s '{"decision":"approve","suppressOutput":true}'"#.into()),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
        };
        let output = run_command_hook(
            &command,
            dir.path(),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "PostToolUse",
                event: json!({ "tool_name": "Bash", "pad": "x".repeat(200 * 1024) }),
            },
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert!(
            output.suppress_output,
            "hook stdout must be honored after a broken-pipe stdin write"
        );
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
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        let expected = dir.path().display().to_string();
        assert_eq!(
            output.additional_context.as_deref(),
            Some(expected.as_str())
        );
    }

    #[tokio::test]
    async fn run_command_hook_plain_text_stdout_is_additional_context() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(r#"printf %s 'Follow the house style.'"#.into()),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert_eq!(
            output.additional_context.as_deref(),
            Some("Follow the house style.")
        );
    }

    #[tokio::test]
    async fn run_command_hook_plain_text_ignored_for_pre_tool() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(r#"printf %s 'not json'"#.into()),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert!(output.additional_context.is_none());
    }

    #[test]
    fn build_payload_merges_event_fields_at_top_level() {
        let payload = build_payload(
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: PathBuf::from("/proj"),
                hook_event_name: "PreToolUse",
                event: json!({ "tool_name": "Bash", "tool_input": { "command": "ls" } }),
            },
            None,
        );
        assert_eq!(payload["session_id"], "s1");
        assert_eq!(payload["hook_event_name"], "PreToolUse");
        assert_eq!(payload["tool_name"], "Bash");
        assert_eq!(payload["tool_input"]["command"], "ls");
        assert!(payload.get("_event").is_none(), "event must not be nested");
    }

    /// Codex's `session-start.command.input` requires `model` and
    /// `permission_mode`; a plugin that branches on them must not read `null`.
    /// The plugin's own `statusMessage` also has to reach the UI, because the
    /// hook can take seconds to answer.
    #[tokio::test]
    async fn session_start_payload_carries_the_model_and_permission_mode() {
        use std::collections::BTreeMap;

        use crate::plugin::InstalledPlugin;

        crate::config::test_support::install_default();

        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let plugin_root = plugin_home.cache.join("acme/demo/abc123");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin_root.join("hooks")).unwrap();
        std::fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            r#"{ "name": "demo" }"#,
        )
        .unwrap();
        // `cat > file` keeps the payload the hook received.
        let captured = home.path().join("payload.json");
        std::fs::write(
            plugin_root.join("hooks/hooks.json"),
            format!(
                r#"{{
                    "hooks": {{
                        "SessionStart": [{{
                            "hooks": [{{ "type": "command",
                                        "statusMessage": "Loading demo context",
                                        "command": "cat > {captured}" }}]
                        }}]
                    }}
                }}"#,
                captured = captured.display()
            ),
        )
        .unwrap();
        PluginStore::new(plugin_home.clone())
            .commit_install(
                &crate::plugin::InstalledState {
                    plugins: BTreeMap::from([(
                        "acme/demo".to_owned(),
                        InstalledPlugin {
                            id: "demo".to_owned(),
                            marketplace: "acme".to_owned(),
                            revision: "abc123".to_owned(),
                            cache_path: plugin_root.clone(),
                            skill_count: 0,
                            command_count: 0,
                            has_hooks: true,
                            has_mcp: false,
                        },
                    )]),
                },
                &plugin_root,
            )
            .unwrap();

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mock = tact_llm::MockClient::new(vec![(
            vec![tact_llm::ContentBlock::Text { text: "ok".into() }],
            Some(tact_llm::StopReason::EndTurn),
        )]);
        let agent = crate::Agent::new(
            tact_llm::LlmProvider::Mock(mock),
            crate::tool::test_support::test_context("session_start_payload"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);
        let mut agent = apply_hooks_with(
            &plugin_home,
            &HookTrust::trusting_everything(),
            agent,
            home.path(),
        )
        .unwrap()
        .0;
        // The payload's `hook_event_name` carries the live session so a plugin
        // can correlate runs; callers used to pass an empty string.
        agent.runtime.session_id = Some("sess-1".to_string());

        agent
            .agent_loop(Some(tact_llm::Message::new_text(
                tact_llm::Role::User,
                "hi",
            )))
            .await
            .unwrap();

        let payload: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&captured).unwrap()).unwrap();
        assert_eq!(payload["hook_event_name"], "SessionStart");
        assert_eq!(payload["source"], "startup");
        assert_eq!(payload["session_id"], "sess-1");
        assert!(
            payload["transcript_path"].is_null(),
            "Tact has no single live transcript file, and a directory was worse \
             than null: {payload}"
        );
        assert_eq!(payload["turn_id"], "0");
        assert_eq!(
            payload["model"], "mock-model",
            "a plugin reads the model it is briefing: {payload}"
        );
        assert_eq!(
            payload["permission_mode"], "default",
            "Codex's required field, in the vocabulary plugins expect: {payload}"
        );

        let mut status = None;
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::Info(text) = update {
                status = Some(text);
            }
        }
        assert_eq!(
            status.as_deref(),
            Some("Loading demo context"),
            "the plugin's status line reaches the UI"
        );
    }

    /// `PreToolUse` / `PostToolUse` context is conversation context, not a field
    /// on the tool arguments. It used to land in `tool_use.input["_hook_context"]`,
    /// which nothing read — so the context was lost and the arguments were
    /// polluted for the permission check and for the tool itself.
    #[tokio::test]
    async fn tool_hook_context_is_collected_without_touching_the_arguments() -> anyhow::Result<()> {
        use std::collections::BTreeMap;

        use crate::plugin::InstalledPlugin;

        crate::config::test_support::install_default();

        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let plugin_root = plugin_home.cache.join("acme/demo/abc123");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin_root.join("hooks")).unwrap();
        std::fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            r#"{ "name": "demo" }"#,
        )
        .unwrap();
        std::fs::write(
            plugin_root.join("hooks/hooks.json"),
            r#"{
                "hooks": {
                    "PreToolUse": [{ "hooks": [{ "type": "command",
                        "command": "echo '{\"hookSpecificOutput\":{\"additionalContext\":\"pre context\"}}'" }] }],
                    "PostToolUse": [{ "hooks": [{ "type": "command",
                        "command": "echo '{\"hookSpecificOutput\":{\"additionalContext\":\"post context\"}}'" }] }]
                }
            }"#,
        )
        .unwrap();
        PluginStore::new(plugin_home.clone())
            .commit_install(
                &crate::plugin::InstalledState {
                    plugins: BTreeMap::from([(
                        "acme/demo".to_owned(),
                        InstalledPlugin {
                            id: "demo".to_owned(),
                            marketplace: "acme".to_owned(),
                            revision: "abc123".to_owned(),
                            cache_path: plugin_root.clone(),
                            skill_count: 0,
                            command_count: 0,
                            has_hooks: true,
                            has_mcp: false,
                        },
                    )]),
                },
                &plugin_root,
            )
            .unwrap();

        let agent = crate::Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(Vec::new())),
            crate::tool::test_support::test_context("tool_hook_context"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        );
        let agent = apply_hooks_with(
            &plugin_home,
            &HookTrust::trusting_everything(),
            agent,
            home.path(),
        )
        .unwrap()
        .0;

        let mut tool_use = ToolUse {
            id: "t1".into(),
            name: "Read".into(),
            input: serde_json::json!({ "file_path": "/tmp/x" }),
        };
        crate::invoke_hooks!(PreToolUse, &agent, &mut tool_use)?;
        assert!(
            tool_use.input.get("_hook_context").is_none(),
            "the arguments must reach the tool unchanged: {}",
            tool_use.input
        );

        let mut result = ToolResult {
            tool_use_id: "t1".into(),
            content: "body".into(),
        };
        crate::invoke_hooks!(
            PostToolUse,
            &agent,
            &tool_use,
            &mut result,
            tact_protocol::StepStatus::Success
        )?;

        let collected: Vec<String> = {
            let mut queue = agent.runtime.pending_hook_context.lock().unwrap();
            queue.drain(..).collect()
        };
        assert_eq!(
            collected,
            vec!["pre context".to_string(), "post context".to_string()],
            "both tool events hand their context to the next request"
        );
        Ok(())
    }

    #[test]
    fn resolve_timeout_semantics() {
        assert_eq!(resolve_timeout(None), Some(60), "default is 60s");
        assert_eq!(resolve_timeout(Some(5)), Some(5));
        assert_eq!(
            resolve_timeout(Some(0)),
            None,
            "explicit 0 disables timeout"
        );
    }

    /// The `(root, data)` pair the hook tests expand against. The two
    /// directories differ so a `PLUGIN_DATA` substitution cannot pass by
    /// accident.
    fn test_dirs() -> PluginDirs {
        PluginDirs {
            root: PathBuf::from("/cache/p"),
            data: PathBuf::from("/data/p"),
        }
    }

    #[test]
    fn expands_plugin_root_placeholder() {
        let dirs = test_dirs();
        // Braced form, Claude Code ABI.
        assert_eq!(
            expand_plugin_placeholders(r#"node "${CLAUDE_PLUGIN_ROOT}/hooks/x.js""#, &dirs),
            r#"node "/cache/p/hooks/x.js""#
        );
        // Braced form, Agent Plugins / Codex ABI — the shape the reference
        // `basic-memory` plugin ships.
        assert_eq!(
            expand_plugin_placeholders(
                r#"uv run --quiet --script "${PLUGIN_ROOT}/hooks/session_start.py""#,
                &dirs
            ),
            r#"uv run --quiet --script "/cache/p/hooks/session_start.py""#
        );
        // Bare form, both root ABIs.
        assert_eq!(
            expand_plugin_placeholders("$PLUGIN_ROOT/x", &dirs),
            "/cache/p/x"
        );
        assert_eq!(
            expand_plugin_placeholders("$CLAUDE_PLUGIN_ROOT/x", &dirs),
            "/cache/p/x"
        );
    }

    #[test]
    fn expands_the_plugin_data_placeholders() {
        let dirs = test_dirs();
        // §9.1: the data directory is its own placeholder in both ABIs, and it
        // must not resolve to the package root.
        assert_eq!(
            expand_plugin_placeholders(r#"sh "${PLUGIN_DATA}/run.sh""#, &dirs),
            r#"sh "/data/p/run.sh""#
        );
        assert_eq!(
            expand_plugin_placeholders(r#"sh "${CLAUDE_PLUGIN_DATA}/run.sh""#, &dirs),
            r#"sh "/data/p/run.sh""#
        );
        assert_eq!(
            expand_plugin_placeholders("$PLUGIN_DATA/x", &dirs),
            "/data/p/x"
        );
    }

    #[test]
    fn a_longer_variable_name_is_not_a_placeholder() {
        let dirs = test_dirs();
        // `$PLUGIN_ROOT_DATA` names one variable; the shell reads it that way,
        // so tact must not rewrite the prefix.
        assert_eq!(
            expand_plugin_placeholders("${PLUGIN_ROOT_DATA}/x", &dirs),
            "${PLUGIN_ROOT_DATA}/x"
        );
        assert_eq!(
            expand_plugin_placeholders("$PLUGIN_ROOT_DATA/x", &dirs),
            "$PLUGIN_ROOT_DATA/x"
        );
    }

    #[tokio::test]
    async fn a_hook_reads_the_plugin_data_env() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(
                r#"printf '{"decision":"approve","additionalContext":"%s"}' "$(printenv PLUGIN_DATA)""#
                    .into(),
            ),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
        };
        let output = run_command_hook(
            &command,
            PluginDirs::root_only(dir.path()),
            &HookRunInput {
                session_id: "s1".into(),
                work_dir: dir.path().to_path_buf(),
                hook_event_name: "UserPromptSubmit",
                event: Value::Null,
            },
            None,
        )
        .await;

        let expected = dir.path().display().to_string();
        assert_eq!(
            output.additional_context.as_deref(),
            Some(expected.as_str())
        );
    }

    /// The reference `basic-memory` plugin runs
    /// `uv run --quiet --script "${PLUGIN_ROOT}/hooks/session_start.py"`, so a
    /// hook written that way has to find its own script through the Codex
    /// placeholder and actually execute.
    #[tokio::test]
    async fn a_codex_style_hook_command_resolves_its_script() {
        let dir = tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("hooks")).unwrap();
        std::fs::write(dir.path().join("hooks/probe.sh"), "printf probe-ran").unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(r#"sh "${PLUGIN_ROOT}/hooks/probe.sh""#.into()),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        assert_eq!(output.additional_context.as_deref(), Some("probe-ran"));
    }

    /// `printenv` keeps a `$PLUGIN_ROOT` placeholder out of the command string,
    /// so this isolates the exported env var from the placeholder rewrite.
    #[tokio::test]
    async fn a_codex_style_hook_reads_the_plugin_root_env() {
        let dir = tempdir().unwrap();
        let command = HookCommand {
            ty: Some("command".into()),
            command: Some(
                r#"printf '{"decision":"approve","additionalContext":"%s"}' "$(printenv PLUGIN_ROOT)""#
                    .into(),
            ),
            command_windows: None,
            timeout: Some(10),
            status_message: None,
            async_: None,
            additional_context_limit: None,
            ..Default::default()
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
            None,
        )
        .await;

        let expected = dir.path().display().to_string();
        assert_eq!(
            output.additional_context.as_deref(),
            Some(expected.as_str())
        );
    }

    #[test]
    fn plugin_subagent_start_hooks_loads_default_hooks_path() {
        // Claude Code default discovery: `hooks/hooks.json` even without a
        // manifest `hooks` field (several official plugins rely on this).
        use crate::plugin::InstalledPlugin;

        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let plugin_root = plugin_home.cache.join("acme/demo/abc123");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin_root.join("hooks")).unwrap();
        std::fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            r#"{ "name": "demo" }"#,
        )
        .unwrap();
        std::fs::write(
            plugin_root.join("hooks/hooks.json"),
            r#"{
                "hooks": {
                    "SubagentStart": [{
                        "hooks": [{ "type": "command",
                                    "command": "printf %s '{\"decision\":\"approve\",\"additionalContext\":\"inject\"}'" }]
                    }]
                }
            }"#,
        )
        .unwrap();
        let store = PluginStore::new(plugin_home.clone());
        store
            .commit_install(
                &crate::plugin::InstalledState {
                    plugins: std::collections::BTreeMap::from([(
                        "acme/demo".to_owned(),
                        InstalledPlugin {
                            id: "demo".to_owned(),
                            marketplace: "acme".to_owned(),
                            revision: "abc123".to_owned(),
                            cache_path: plugin_root.clone(),
                            skill_count: 0,
                            command_count: 0,
                            has_hooks: true,
                            has_mcp: false,
                        },
                    )]),
                },
                &plugin_root,
            )
            .unwrap();

        let hooks =
            subagent_start_hooks_with(&plugin_home, &HookTrust::trusting_everything(), home.path())
                .unwrap();
        assert_eq!(
            hooks.len(),
            1,
            "default hooks/hooks.json must be discovered without a manifest hooks field"
        );

        // Running the hook appends its plain-text stdout as context.
        let mut ctx = SubagentStartContext {
            name: "child-1".into(),
            prompt: "do the thing".into(),
            system_prompt: "base".into(),
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(hooks[0](&mut ctx))
            .unwrap();
        assert_eq!(result, HookControl::Continue);
        assert!(
            ctx.system_prompt.contains("inject"),
            "hook stdout should be appended to the system prompt"
        );
    }

    #[test]
    fn plugin_subagent_start_hooks_propagates_block() {
        use crate::plugin::InstalledPlugin;

        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let plugin_root = plugin_home.cache.join("acme/demo/abc123");
        std::fs::create_dir_all(plugin_root.join(".codex-plugin")).unwrap();
        std::fs::create_dir_all(plugin_root.join("hooks")).unwrap();
        std::fs::write(
            plugin_root.join(".codex-plugin/plugin.json"),
            r#"{ "name": "demo" }"#,
        )
        .unwrap();
        std::fs::write(
            plugin_root.join("hooks/hooks.json"),
            r#"{
                "hooks": {
                    "SubagentStart": [{
                        "hooks": [{ "type": "command",
                                    "command": "printf %s '{\"decision\":\"block\",\"reason\":\"no subagents\"}'" }]
                    }]
                }
            }"#,
        )
        .unwrap();
        let store = PluginStore::new(plugin_home.clone());
        store
            .commit_install(
                &crate::plugin::InstalledState {
                    plugins: std::collections::BTreeMap::from([(
                        "acme/demo".to_owned(),
                        InstalledPlugin {
                            id: "demo".to_owned(),
                            marketplace: "acme".to_owned(),
                            revision: "abc123".to_owned(),
                            cache_path: plugin_root.clone(),
                            skill_count: 0,
                            command_count: 0,
                            has_hooks: true,
                            has_mcp: false,
                        },
                    )]),
                },
                &plugin_root,
            )
            .unwrap();

        let hooks =
            subagent_start_hooks_with(&plugin_home, &HookTrust::trusting_everything(), home.path())
                .unwrap();
        let mut ctx = SubagentStartContext {
            name: "child-1".into(),
            prompt: "p".into(),
            system_prompt: "base".into(),
        };
        let result = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(hooks[0](&mut ctx))
            .unwrap();

        assert_eq!(
            result,
            HookControl::Block("no subagents".to_string()),
            "SubagentStart block must propagate to spawn_subagent"
        );
    }

    // ── Review and file sources ──────────────────────────────────────────

    /// A hooks file with one command per named event, for the review tests.
    fn one_command_hooks(event: &str, commands: &[&str]) -> HooksFile {
        let group = serde_json::json!({
            "matcher": "startup|resume|compact",
            "hooks": commands
                .iter()
                .map(|c| serde_json::json!({ "type": "command", "command": c }))
                .collect::<Vec<_>>(),
        });
        // An event maps to an array of matcher groups; the key is dynamic, so it
        // cannot be written inline in `json!`.
        let mut events = serde_json::Map::new();
        events.insert(event.to_string(), serde_json::json!([group]));
        serde_json::from_value(serde_json::json!({ "hooks": events })).unwrap()
    }

    fn source_with(label: &str, dir: &Path, hooks: HooksFile) -> HookSource {
        HookSource {
            label: label.to_string(),
            origin: HookOrigin::UserFile,
            dirs: PluginDirs::root_only(dir),
            hooks,
        }
    }

    #[test]
    fn a_hook_identity_covers_every_field_a_reviewer_sees() {
        let base = hook_definition_hash("~/.tact/hooks.json", "PreToolUse", None, "run.sh");

        // Same definition, same hash — the store would be useless otherwise.
        assert_eq!(
            base,
            hook_definition_hash("~/.tact/hooks.json", "PreToolUse", None, "run.sh")
        );
        // Every other field changes the identity, because every other field is
        // something the reviewer agreed to.
        assert_ne!(
            base,
            hook_definition_hash("~/.tact/hooks.json", "PreToolUse", None, "run-2.sh")
        );
        assert_ne!(
            base,
            hook_definition_hash("plugin x", "PreToolUse", None, "run.sh")
        );
        assert_ne!(
            base,
            hook_definition_hash("~/.tact/hooks.json", "PostToolUse", None, "run.sh")
        );
        assert_ne!(
            base,
            hook_definition_hash("~/.tact/hooks.json", "PreToolUse", Some("Bash"), "run.sh")
        );
        // The NUL separator: concatenation must not collide.
        assert_ne!(
            hook_definition_hash("a", "b", None, "c"),
            hook_definition_hash("ab", "", None, "c")
        );
    }

    #[test]
    fn an_unreviewed_hook_is_read_and_then_refused() {
        let dir = tempfile::tempdir().unwrap();
        let source = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            one_command_hooks("SessionStart", &["run.sh"]),
        );
        let trust = HookTrust::from_path(dir.path().join("hooks-state.json"));

        let mut report = HookLoadReport::default();
        let admitted = admit_trusted(&source, &trust, &mut report);

        // The file was parsed (the hook is visible)…
        assert_eq!(report.pending.len(), 1, "{report:?}");
        assert_eq!(report.pending[0].command, "run.sh");
        // …and nothing was admitted, so nothing can be registered or spawned.
        assert!(admitted.hooks.is_empty());
        assert!(report.trusted.is_empty());
    }

    #[test]
    fn approving_one_definition_admits_only_that_one_across_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("hooks-state.json");
        let source = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            one_command_hooks("SessionStart", &["approved.sh", "still-pending.sh"]),
        );

        // Review everything, then approve only the first pending entry.
        let mut trust = HookTrust::from_path(state.clone());
        let mut report = HookLoadReport::default();
        admit_trusted(&source, &trust, &mut report);
        assert_eq!(report.pending.len(), 2);
        let approved = trust.trust(&report.pending[..1]).unwrap();
        assert_eq!(approved, 1);

        // A fresh load of the store — what the next session does.
        let reloaded = HookTrust::from_path(state.clone());
        let mut after = HookLoadReport::default();
        let admitted = admit_trusted(&source, &reloaded, &mut after);

        let admitted_commands: Vec<&str> = admitted
            .commands_for(HookEventKind::SessionStart)
            .iter()
            .filter_map(|(_, command)| command.command.as_deref())
            .collect();
        assert_eq!(admitted_commands, ["approved.sh"]);
        assert_eq!(after.pending.len(), 1);
        assert_eq!(after.pending[0].command, "still-pending.sh");
        // The store is a file the user can inspect and delete.
        assert!(state.is_file());
    }

    #[test]
    fn forgetting_puts_every_hook_back_into_review() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("hooks-state.json");
        let source = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            one_command_hooks("Stop", &["run.sh"]),
        );

        let mut trust = HookTrust::from_path(state.clone());
        let mut report = HookLoadReport::default();
        admit_trusted(&source, &trust, &mut report);
        trust.trust(&report.pending).unwrap();

        let mut trusted = HookTrust::from_path(state.clone());
        assert!(trusted.is_trusted(&hook_definition_hash(
            "~/.tact/hooks.json",
            "Stop",
            Some("startup|resume|compact"),
            "run.sh"
        )));

        trusted.forget_all().unwrap();
        let emptied = HookTrust::from_path(state);
        let mut report = HookLoadReport::default();
        assert!(
            admit_trusted(&source, &emptied, &mut report)
                .hooks
                .is_empty()
        );
        assert_eq!(report.pending.len(), 1);
    }

    #[test]
    fn an_unparseable_store_reviews_everything_again() {
        // Fail-closed: a store we cannot read must not be read as "trusted".
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("hooks-state.json");
        std::fs::write(&state, "{ not json").unwrap();

        let trust = HookTrust::from_path(state);
        assert!(!trust.is_trusted("anything"));
    }

    #[test]
    fn both_tact_hooks_files_are_sources_in_a_documented_order() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("user-hooks.json");
        std::fs::write(&user, r#"{"hooks":{"Stop":[{"hooks":[]}]}}"#).unwrap();
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            dir.path().join(".tact/hooks.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"repo.sh"}]}]}}"#,
        )
        .unwrap();

        let sources = collect_hook_sources_with(None, Some(user.clone()), dir.path()).unwrap();

        let labels: Vec<&str> = sources.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        assert_eq!(labels[0], "~/.tact/hooks.json");
        assert!(labels[1].ends_with(".tact/hooks.json"), "{labels:?}");
        assert_eq!(sources[1].origin, HookOrigin::ProjectFile);
        // The project file's `${PLUGIN_ROOT}` is its own directory.
        assert_eq!(sources[1].dirs.root, dir.path().join(".tact"));
    }

    #[test]
    fn a_repository_hook_file_that_cannot_be_parsed_is_skipped() {
        // A cloned repo must not be able to stop Tact from starting.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(dir.path().join(".tact/hooks.json"), "{ not json").unwrap();

        let sources = collect_hook_sources_with(None, None, dir.path()).unwrap();

        assert!(sources.is_empty(), "{:?}", sources.len());
    }

    #[test]
    fn the_load_report_says_how_to_review_and_stays_quiet_when_nothing_pends() {
        let quiet = HookLoadReport {
            trusted: vec![HookSummary {
                hash: "h".into(),
                source: "~/.tact/hooks.json".into(),
                event: "Stop".into(),
                matcher: None,
                command: "run.sh".into(),
            }],
            pending: Vec::new(),
        };
        assert!(quiet.is_quiet());
        assert!(quiet.notice_lines().is_empty());

        let loud = HookLoadReport {
            trusted: Vec::new(),
            pending: vec![HookSummary {
                hash: "h".into(),
                source: "plugin ponytail".into(),
                event: "SessionStart".into(),
                matcher: Some("startup".into()),
                command: "brief.sh".into(),
            }],
        };
        let lines = loud.notice_lines();
        assert!(!loud.is_quiet());
        assert!(lines[0].contains("1 hook(s) need review"), "{lines:?}");
        assert!(lines[1].contains("plugin ponytail"), "{lines:?}");
        assert!(lines[1].contains("brief.sh"), "{lines:?}");
        assert!(lines[2].contains("hooks trust --all"), "{lines:?}");
    }

    // ── exit 2: the simple block contract ────────────────────────────────

    fn hook_command(script: &str) -> HookCommand {
        HookCommand {
            ty: Some("command".into()),
            command: Some(script.into()),
            timeout: Some(30),
            ..Default::default()
        }
    }

    fn hook_input(dir: &Path, event: &'static str) -> HookRunInput {
        HookRunInput {
            session_id: "s1".into(),
            work_dir: dir.to_path_buf(),
            hook_event_name: event,
            event: json!({ "tool_name": "bash" }),
        }
    }

    #[tokio::test]
    async fn exit_two_with_a_reason_blocks_a_tool_call() {
        let dir = tempdir().unwrap();
        let command = hook_command("echo 'rm -rf is blocked' >&2; exit 2");

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "PreToolUse"),
            None,
        )
        .await;

        assert_eq!(
            output.control,
            HookControl::Block("rm -rf is blocked".to_string()),
            "stderr is the reason the caller shows"
        );
    }

    #[tokio::test]
    async fn exit_two_without_a_reason_does_not_block() {
        // Nothing to tell the user, so blocking would be an unexplained refusal.
        let dir = tempdir().unwrap();
        let command = hook_command("exit 2");

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "PreToolUse"),
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn exit_two_stays_fail_open_where_codex_gives_it_no_meaning() {
        // `SessionStart` has `continue: false`, not a block decision; a hook that
        // exits 2 there must not be able to refuse the session.
        let dir = tempdir().unwrap();
        let command = hook_command("echo 'not a veto' >&2; exit 2");

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "SessionStart"),
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn a_json_decision_outranks_a_bare_exit_two() {
        let dir = tempdir().unwrap();
        let command = hook_command(
            "printf '{\"decision\":\"block\",\"reason\":\"json reason\"}'; \
             echo 'stderr reason' >&2; exit 2",
        );

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "PreToolUse"),
            None,
        )
        .await;

        assert_eq!(
            output.control,
            HookControl::Block("json reason".to_string()),
            "the richer contract wins over the exit status"
        );
    }

    #[tokio::test]
    async fn exit_two_still_blocks_when_stdout_only_adds_context() {
        // `additionalContext` is an addition, not a decision: it must survive,
        // and the stderr reason must still block.
        let dir = tempdir().unwrap();
        let command = hook_command(
            "printf '{\"hookSpecificOutput\":{\"hookEventName\":\"PreToolUse\",\
             \"additionalContext\":\"a note\"}}'; \
             echo 'stderr reason' >&2; exit 2",
        );

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "PreToolUse"),
            None,
        )
        .await;

        assert_eq!(
            output.control,
            HookControl::Block("stderr reason".to_string())
        );
        assert_eq!(output.additional_context.as_deref(), Some("a note"));
    }

    #[tokio::test]
    async fn a_non_two_failure_is_reported_and_continues() {
        let dir = tempdir().unwrap();
        let command = hook_command("echo 'something broke' >&2; exit 3");

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "PreToolUse"),
            None,
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
    }

    #[tokio::test]
    async fn a_hook_context_limit_truncates_what_reaches_the_model() {
        let dir = tempdir().unwrap();
        let long = "x".repeat(4_000);
        let mut command = hook_command(&format!(
            "printf '{{\"hookSpecificOutput\":{{\"hookEventName\":\"SessionStart\",\
             \"additionalContext\":\"{long}\"}}}}'"
        ));
        command.additional_context_limit = Some(10);

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "SessionStart"),
            None,
        )
        .await;

        let context = output.additional_context.expect("context is kept");
        assert!(
            context.contains("truncated to 10 tokens by additionalContextLimit"),
            "{context}"
        );
        assert!(context.len() < long.len(), "the budget must bite");
    }

    #[tokio::test]
    async fn a_context_inside_its_limit_is_left_exactly_as_written() {
        let dir = tempdir().unwrap();
        let mut command = hook_command(
            "printf '{\"hookSpecificOutput\":{\"hookEventName\":\"SessionStart\",\
             \"additionalContext\":\"short note\"}}'",
        );
        command.additional_context_limit = Some(1_000);

        let output = run_command_hook(
            &command,
            dir.path(),
            &hook_input(dir.path(), "SessionStart"),
            None,
        )
        .await;

        assert_eq!(output.additional_context.as_deref(), Some("short note"));
    }

    #[test]
    fn a_permission_decision_allow_parses_as_an_allow() {
        let allow = parse_output(
            r#"{"hookSpecificOutput":{"hookEventName":"PermissionRequest","permissionDecision":"allow"}}"#,
            "PermissionRequest",
        );
        assert_eq!(allow.control, HookControl::Allow);

        // And a deny is still a deny, with its reason.
        let deny = parse_output(
            r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"no"}}"#,
            "PreToolUse",
        );
        assert_eq!(deny.control, HookControl::Block("no".to_string()));
    }

    /// A `PermissionRequest` hook registered from a file can answer the prompt.
    #[tokio::test]
    async fn a_permission_request_hook_is_registered_and_can_allow() {
        crate::config::test_support::install_default();

        let dir = tempdir().unwrap();
        let user = dir.path().join("hooks.json");
        std::fs::write(
            &user,
            r#"{"hooks":{"PermissionRequest":[{"matcher":"bash","hooks":[
                {"type":"command","command":"printf '{\"hookSpecificOutput\":{\"hookEventName\":\"PermissionRequest\",\"permissionDecision\":\"allow\"}}'"}
            ]}]}}"#,
        )
        .unwrap();

        let sources = collect_hook_sources_with(None, Some(user), dir.path()).unwrap();
        let agent = crate::Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(Vec::new())),
            crate::tool::test_support::test_context("permission_request"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        );
        let (agent, report) = apply_hook_sources(
            sources,
            &HookTrust::trusting_everything(),
            agent,
            dir.path(),
        );
        assert!(report.pending.is_empty(), "{report:?}");

        let hooks = agent.hooks_by_type(crate::hook::HookTypes::PermissionRequest);
        assert_eq!(hooks.len(), 1, "the event must be registered");
        let crate::hook::Hook::PermissionRequest(hook) = hooks[0] else {
            panic!("registered under the wrong event")
        };

        // The matcher is not `bash`, so the hook stays out of the way.
        let mut other = ToolUse {
            id: "t0".into(),
            name: "read_file".into(),
            input: json!({ "path": "a" }),
        };
        assert_eq!(
            hook(&agent, &mut other).await.unwrap(),
            HookControl::Continue
        );

        // A matching call gets an answer instead of a prompt.
        let mut bash = ToolUse {
            id: "t1".into(),
            name: "bash".into(),
            input: json!({ "command": "ls" }),
        };
        assert_eq!(hook(&agent, &mut bash).await.unwrap(), HookControl::Allow);
    }

    /// An `Interrupt` hook runs once per turn, and its message is surfaced.
    #[tokio::test]
    async fn an_interrupt_hook_is_registered_and_runs_once_per_turn() {
        crate::config::test_support::install_default();

        let dir = tempdir().unwrap();
        let user = dir.path().join("hooks.json");
        std::fs::write(
            &user,
            r#"{"hooks":{"Interrupt":[{"hooks":[
                {"type":"command","command":"printf '{\"systemMessage\":\"interrupted, flushing\"}'"}
            ]}]}}"#,
        )
        .unwrap();

        let sources = collect_hook_sources_with(None, Some(user), dir.path()).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let agent = crate::Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(Vec::new())),
            crate::tool::test_support::test_context("interrupt"),
            crate::tool::toolset(),
            crate::mcp::MCPToolRouter::new(),
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        )
        .with_ui_channel(tx);
        let (mut agent, _) = apply_hook_sources(
            sources,
            &HookTrust::trusting_everything(),
            agent,
            dir.path(),
        );

        assert_eq!(
            agent.hooks_by_type(crate::hook::HookTypes::Interrupt).len(),
            1
        );

        agent.dispatch_interrupt_hooks().await.unwrap();
        // The second call is the same turn: the hooks must not run again.
        agent.dispatch_interrupt_hooks().await.unwrap();

        let mut messages = Vec::new();
        while let Ok(update) = rx.try_recv() {
            if let AgentUpdate::Info(text) = update {
                messages.push(text);
            }
        }
        assert_eq!(
            messages
                .iter()
                .filter(|m| m.contains("interrupted, flushing"))
                .count(),
            1,
            "one notice per turn, got {messages:?}"
        );
    }

    #[test]
    fn only_the_events_codex_documents_react_to_exit_two() {
        for event in [
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "Stop",
            "SubagentStop",
            "UserPromptSubmit",
        ] {
            assert!(exit_two_blocks(event), "{event} should block");
        }
        for event in ["SessionStart", "PreCompact", "PostCompact", "SessionEnd"] {
            assert!(!exit_two_blocks(event), "{event} must stay fail-open");
        }
    }
    // ── `type: "mcp_tool"`: a hook that lives in an MCP server ──────────

    /// An agent wired to one mock MCP server whose tool replies with `reply`,
    /// plus the service handle so a test can read the calls it received.
    fn agent_with_an_mcp_tool(
        name: &str,
        reply: Value,
    ) -> (crate::Agent, Arc<crate::mcp::MockMcpService>) {
        let service = Arc::new(crate::mcp::MockMcpService::new(Vec::new(), move |_| {
            Ok(rmcp::model::CallToolResult::success(vec![
                rmcp::model::Content::text(reply.to_string()),
            ]))
        }));
        let mut router = crate::mcp::MCPToolRouter::new();
        router.register_client(crate::mcp::McpClient::with_service(
            "policy",
            Vec::new(),
            service.clone(),
        ));
        let agent = crate::Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(Vec::new())),
            crate::tool::test_support::test_context(name),
            crate::tool::toolset(),
            router,
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        );
        (agent, service)
    }

    fn mcp_tool_hook(server: &str, tool: &str) -> HookCommand {
        HookCommand {
            ty: Some("mcp_tool".into()),
            server: Some(server.into()),
            tool: Some(tool.into()),
            ..Default::default()
        }
    }

    /// An agent whose MCP tool always fails, for the failure contract.
    fn agent_with_a_broken_mcp_tool(name: &str) -> crate::Agent {
        let service = Arc::new(crate::mcp::MockMcpService::new(Vec::new(), |_| {
            Err(rmcp::service::ServiceError::McpError(
                rmcp::model::ErrorData::internal_error(
                    "the policy server is on fire".to_string(),
                    None,
                ),
            ))
        }));
        let mut router = crate::mcp::MCPToolRouter::new();
        router.register_client(crate::mcp::McpClient::with_service(
            "policy",
            Vec::new(),
            service,
        ));
        crate::Agent::new(
            tact_llm::LlmProvider::Mock(tact_llm::MockClient::new(Vec::new())),
            crate::tool::test_support::test_context(name),
            crate::tool::toolset(),
            router,
            crate::permission::PermissionManager::try_new(
                crate::permission::PermissionMode::Default,
            )
            .unwrap(),
            crate::AgentSystemPrompt::Static("test".to_string()),
        )
    }

    #[tokio::test]
    async fn an_mcp_tool_hook_calls_the_servers_tool_with_its_arguments() {
        let dir = tempdir().unwrap();
        let (agent, service) = agent_with_an_mcp_tool(
            "mcp_tool_hook_calls",
            json!({"decision": "block", "reason": "not on my watch"}),
        );
        let mut command = mcp_tool_hook("policy", "gate");
        command.arguments = Some(json!({ "path": "secret.txt" }));

        let output = run_hook(
            &command,
            PluginDirs::root_only(dir.path()),
            &hook_input(dir.path(), "PreToolUse"),
            Some(&agent),
        )
        .await;

        // The tool's JSON decision is honoured exactly as a command hook's
        // stdout would be — that reuse is the whole point of the handler.
        assert_eq!(
            output.control,
            HookControl::Block("not on my watch".to_string())
        );
        // And the reviewed definition's arguments arrive intact.
        let calls = service.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        assert_eq!(calls[0].0, "gate");
        assert_eq!(calls[0].1, json!({ "path": "secret.txt" }));
    }

    #[tokio::test]
    async fn an_mcp_tool_hook_reaches_the_conversation_with_its_context() {
        let dir = tempdir().unwrap();
        let (agent, _service) = agent_with_an_mcp_tool(
            "mcp_tool_hook_context",
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": "the policy server says hello"
                }
            }),
        );
        let command = mcp_tool_hook("policy", "brief");

        let output = run_hook(
            &command,
            PluginDirs::root_only(dir.path()),
            &hook_input(dir.path(), "UserPromptSubmit"),
            Some(&agent),
        )
        .await;

        assert_eq!(
            output.additional_context.as_deref(),
            Some("the policy server says hello")
        );
    }

    #[tokio::test]
    async fn an_mcp_tool_hooks_context_is_bounded_by_its_own_limit() {
        // `additionalContextLimit` is the author's own ceiling, and it must apply
        // to this kind too — one contract, two ways to produce the output.
        let dir = tempdir().unwrap();
        let long = "word ".repeat(2_000);
        let (agent, _service) = agent_with_an_mcp_tool(
            "mcp_tool_hook_limit",
            json!({
                "hookSpecificOutput": {
                    "hookEventName": "UserPromptSubmit",
                    "additionalContext": long
                }
            }),
        );
        let mut command = mcp_tool_hook("policy", "brief");
        command.additional_context_limit = Some(10);

        let output = run_hook(
            &command,
            PluginDirs::root_only(dir.path()),
            &hook_input(dir.path(), "UserPromptSubmit"),
            Some(&agent),
        )
        .await;

        let context = output.additional_context.expect("context survived");
        assert!(
            context.contains("truncated to 10 tokens by additionalContextLimit"),
            "{context}"
        );
    }

    #[tokio::test]
    async fn a_broken_mcp_tool_hook_reports_and_continues() {
        // A hook must not be able to halt the loop by being broken — the same
        // contract every command hook failure follows.
        let dir = tempdir().unwrap();
        let agent = agent_with_a_broken_mcp_tool("mcp_tool_hook_broken");
        let command = mcp_tool_hook("policy", "gate");

        let output = run_hook(
            &command,
            PluginDirs::root_only(dir.path()),
            &hook_input(dir.path(), "PreToolUse"),
            Some(&agent),
        )
        .await;

        assert_eq!(output.control, HookControl::Continue);
        assert!(output.additional_context.is_none());
    }

    #[tokio::test]
    async fn an_mcp_tool_hook_without_a_server_or_tool_is_named_not_ignored() {
        // The failure this whole kind exists to fix: an entry the user reviewed
        // and approved must not be silently inert.
        let dir = tempdir().unwrap();
        let agent = agent_with_a_broken_mcp_tool("mcp_tool_hook_invalid");

        for command in [
            HookCommand {
                ty: Some("mcp_tool".into()),
                ..Default::default()
            },
            HookCommand {
                ty: Some("mcp_tool".into()),
                server: Some("policy".into()),
                ..Default::default()
            },
        ] {
            assert!(matches!(command.kind(), HookKind::Invalid(_)));
            let output = run_hook(
                &command,
                PluginDirs::root_only(dir.path()),
                &hook_input(dir.path(), "PreToolUse"),
                Some(&agent),
            )
            .await;
            assert_eq!(output.control, HookControl::Continue);
        }
    }

    #[test]
    fn a_hook_type_tact_does_not_know_is_still_a_command() {
        // Deliberately permissive: an absent or unrecognised `type` has always
        // meant "a command with a `command` string", and turning that into an
        // error would break a working configuration to punish a typo.
        for ty in [None, Some("command".to_string()), Some("somethingElse".to_string())] {
            let command = HookCommand {
                ty,
                command: Some("echo hi".into()),
                ..Default::default()
            };
            assert_eq!(command.kind(), HookKind::Command);
        }
        // Both spellings of the one type Tact does add.
        assert!(matches!(
            HookCommand {
                ty: Some("mcpTool".into()),
                server: Some("s".into()),
                tool: Some("t".into()),
                ..Default::default()
            }
            .kind(),
            HookKind::McpTool { .. }
        ));
    }
    // ── `mcp_tool` entries are identified by what they run ──────────────

    /// A hooks file whose one matcher group holds the given `mcp_tool` entries.
    fn mcp_tool_hooks(event: &str, entries: &[(&str, &str, serde_json::Value)]) -> HooksFile {
        let group = serde_json::json!({
            "hooks": entries
                .iter()
                .map(|(server, tool, arguments)| serde_json::json!({
                    "type": "mcp_tool",
                    "server": server,
                    "tool": tool,
                    "arguments": arguments,
                }))
                .collect::<Vec<_>>(),
        });
        let mut events = serde_json::Map::new();
        events.insert(event.to_string(), serde_json::json!([group]));
        serde_json::from_value(serde_json::json!({ "hooks": events })).unwrap()
    }

    #[test]
    fn two_mcp_tool_entries_are_not_the_same_definition() {
        // Before the identity covered the tool call, every `mcp_tool` entry in
        // one source hashed on an empty command string: approving one would
        // have approved the rest.
        let dir = tempfile::tempdir().unwrap();
        let source = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            mcp_tool_hooks(
                "PreToolUse",
                &[
                    ("policy", "gate", json!({})),
                    ("policy", "audit", json!({})),
                ],
            ),
        );
        let trust = HookTrust::from_path(dir.path().join("hooks-state.json"));
        let mut report = HookLoadReport::default();
        admit_trusted(&source, &trust, &mut report);

        assert_eq!(report.pending.len(), 2);
        assert_ne!(report.pending[0].hash, report.pending[1].hash);
        // And the reviewer is shown the call, not a blank line.
        assert_eq!(report.pending[0].command, "mcp_tool policy/gate {}");
        assert_eq!(report.pending[1].command, "mcp_tool policy/audit {}");
    }

    #[test]
    fn approving_one_mcp_tool_entry_admits_only_that_one() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("hooks-state.json");
        let source = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            mcp_tool_hooks(
                "PreToolUse",
                &[("policy", "gate", json!({})), ("policy", "audit", json!({}))],
            ),
        );

        let mut trust = HookTrust::from_path(state.clone());
        let mut report = HookLoadReport::default();
        admit_trusted(&source, &trust, &mut report);
        assert_eq!(trust.trust(&report.pending[..1]).unwrap(), 1);

        let reloaded = HookTrust::from_path(state);
        let mut after = HookLoadReport::default();
        let admitted = admit_trusted(&source, &reloaded, &mut after);

        let tools: Vec<&str> = admitted
            .commands_for(HookEventKind::PreToolUse)
            .iter()
            .filter_map(|(_, command)| command.tool.as_deref())
            .collect();
        assert_eq!(tools, ["gate"]);
        assert_eq!(after.pending.len(), 1);
        assert_eq!(after.pending[0].command, "mcp_tool policy/audit {}");
    }

    #[test]
    fn editing_an_mcp_tool_hooks_arguments_invalidates_its_approval() {
        // The arguments are what the tool will actually be asked to do, so
        // changing them is changing the definition.
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("hooks-state.json");
        let original = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            mcp_tool_hooks("PreToolUse", &[("policy", "gate", json!({ "path": "a.txt" }))]),
        );
        let mut trust = HookTrust::from_path(state.clone());
        let mut report = HookLoadReport::default();
        admit_trusted(&original, &trust, &mut report);
        trust.trust(&report.pending).unwrap();

        // Same server and tool, different input.
        let edited = source_with(
            "~/.tact/hooks.json",
            dir.path(),
            mcp_tool_hooks("PreToolUse", &[("policy", "gate", json!({ "path": "b.txt" }))]),
        );
        let reloaded = HookTrust::from_path(state);
        let mut after = HookLoadReport::default();
        let admitted = admit_trusted(&edited, &reloaded, &mut after);

        assert_eq!(after.pending.len(), 1, "the edit must need a fresh review");
        assert!(
            admitted.commands_for(HookEventKind::PreToolUse).is_empty(),
            "an edited definition must not inherit the old approval"
        );
        assert_eq!(admitted.hooks.len(), 0);
    }
    // ── `[hooks]` in `config.toml` ──────────────────────────────────────

    /// Writes a `config.toml` carrying a `[hooks]` table plus an unrelated
    /// table, to prove only the former is read.
    fn write_config_with_hooks(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("config.toml");
        let content = format!(
            "[llm]\nmodel = \"unrelated\"\n\n{body}"
        );
        std::fs::write(&path, content).unwrap();
        path
    }

    #[tokio::test]
    async fn a_hooks_table_in_config_toml_is_a_source_of_its_own() {
        let dir = tempfile::tempdir().unwrap();
        write_config_with_hooks(
            dir.path(),
            "[[hooks.PreToolUse]]\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"gate.sh\"\n",
        );

        let sources = collect_hook_sources_with(None, None, dir.path()).unwrap();
        let config_sources: Vec<&HookSource> = sources
            .iter()
            .filter(|source| source.label.ends_with("config.toml"))
            .collect();
        let labels: Vec<&str> = sources.iter().map(|s| s.label.as_str()).collect();
        assert_eq!(config_sources.len(), 1, "{labels:?}");
        // The review names the file, so `hooks trust --source` can address it.
        assert_eq!(
            config_sources[0].label,
            dir.path().join("config.toml").display().to_string()
        );
        assert_eq!(config_sources[0].origin, HookOrigin::ProjectFile);

        // Fail-closed, like every other source: read, then refused.
        let trust = HookTrust::from_path(dir.path().join("hooks-state.json"));
        let mut report = HookLoadReport::default();
        let admitted = admit_trusted(config_sources[0], &trust, &mut report);
        assert_eq!(report.pending.len(), 1);
        assert_eq!(report.pending[0].command, "gate.sh");
        assert!(admitted.hooks.is_empty(), "nothing runs before review");
    }

    #[test]
    fn a_config_toml_without_a_hooks_table_contributes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.toml"), "[llm]\nmodel = \"x\"\n").unwrap();

        let sources = collect_hook_sources_with(None, None, dir.path()).unwrap();
        assert!(
            sources.iter().all(|source| !source.label.ends_with("config.toml")),
            "an empty table would be a source `hooks list` has to explain: {:?}",
            sources.iter().map(|s| s.label.as_str()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn two_config_files_stay_two_sources() {
        // Merging them the way the config loader merges its values would make
        // "approve this file's hooks" impossible and would show the user one
        // anonymous list.
        let dir = tempfile::tempdir().unwrap();
        write_config_with_hooks(
            dir.path(),
            "[[hooks.PreToolUse]]\n[[hooks.PreToolUse.hooks]]\ntype = \"command\"\ncommand = \"root.sh\"\n",
        );
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            dir.path().join(".tact").join("config.toml"),
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"tact-dir.sh\"\n",
        )
        .unwrap();

        let sources = collect_hook_sources_with(None, None, dir.path()).unwrap();
        let labels: Vec<&str> = sources
            .iter()
            .map(|source| source.label.as_str())
            .filter(|label| label.ends_with("config.toml"))
            .collect();
        assert_eq!(labels.len(), 2, "{labels:?}");
        // Ascending specificity: the workdir file, then `.tact/`.
        assert!(labels[0].ends_with("config.toml") && !labels[0].contains(".tact"));
        assert!(labels[1].contains(".tact"));
    }

    #[test]
    fn an_mcp_tool_entry_can_be_declared_in_toml() {
        // The TOML and JSON spellings share one type, so the handler kind — and
        // its identity — cannot differ between them.
        let dir = tempfile::tempdir().unwrap();
        write_config_with_hooks(
            dir.path(),
            "[[hooks.PreToolUse]]\n[[hooks.PreToolUse.hooks]]\ntype = \"mcp_tool\"\nserver = \"policy\"\ntool = \"gate\"\n\n[hooks.PreToolUse.hooks.arguments]\npath = \"a.txt\"\n",
        );

        let sources = collect_hook_sources_with(None, None, dir.path()).unwrap();
        let source = sources
            .iter()
            .find(|source| source.label.ends_with("config.toml"))
            .expect("the table became a source");

        let trust = HookTrust::from_path(dir.path().join("hooks-state.json"));
        let mut report = HookLoadReport::default();
        admit_trusted(source, &trust, &mut report);
        assert_eq!(report.pending.len(), 1);
        assert_eq!(report.pending[0].command, "mcp_tool policy/gate {\"path\":\"a.txt\"}");
    }

    #[test]
    fn the_users_config_toml_is_the_one_beside_its_hooks_json() {
        // The user-scope pair is addressed as one value, so a caller that
        // injects a user hooks file also decides the user config — and a caller
        // passing neither gets no user-scope hooks at all, which is what keeps
        // the suite independent of the machine's real `$HOME`.
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(
            home.join("config.toml"),
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"user-config.sh\"\n",
        )
        .unwrap();

        let sources =
            collect_hook_sources_with(None, Some(home.join("hooks.json")), dir.path()).unwrap();
        let user: Vec<&HookSource> = sources
            .iter()
            .filter(|source| source.origin == HookOrigin::UserFile)
            .collect();
        assert_eq!(user.len(), 1, "{:?}", sources.len());
        assert_eq!(user[0].label, home.join("config.toml").display().to_string());
        // `${PLUGIN_ROOT}` and `${PLUGIN_DATA}` are the same directory for a
        // user-scope file, exactly as they are for `~/.tact/hooks.json`.
        assert_eq!(user[0].dirs.root, home);
        assert_eq!(user[0].dirs.data, home);

        // And with no user file injected, nothing user-scope is read.
        let isolated = collect_hook_sources_with(None, None, dir.path()).unwrap();
        assert!(
            isolated.iter().all(|source| source.origin != HookOrigin::UserFile),
            "a caller that injects no user file must read no user config"
        );
    }

    #[test]
    fn config_hook_sources_come_after_the_hooks_json_ones() {
        // The order is the only thing a new source can disturb (SessionStart
        // context is concatenated in it), so appending is the whole rule.
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("home");
        std::fs::create_dir_all(&user).unwrap();
        let user_hooks = user.join("hooks.json");
        std::fs::write(
            &user_hooks,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"user.sh"}]}]}}"#,
        )
        .unwrap();
        write_config_with_hooks(
            dir.path(),
            "[[hooks.Stop]]\n[[hooks.Stop.hooks]]\ntype = \"command\"\ncommand = \"config.sh\"\n",
        );
        std::fs::create_dir_all(dir.path().join(".tact")).unwrap();
        std::fs::write(
            dir.path().join(".tact").join("hooks.json"),
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"project.sh"}]}]}}"#,
        )
        .unwrap();

        let sources = collect_hook_sources_with(None, Some(user_hooks), dir.path()).unwrap();
        let order: Vec<&str> = sources
            .iter()
            .map(|source| source.label.as_str())
            .map(|label| {
                if label.ends_with("config.toml") {
                    "config"
                } else {
                    "hooks.json"
                }
            })
            .collect();
        assert_eq!(order, ["hooks.json", "hooks.json", "config"]);
    }
}
