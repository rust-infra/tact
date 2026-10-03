//! Typed native tool metadata: policies, presentation, effects, and structured results.
//!
//! Each native tool exposes one static `ToolMetadata` with closed-form policies.
//! `ToolRouter` binds metadata to the handler; agent scheduling, permission, and
//! TUI rendering consume typed policies carried by `ResolvedTool`, `ToolCallResult`,
//! and protocol `ToolPresentationInfo`.

use std::path::Path;
use std::path::PathBuf;

use serde_json::Value;
use tact_protocol::{ToolDetailKind, ToolPopupKind, ToolPresentationInfo, ToolVisualKind};

use crate::agent::tool_schedule::ToolResources;
use crate::permission::CapabilityRisk;

// ---------------------------------------------------------------------------
// Metadata struct
// ---------------------------------------------------------------------------

/// Static metadata for a native tool, declared as a constant beside the handler.
pub struct ToolMetadata {
    pub name: &'static str,
    pub description: &'static str,
    pub permission: PermissionPolicy,
    pub permission_prompt: PermissionPromptPolicy,
    pub resources: ResourcePolicy,
    pub domain: ToolDomain,
    pub presentation: ToolPresentation,
    pub output: OutputPolicy,
    pub argument_summary: ArgumentSummaryPolicy,
}

// ---------------------------------------------------------------------------
// Permission policies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    /// A read with no path target (metadata-only tools).
    Read,
    /// A write with no path target.
    Write,
    High,
    /// A read whose **target** decides the risk: `.env` inside the workspace is
    /// not the same proposition as `src/main.rs`.
    ReadPath {
        path_field: &'static str,
    },
    /// A write whose target decides the risk — a `*.pem` is not a `*.rs`.
    WritePath {
        path_field: &'static str,
    },
    /// A patch whose headers name the targets.
    PatchPaths,
    ShellCommand {
        command_field: &'static str,
    },
}

