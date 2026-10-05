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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

impl ToolMetadata {
    /// A read-only tool with no target path that takes its input as JSON.
    ///
    /// Eight of the native tools are exactly this and nothing about them needs
    /// to differ, so the shape is stated once instead of re-typed per tool.
    ///
    /// A tool that needs even one field different does not belong here: it
    /// spells out its own literal. A preset quietly grown an exception would be
    /// a description that is right for seven tools and wrong for the eighth.
    pub const fn read_json(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
    ) -> Self {
        Self::plain(
            name,
            description,
            display_name,
            PermissionPolicy::Read,
            ResourcePolicy::Independent,
        )
    }

    /// A tool that writes the `team` scope: the teammate/messaging family.
    ///
    /// They serialize among themselves through that shared scope while still
    /// overlapping file reads — see [`ResourcePolicy::SharedState`].
    pub const fn team_write(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
    ) -> Self {
        Self::plain(
            name,
            description,
            display_name,
            PermissionPolicy::Write,
            ResourcePolicy::SharedState { scope: "team" },
        )
    }

    /// A tool that must not run alongside any other: [`ResourcePolicy::Barrier`].
    pub const fn barrier_write(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
    ) -> Self {
        Self::plain(
            name,
            description,
            display_name,
            PermissionPolicy::Write,
            ResourcePolicy::Barrier,
        )
    }

    /// The domain, presentation, output and argument-summary every preset
    /// shares; only the permission and the resource claim vary between them.
    ///
    /// Private on purpose: a caller outside this module picking the remaining
    /// fields à la carte is the duplication these presets exist to remove.
    const fn plain(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
        permission: PermissionPolicy,
        resources: ResourcePolicy,
    ) -> Self {
        Self {
            name,
            description,
            permission,
            permission_prompt: PermissionPromptPolicy::Json,
            resources,
            domain: ToolDomain::Generic,
            presentation: ToolPresentation {
                visual_kind: ToolVisualKind::Generic,
                display_name,
                live_output: LiveOutputPolicy::Standard,
                detail: DetailPolicy::Result,
                popup: PopupPolicy::None,
                compact_result_to_meta: false,
            },
            output: OutputPolicy::KeepInline,
            argument_summary: ArgumentSummaryPolicy::Json,
        }
    }

    /// A tool whose target is one input field holding a path it only reads.
    ///
    /// The field name appears four times in the equivalent literal — the risk,
    /// the prompt an "always allow" is keyed on, the reservation the scheduler
    /// makes, and the card's title — and all four have to agree. Here they are
    /// one argument.
    pub const fn path_read(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
        path_field: &'static str,
    ) -> Self {
        Self {
            name,
            description,
            permission: PermissionPolicy::ReadPath { path_field },
            permission_prompt: PermissionPromptPolicy::Path { field: path_field },
            resources: ResourcePolicy::ReadPath { field: path_field },
            domain: ToolDomain::Generic,
            presentation: ToolPresentation {
                visual_kind: ToolVisualKind::FileRead,
                display_name,
                live_output: LiveOutputPolicy::Standard,
                detail: DetailPolicy::Result,
                popup: PopupPolicy::None,
                compact_result_to_meta: false,
            },
            output: OutputPolicy::KeepInline,
            argument_summary: ArgumentSummaryPolicy::Path { field: path_field },
        }
    }

    /// The task tools: one per [`TaskOperation`], which is the only thing that
    /// distinguishes them.
    ///
    /// Both take the `task` scope, so the reading one is a plain read while the
    /// writing one is a `SharedState` write: two task writes serialize through
    /// that scope without blocking file reads.
    pub const fn task_read(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
        operation: TaskOperation,
    ) -> Self {
        Self {
            name,
            description,
            permission: PermissionPolicy::Read,
            permission_prompt: PermissionPromptPolicy::Json,
            resources: ResourcePolicy::Independent,
            domain: ToolDomain::Task(operation),
            presentation: Self::task_presentation(display_name),
            output: OutputPolicy::KeepInline,
            argument_summary: ArgumentSummaryPolicy::Json,
        }
    }

    /// [`Self::task_read`]'s writing half.
    pub const fn task_write(
        name: &'static str,
        description: &'static str,
        display_name: &'static str,
        operation: TaskOperation,
    ) -> Self {
        Self {
            name,
            description,
            permission: PermissionPolicy::Write,
            permission_prompt: PermissionPromptPolicy::Json,
            resources: ResourcePolicy::SharedState { scope: "task" },
            domain: ToolDomain::Task(operation),
            presentation: Self::task_presentation(display_name),
            output: OutputPolicy::KeepInline,
            argument_summary: ArgumentSummaryPolicy::Json,
        }
    }

