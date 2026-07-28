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
    Read,
    Write,
    High,
    ShellCommand { command_field: &'static str },
}

impl PermissionPolicy {
    /// Classify a concrete JSON input into a capability risk level.
    pub fn resolve(&self, input: &Value) -> CapabilityRisk {
        match self {
            PermissionPolicy::Read => CapabilityRisk::Read,
            PermissionPolicy::Write => CapabilityRisk::Write,
            PermissionPolicy::High => CapabilityRisk::High,
            PermissionPolicy::ShellCommand { command_field } => {
                let cmd = input
                    .get(*command_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if cmd.starts_with("sudo ") || cmd.starts_with("su ") {
                    CapabilityRisk::High
                } else {
                    CapabilityRisk::Write
                }
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptPolicy {
    Json,
    Question { field: &'static str },
    Command { field: &'static str },
    Path { field: &'static str },
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
fn extract_patch_paths(patch: &str) -> Vec<String> {
    patch
        .lines()
        .filter_map(|line| line.strip_prefix("+++ b/"))
        .map(|rest| {
            // Strip trailing tab (grep-style) and whitespace.
            rest.split('\t').next().unwrap_or(rest).trim().to_string()
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
}

impl ToolCallResult {
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            effects: Vec::new(),
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
    fn shell_permission_policy_classifies_common_cmd_as_write_and_sudo_as_high() {
        let policy = PermissionPolicy::ShellCommand {
            command_field: "command",
        };
        // Non-sudo commands are classified as Write because we cannot
        // distinguish read-only from destructive shell commands.
        assert_eq!(
            policy.resolve(&json!({"command": "git status"})),
            CapabilityRisk::Write
        );
        assert_eq!(
            policy.resolve(&json!({"command": "cargo test"})),
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
        assert_eq!(info.popup, ToolPopupKind::SubagentTranscript);
    }

    #[test]
    fn string_converts_to_effect_free_tool_result() {
        let result = "ok".to_string().into_tool_call_result();
        assert_eq!(result.content, "ok");
        assert!(result.effects.is_empty());
    }
}
