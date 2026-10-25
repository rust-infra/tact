//! Tool metadata: policies, presentation, and the static shape shared by the
//! kernel and extensions.
//!
//! Behavioural classification (sensitive paths, shell read-only checks) lives
//! in the extensions crate behind `PermissionService`; this module only owns
//! the declarative contract.

use serde_json::Value;
use tact_protocol::{ToolDetailKind, ToolPopupKind, ToolPresentationInfo, ToolVisualKind};

use crate::tool::resources::{ResourcePolicy, patch};

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

    /// A tool that must not run alongside any other.
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

    /// The task tools: one per [`TaskOperation`].
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

    /// The task tools' writing half.
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

/// An owned presentation descriptor.
///
/// Native tools declare [`ToolPresentation`] with static labels; a dynamic
/// source (MCP) supplies owned labels. `ToolPolicy` is the owned form used by
/// [`ToolDescriptor`](crate::tool::descriptor::ToolDescriptor).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPresentationOwned {
    pub visual_kind: ToolVisualKind,
    pub display_name: String,
    pub live_output: LiveOutputPolicy,
    pub detail: DetailPolicy,
    pub popup: PopupPolicy,
    pub compact_result_to_meta: bool,
}

impl From<ToolPresentation> for ToolPresentationOwned {
    fn from(value: ToolPresentation) -> Self {
        Self {
            visual_kind: value.visual_kind,
            display_name: value.display_name.to_string(),
            live_output: value.live_output,
            detail: value.detail,
            popup: value.popup,
            compact_result_to_meta: value.compact_result_to_meta,
        }
    }
}

/// The owned policy carried by a [`ToolDescriptor`](crate::tool::descriptor::ToolDescriptor).
///
/// This is the policy-only half of a native [`ToolMetadata`]: no static name,
/// description or display string is required, so a source discovered at runtime
/// (MCP) can construct one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicy {
    pub permission: PermissionPolicy,
    pub permission_prompt: PermissionPromptPolicy,
    pub resources: ResourcePolicy,
    pub domain: ToolDomain,
    pub presentation: ToolPresentationOwned,
    pub output: OutputPolicy,
    pub argument_summary: ArgumentSummaryPolicy,
    /// The source declared this call pre-approved (an MCP entry's
    /// `approval_mode: "auto"`).
    ///
    /// Deliberately *not* a lower risk: `Read` is allowed before plan mode, so
    /// mapping this onto `Read` would let a write run in plan mode. It is
    /// consulted after plan mode and after an explicit `deny` rule, so it can
    /// only skip the prompt.
    pub auto_approved: bool,
}

impl ToolMetadata {
    /// Convert this native declaration into an owned policy.
    #[must_use]
    pub fn policy(&self) -> ToolPolicy {
        ToolPolicy {
            permission: self.permission,
            permission_prompt: self.permission_prompt,
            resources: self.resources,
            domain: self.domain,
            presentation: self.presentation.into(),
            output: self.output,
            argument_summary: self.argument_summary,
            // A built-in tool never declares itself pre-approved: that is a
            // property of a remotely-configured source.
            auto_approved: false,
        }
    }
}