    const fn task_presentation(display_name: &'static str) -> ToolPresentation {
        ToolPresentation {
            visual_kind: ToolVisualKind::Task,
            display_name,
            live_output: LiveOutputPolicy::Standard,
            detail: DetailPolicy::Result,
            popup: PopupPolicy::None,
            compact_result_to_meta: false,
        }
    }
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The shared half of the presets, spelled out.
    ///
    /// Seventeen constants are built from these three constructors, so this is
    /// the one place a change to the common shape would show up: the compiler
    /// would accept a preset that quietly started naming a different display
    /// or a different output policy, and every tool using it would inherit it.
    #[test]
    fn the_shared_preset_shape_is_what_the_listing_tools_rely_on() {
        let metadata = ToolMetadata::read_json("name", "description", "Display");
        assert_eq!(metadata.name, "name");
        assert_eq!(metadata.description, "description");
        assert_eq!(metadata.permission_prompt, PermissionPromptPolicy::Json);
        assert_eq!(metadata.domain, ToolDomain::Generic);
        assert_eq!(metadata.output, OutputPolicy::KeepInline);
        assert_eq!(metadata.argument_summary, ArgumentSummaryPolicy::Json);
        assert_eq!(
            metadata.presentation,
            ToolPresentation {
                visual_kind: ToolVisualKind::Generic,
                display_name: "Display",
                live_output: LiveOutputPolicy::Standard,
                detail: DetailPolicy::Result,
                popup: PopupPolicy::None,
                compact_result_to_meta: false,
            }
        );
    }

    /// The three families are parameterized by *data* — which task operation,
    /// which input field — and by nothing else.
    ///
    /// That is the whole justification for a constructor here, so it is
    /// asserted rather than assumed. A member that grew a second difference
    /// would mean the constructor is hiding a decision instead of naming the
    /// one thing that varies, which is the line between this and the presets
    /// above: those serve tools that agree on everything, and a tool that
    /// differs in what it *does* keeps its own literal.
    #[test]
    fn the_families_differ_only_in_the_data_they_are_parameterized_by() {
        let get = ToolMetadata::task_read("n", "d", "P", TaskOperation::Get);
        let list = ToolMetadata::task_read("n", "d", "P", TaskOperation::List);
        assert_eq!(get.domain, ToolDomain::Task(TaskOperation::Get));
        assert_eq!(list.domain, ToolDomain::Task(TaskOperation::List));
        assert_eq!(
            ToolMetadata {
                domain: get.domain,
                ..list
            },
            get,
            "one task read differs from the other only in the operation"
        );

        let create = ToolMetadata::task_write("n", "d", "P", TaskOperation::Create);
        let update = ToolMetadata::task_write("n", "d", "P", TaskOperation::Update);
        assert_eq!(create.domain, ToolDomain::Task(TaskOperation::Create));
        assert_eq!(
            ToolMetadata {
                domain: create.domain,
                ..update
            },
            create
        );

        // One argument, four fields, all carrying the same name: the prompt
        // cannot ask about a path the scheduler did not reserve.
        for field in ["path", "file_path"] {
            let metadata = ToolMetadata::path_read("n", "d", "P", field);
            assert_eq!(
                metadata.permission,
                PermissionPolicy::ReadPath { path_field: field }
            );
            assert_eq!(
                metadata.permission_prompt,
                PermissionPromptPolicy::Path { field }
            );
            assert_eq!(metadata.resources, ResourcePolicy::ReadPath { field });
            assert_eq!(
                metadata.argument_summary,
                ArgumentSummaryPolicy::Path { field }
            );
        }
    }

    /// A task write takes the `task` scope; a task read does not.
    #[test]
    fn the_task_families_split_on_what_they_claim() {
        let read = ToolMetadata::task_read("n", "d", "P", TaskOperation::Get);
        let write = ToolMetadata::task_write("n", "d", "P", TaskOperation::Create);
        assert_eq!(
            (read.permission, read.resources),
            (PermissionPolicy::Read, ResourcePolicy::Independent)
        );
        assert_eq!(
            (write.permission, write.resources),
            (
                PermissionPolicy::Write,
                ResourcePolicy::SharedState { scope: "task" }
            )
        );
    }

    /// The presets differ in exactly two fields, and each names its own claim.
    ///
    /// Stated as a comparison rather than three field dumps: "these are the
    /// same tool with a different permission and resource claim" is the reason
    /// they are presets at all, and it is the property that would be lost if
    /// one of them grew a third difference.
    #[test]
    fn the_presets_differ_only_in_the_claim_they_make() {
        let read = ToolMetadata::read_json("n", "d", "P");
        let team = ToolMetadata::team_write("n", "d", "P");
        let barrier = ToolMetadata::barrier_write("n", "d", "P");

        assert_eq!(
            (read.permission, read.resources),
            (PermissionPolicy::Read, ResourcePolicy::Independent)
        );
        // The scope string is load-bearing: it is what serializes the
        // teammate tools against each other and nobody else.
        assert_eq!(
            (team.permission, team.resources),
            (
                PermissionPolicy::Write,
                ResourcePolicy::SharedState { scope: "team" }
            )
        );
        assert_eq!(
            (barrier.permission, barrier.resources),
            (PermissionPolicy::Write, ResourcePolicy::Barrier)
        );

        let baseline = ToolMetadata {
            permission: read.permission,
            resources: read.resources,
            ..read
        };
        for other in [team, barrier] {
            assert_eq!(
                ToolMetadata {
                    permission: read.permission,
                    resources: read.resources,
                    ..other
                },
                baseline
            );
        }
    }
}