impl PermissionPolicy {
    /// Classify a concrete JSON input into a capability risk level.
    ///
    /// Target-aware variants consult the built-in registry in
    /// [`crate::security::sensitive`] and report [`CapabilityRisk::High`] for a
    /// sensitive target, which is what makes the target reachable by plan mode
    /// (blocks it), `Auto` mode (allows it) and the settings rules as usual.
    ///
    /// This is the *built-in* answer. Dispatch uses
    /// [`Self::sensitive`] against a configured
    /// [`Scanner`](crate::security::sensitive::Scanner), whose `extra` patterns
    /// are a superset of this; a caller that only wants the declared risk can
    /// keep using this method.
    pub fn resolve(&self, input: &Value) -> CapabilityRisk {
        match self {
            PermissionPolicy::Read => CapabilityRisk::Read,
            PermissionPolicy::Write => CapabilityRisk::Write,
            PermissionPolicy::High => CapabilityRisk::High,
            PermissionPolicy::ReadPath { path_field } => {
                if path_target_is_sensitive(input, path_field) {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Read
                }
            }
            PermissionPolicy::WritePath { path_field } => {
                if path_target_is_sensitive(input, path_field) {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Write
                }
            }
            PermissionPolicy::PatchPaths => {
                if patch_targets_are_sensitive(input) {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Write
                }
            }
            PermissionPolicy::ShellCommand { command_field } => {
                let cmd = input
                    .get(*command_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // The sensitive scan runs **first**: `cat ~/.ssh/id_ed25519` is
                // provably read-only *on the file*, which is exactly why the
                // read-only classifier must not be the last word on it. The
                // file's contents are the secret.
                if crate::security::sensitive::classify_command(cmd).is_some() {
                    return CapabilityRisk::High;
                }
                if cmd.starts_with("sudo ") || cmd.starts_with("su ") {
                    CapabilityRisk::High
                } else if super::readonly_shell::is_read_only_shell_command(cmd) {
                    // Provably read-only commands (ls, grep, cat, git status,
                    // …) are auto-allowed — this is what lets plan mode run
                    // inspection commands. Anything ambiguous stays Write.
                    CapabilityRisk::Read
                } else {
                    CapabilityRisk::Write
                }
            }
        }
    }

    /// The raw string this policy's decision is about: the path, the patch's
    /// first target, or the command.
    ///
    /// Used for display and for `redaction.basic_only_paths`, which has to be
    /// answerable even when there is no sensitive hit — a `level: "credential"`
    /// setting applies to every call, fixtures included.
    #[must_use]
    pub fn target(&self, input: &Value) -> Option<String> {
        match self {
            PermissionPolicy::ReadPath { path_field }
            | PermissionPolicy::WritePath { path_field } => input
                .get(*path_field)
                .and_then(|v| v.as_str())
                .map(str::to_string),
            PermissionPolicy::PatchPaths => patch_targets(input).into_iter().next(),
            PermissionPolicy::ShellCommand { command_field } => input
                .get(*command_field)
                .and_then(|v| v.as_str())
                .map(str::to_string),
            PermissionPolicy::Read | PermissionPolicy::Write | PermissionPolicy::High => None,
        }
    }

    /// The sensitive-path guard for this policy's target, if any.
    ///
    /// [`Scanner`](crate::security::sensitive::Scanner) carries the user's
    /// `enabled` / `extra` / `allow` overlay, so this is the configured form of
    /// the same question [`Self::resolve`] answers with built-ins only. A
    /// [`Tier::Credential`](crate::security::sensitive::Tier::Credential) hit is
    /// refused before modes and rules; a
    /// [`Tier::Secret`](crate::security::sensitive::Tier::Secret) hit becomes
    /// `High` and follows the ordinary ladder.
    #[must_use]
    pub fn sensitive(
        &self,
        input: &Value,
        scanner: &crate::security::sensitive::Scanner,
    ) -> Option<crate::security::sensitive::Hit> {
        match self {
            PermissionPolicy::ReadPath { .. } | PermissionPolicy::WritePath { .. } => {
                let raw = self.target(input)?;
                if !target_is_workspace_relative(&raw) {
                    return None;
                }
                scanner.classify(&raw)
            }
            PermissionPolicy::PatchPaths => patch_targets(input)
                .into_iter()
                .find_map(|path| scanner.classify(&path)),
            PermissionPolicy::ShellCommand { .. } => {
                let command = self.target(input)?;
                scanner.classify_command(&command)
            }
            PermissionPolicy::Read | PermissionPolicy::Write | PermissionPolicy::High => None,
        }
    }
}

/// Whether the file tools could even reach this path.
///
/// They resolve `path` against the workspace and refuse anything that escapes
/// it (`tool::safe_path`), and a leading `~` is not expanded — it names a
/// literal `~` directory. Gating such a path would only interrupt the user with
/// a prompt before an inevitable error, so the guard skips them. Home-relative
/// secrets are `bash`'s problem, and `bash` has no workspace boundary to skip.
fn target_is_workspace_relative(raw: &str) -> bool {
    let raw = raw.trim();
    !(raw.starts_with('/') || raw.starts_with('~'))
}

fn path_target_is_sensitive(input: &Value, path_field: &str) -> bool {
    input
        .get(path_field)
        .and_then(|v| v.as_str())
        .is_some_and(|raw| {
            target_is_workspace_relative(raw)
                && crate::security::sensitive::classify_path(raw).is_some()
        })
}

fn patch_targets(input: &Value) -> Vec<String> {
    input
        .get("patch")
        .and_then(|v| v.as_str())
        .map(extract_patch_paths)
        .unwrap_or_default()
}

fn patch_targets_are_sensitive(input: &Value) -> bool {
    patch_targets(input)
        .iter()
        .any(|path| crate::security::sensitive::classify_path(path).is_some())
}

/// The target paths a unified diff names — the same extraction the permission
/// policy and the "always allow" rule generator use, so the prompt, the rule
/// and the guard can never disagree about which files a patch touches.
#[must_use]
pub fn patch_target_paths(patch: &str) -> Vec<String> {
    extract_patch_paths(patch)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptPolicy {
    Json,
    Question {
        field: &'static str,
    },
    Command {
        field: &'static str,
    },
    Path {
        field: &'static str,
    },
    /// A patch: the prompt shows its target path(s), and an "always allow"
    /// persists a rule keyed on the patch text rather than the bare tool.
    ///
    /// `apply_patch` used to declare `Path { field: "path" }`, a field its
    /// input does not have, so `PermissionRule::generate` fell back to a bare
    /// rule: one click permitted every future patch. This variant exists so
    /// that cannot happen — see [`PermissionPromptPolicy::generate_key`].
    PatchTarget {
        patch_field: &'static str,
    },
}

impl PermissionPromptPolicy {
    /// The `(field, value)` an "always allow" rule should be built from, or
    /// `None` when no input-aware rule is possible.
    ///
    /// `None` is a deliberate refusal, not a fallback: the caller must not
    /// degrade to a bare tool-wide rule, because a bare rule is broader than
    /// the prompt the user answered.
    #[must_use]
    pub fn generate_key(&self, input: &Value) -> Option<(&'static str, String)> {
        match self {
            PermissionPromptPolicy::Command { field }
            | PermissionPromptPolicy::Question { field }
            | PermissionPromptPolicy::Path { field } => input
                .get(*field)
                .and_then(|v| v.as_str())
                .map(|v| (*field, v.to_string())),
            PermissionPromptPolicy::PatchTarget { patch_field } => {
                let patch = input.get(*patch_field).and_then(|v| v.as_str())?;
                let paths = extract_patch_paths(patch);
                // Anchor the rule on the patch text, which is what actually
                // round-trips, but only when the patch names exactly one file:
                // a multi-file patch keyed on one of its paths would silently
                // authorise the others too.
                if paths.len() != 1 {
                    return None;
                }
                Some((*patch_field, format!("*{}*", paths[0])))
            }
            PermissionPromptPolicy::Json => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Resource policies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourcePolicy {
    Independent,
    Barrier,
    ReadPath {
        field: &'static str,
    },
    WritePath {
        field: &'static str,
    },
    SharedState {
        scope: &'static str,
    },
    PatchFiles {
        patch_field: &'static str,
        dry_run_field: &'static str,
    },
}

impl ResourcePolicy {
    /// Resolve concrete tool input → `ToolResources` for wave scheduling.
    pub fn resolve(&self, input: &Value, work_dir: &Path) -> ToolResources {
        match self {
            ResourcePolicy::Independent => ToolResources::independent(),
            ResourcePolicy::Barrier => ToolResources::barrier(),
            ResourcePolicy::ReadPath { field } => {
                let mut r = ToolResources::independent();
                if let Some(p) = input.get(*field).and_then(|v| v.as_str()) {
                    r.reads.push(work_dir.join(p));
                }
                r
            }
            ResourcePolicy::WritePath { field } => {
                let mut r = ToolResources::independent();
                if let Some(p) = input.get(*field).and_then(|v| v.as_str()) {
                    r.writes.push(work_dir.join(p));
                }
                r
            }
            ResourcePolicy::SharedState { scope } => {
                // Shared state: synthetic write so same-scope tools serialize,
                // but they can still overlap with file reads (unlike barrier).
                let mut r = ToolResources::independent();
                r.writes.push(PathBuf::from(format!("__tact_{scope}__")));
                r
            }
            ResourcePolicy::PatchFiles {
                patch_field,
                dry_run_field,
            } => {
                let mut r = ToolResources::independent();
                if input
                    .get(*dry_run_field)
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                {
                    return r;
                }
                if let Some(patch) = input.get(*patch_field).and_then(|v| v.as_str()) {
                    for path in extract_patch_paths(patch) {
                        r.writes.push(work_dir.join(&path));
                    }
                }
                r
            }
        }
    }

    /// Extract recent file paths for display purposes.
    pub fn recent_paths(&self, input: &Value) -> Vec<String> {
        match self {
            ResourcePolicy::ReadPath { field } | ResourcePolicy::WritePath { field } => input
                .get(*field)
                .and_then(|v| v.as_str())
                .map(|p| vec![p.to_string()])
                .unwrap_or_default(),
            ResourcePolicy::PatchFiles { patch_field, .. } => input
                .get(*patch_field)
                .and_then(|v| v.as_str())
                .map(extract_patch_paths)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

/// Extract file paths from a unified diff patch (`+++ b/path` lines).
/// The target paths a unified diff names.
///
/// Handles both `+++ b/<path>` (git's default) and a bare `+++ <path>`, which
/// `apply_patch`'s own parser also accepts — a guard that only understood the
/// `b/` form would miss `+++ secrets.json` entirely. `/dev/null` is skipped: it
/// marks a deletion, not a file to touch.
fn extract_patch_paths(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| {
            let rest = line
                .strip_prefix("+++ b/")
                .or_else(|| line.strip_prefix("+++ "))?;
            // Strip trailing tab (grep-style) and whitespace.
            let path = rest.split('\t').next().unwrap_or(rest).trim();
            if path.is_empty() || path == "/dev/null" {
                return None;
            }
            Some(path.to_string())
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Domain & task operations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDomain {
    Generic,
    Task(TaskOperation),
    Subagent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskOperation {
    Create,
    Get,
    List,
    Update,
}

// ---------------------------------------------------------------------------
// Presentation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutputPolicy {
    Standard,
    FullTranscript,
    /// The tool returns immediately but its card stays live: streamed
    /// `ToolProgress` updates keep rendering until a
    /// `BackgroundTaskFinished`-style completion event finalizes the card.
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailPolicy {
    None,
    Result,
    InputField(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PopupPolicy {
    None,
    SubagentTranscript,
}

pub struct ToolPresentation {
    pub visual_kind: ToolVisualKind,
    pub display_name: &'static str,
    pub live_output: LiveOutputPolicy,
    pub detail: DetailPolicy,
    pub popup: PopupPolicy,
    pub compact_result_to_meta: bool,
}

impl ToolPresentation {
    pub fn to_protocol(self) -> ToolPresentationInfo {
        ToolPresentationInfo {
            visual_kind: self.visual_kind,
            display_name: self.display_name.to_string(),
            keep_full_live_output: matches!(self.live_output, LiveOutputPolicy::FullTranscript),
            keep_live: matches!(self.live_output, LiveOutputPolicy::Background),
            detail: match self.detail {
                DetailPolicy::None => ToolDetailKind::None,
                DetailPolicy::Result => ToolDetailKind::Result,
                DetailPolicy::InputField(field) => ToolDetailKind::InputField(field.to_string()),
            },
            popup: match self.popup {
                PopupPolicy::None => ToolPopupKind::None,
                PopupPolicy::SubagentTranscript => ToolPopupKind::SubagentTranscript,
            },
            compact_result_to_meta: self.compact_result_to_meta,
        }
    }
}

// ---------------------------------------------------------------------------
// Output & argument summary policies
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputPolicy {
    PersistLargeOutput,
    KeepInline,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArgumentSummaryPolicy {
    Json,
    Path { field: &'static str },
    Command { field: &'static str },
    Question { field: &'static str },
    SubagentPrompt { field: &'static str },
    // The one id a human reads — a background task id, a subagent child id.
    // An absent field yields "", so a call that omits the optional id renders as
    // the bare label (⏳ Wait Background) rather than an empty parameter. Prefer
    // this over `Json` whenever the tool has a single meaningful argument: a
    // serialized input object is a dump, and the renderer keeps dumps out of the
    // title.
    Id { field: &'static str },
    PatchPreview { patch_field: &'static str },
    ReadOffsetLimit { path_field: &'static str },
}

// ---------------------------------------------------------------------------
// Tool effects & structured results
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEffect {
    CompactHistory { focus: Option<String> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCallResult {
    pub content: String,
    pub effects: Vec<ToolEffect>,
    /// Optional image produced by the tool (e.g. `read_image`). When set, the
    /// tool-dispatch layer emits a companion `ContentBlock::Image` alongside
    /// the text `ToolResult`; the wire layer folds it into a following `user`
    /// message (Chat Completions `role:tool` cannot carry an image).
    #[allow(dead_code)]
    pub image: Option<tact_llm::ImageSource>,
}

impl ToolCallResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
            image: None,
        }
    }

    /// A tool result carrying a text envelope plus an image block.
    #[allow(dead_code)]
    pub fn text_image(content: impl Into<String>, image: tact_llm::ImageSource) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
            image: Some(image),
        }
    }
}

/// Trait for converting handler return values into `ToolCallResult`.
pub trait IntoToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult;
}

impl IntoToolCallResult for String {
    fn into_tool_call_result(self) -> ToolCallResult {
        ToolCallResult::text(self)
    }
}

impl IntoToolCallResult for &'static str {
    fn into_tool_call_result(self) -> ToolCallResult {
        ToolCallResult::text(self)
    }
}

impl IntoToolCallResult for ToolCallResult {
    fn into_tool_call_result(self) -> ToolCallResult {
        self
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn path_resource_policy_resolves_and_reports_recent_path() {
        let policy = ResourcePolicy::WritePath { field: "path" };
        let input = json!({ "path": "src/lib.rs" });
        let resources = policy.resolve(&input, Path::new("/repo"));

        assert_eq!(resources.writes, vec![Path::new("/repo/src/lib.rs")]);
        assert_eq!(policy.recent_paths(&input), vec!["src/lib.rs"]);
    }

    #[test]
    fn shell_permission_policy_classifies_readonly_as_read_and_others_as_write_or_high() {
        let policy = PermissionPolicy::ShellCommand {
            command_field: "command",
        };
        // Provably read-only commands (no shell metacharacters, safelisted
        // program) are classified as Read so plan mode can run inspection.
        assert_eq!(
            policy.resolve(&json!({"command": "ls -la"})),
            CapabilityRisk::Read
        );
        assert_eq!(
            policy.resolve(&json!({"command": "grep -rn needle ."})),
            CapabilityRisk::Read
        );
        assert_eq!(
            policy.resolve(&json!({"command": "git status"})),
            CapabilityRisk::Read
        );
        // Unknown programs, shell metacharacters, and unsafe options stay
        // Write because we cannot distinguish them from destructive commands.
        assert_eq!(
            policy.resolve(&json!({"command": "cargo test"})),
            CapabilityRisk::Write
        );
        assert_eq!(
            policy.resolve(&json!({"command": "ls | wc -l"})),
            CapabilityRisk::Write
        );
        assert_eq!(
            policy.resolve(&json!({"command": "find . -delete"})),
            CapabilityRisk::Write
        );
        // sudo / su commands are classified as High.
        assert_eq!(
            policy.resolve(&json!({"command": "sudo ls"})),
            CapabilityRisk::High
        );
        assert_eq!(
            policy.resolve(&json!({"command": "su root"})),
            CapabilityRisk::High
        );
    }

    #[test]
    fn adjacent_native_presentation_maps_to_protocol_without_name_matching() {
        let presentation = ToolPresentation {
            visual_kind: ToolVisualKind::Subagent,
            display_name: "🤖 Subagent",
            live_output: LiveOutputPolicy::FullTranscript,
            detail: DetailPolicy::Result,
            popup: PopupPolicy::SubagentTranscript,
            compact_result_to_meta: false,
        };
        let info = presentation.to_protocol();
        assert!(info.keep_full_live_output);
        assert!(!info.keep_live);
        assert_eq!(info.popup, ToolPopupKind::SubagentTranscript);
    }

    #[test]
    fn background_live_output_policy_keeps_card_live() {
        let presentation = ToolPresentation {
            visual_kind: ToolVisualKind::Command,
            display_name: "⚙️ Background Run",
            live_output: LiveOutputPolicy::Background,
            detail: DetailPolicy::Result,
            popup: PopupPolicy::None,
            compact_result_to_meta: false,
        };
        let info = presentation.to_protocol();
        assert!(info.keep_live);
        assert!(!info.keep_full_live_output);
    }

    #[test]
    fn string_converts_to_effect_free_tool_result() {
        let result = "ok".to_string().into_tool_call_result();
        assert_eq!(result.content, "ok");
        assert!(result.effects.is_empty());
    }
}