/// How a tool's target decides the permission risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPolicy {
    Read,
    Write,
    High,
    ReadPath { path_field: &'static str },
    WritePath { path_field: &'static str },
    PatchPaths,
    ShellCommand { command_field: &'static str },
}

impl PermissionPolicy {
    /// The raw string this policy is about, when it has one.
    ///
    /// Pure JSON/patch parsing only: sensitive-path and shell classification
    /// live behind the extension `PermissionService`.
    #[must_use]
    pub fn target(&self, input: &Value) -> Option<String> {
        match self {
            PermissionPolicy::ReadPath { path_field }
            | PermissionPolicy::WritePath { path_field } => input
                .get(*path_field)
                .and_then(|value| value.as_str())
                .map(str::to_string),
            PermissionPolicy::PatchPaths => {
                patch::extract_patch_paths(input.get("patch").and_then(|value| value.as_str())?)
                    .into_iter()
                    .next()
            }
            PermissionPolicy::ShellCommand { command_field } => input
                .get(*command_field)
                .and_then(|value| value.as_str())
                .map(str::to_string),
            PermissionPolicy::Read | PermissionPolicy::Write | PermissionPolicy::High => None,
        }
    }
}

/// The text a permission prompt shows for an "always allow" rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionPromptPolicy {
    Json,
    Question { field: &'static str },
    Command { field: &'static str },
    Path { field: &'static str },
    PatchTarget { patch_field: &'static str },
}

impl PermissionPromptPolicy {
    /// The `(field, value)` an "always allow" rule should be built from.
    #[must_use]
    pub fn generate_key(&self, input: &Value) -> Option<(&'static str, String)> {
        match self {
            PermissionPromptPolicy::Command { field }
            | PermissionPromptPolicy::Question { field }
            | PermissionPromptPolicy::Path { field } => input
                .get(*field)
                .and_then(|value| value.as_str())
                .map(|value| (*field, value.to_string())),
            PermissionPromptPolicy::PatchTarget { patch_field } => {
                let patch = input.get(*patch_field).and_then(|value| value.as_str())?;
                let paths = patch::extract_patch_paths(patch);
                if paths.len() != 1 {
                    return None;
                }
                Some((*patch_field, format!("*{}*", paths[0])))
            }
            PermissionPromptPolicy::Json => None,
        }
    }
}

/// Which domain a tool belongs to, for presentation and task snapshots.
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveOutputPolicy {
    Standard,
    FullTranscript,
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
    Id { field: &'static str },
    PatchPreview { patch_field: &'static str },
    ReadOffsetLimit { path_field: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn read_json_preset_is_read_independent() {
        let metadata = ToolMetadata::read_json("echo", "Echo", "Echo");
        assert_eq!(metadata.permission, PermissionPolicy::Read);
        assert_eq!(metadata.resources, ResourcePolicy::Independent);
    }

    #[test]
    fn path_read_preset_names_one_field_everywhere() {
        let metadata = ToolMetadata::path_read("read_file", "Read", "Read File", "path");
        assert_eq!(
            metadata.permission,
            PermissionPolicy::ReadPath { path_field: "path" }
        );
        assert_eq!(
            metadata.permission_prompt,
            PermissionPromptPolicy::Path { field: "path" }
        );
        assert_eq!(
            metadata.resources,
            ResourcePolicy::ReadPath { field: "path" }
        );
        assert_eq!(
            metadata.argument_summary,
            ArgumentSummaryPolicy::Path { field: "path" }
        );
    }

    #[test]
    fn path_resource_policy_resolves_and_reports_recent_path() {
        let policy = ResourcePolicy::WritePath { field: "path" };
        let input = json!({ "path": "src/lib.rs" });
        let resources = policy.resolve(&input, Path::new("/repo"));

        assert_eq!(resources.writes, vec![Path::new("/repo/src/lib.rs")]);
        assert_eq!(policy.recent_paths(&input), vec!["src/lib.rs"]);
    }

    #[test]
    fn patch_target_paths_extracts_git_and_bare_headers() {
        assert_eq!(
            patch::patch_target_paths("+++ b/src/lib.rs\n+++ /dev/null\n"),
            vec!["src/lib.rs"]
        );
        assert_eq!(
            patch::patch_target_paths("+++ secrets.json\n"),
            vec!["secrets.json"]
        );
    }

    #[test]
    fn permission_target_is_pure_json_parsing() {
        let read = PermissionPolicy::ReadPath { path_field: "path" };
        assert_eq!(
            read.target(&json!({"path": "src/lib.rs"})),
            Some("src/lib.rs".to_string())
        );
        let patch = PermissionPolicy::PatchPaths;
        assert_eq!(
            patch.target(&json!({"patch": "+++ b/src/lib.rs\n"})),
            Some("src/lib.rs".to_string())
        );
        let shell = PermissionPolicy::ShellCommand {
            command_field: "command",
        };
        assert_eq!(
            shell.target(&json!({"command": "ls"})),
            Some("ls".to_string())
        );
    }
}
