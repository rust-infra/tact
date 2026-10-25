//! Tool metadata compatibility re-exports.
//!
//! The declarative contract now lives in `tact_kernel`. This module keeps the
//! historical `tact::tool::metadata` and `tact::tool::*` paths working while the
//! extension crate migrates.

pub use tact_kernel::tool::resources::patch::{extract_patch_paths, patch_target_paths};
pub use tact_kernel::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, TaskOperation, ToolDomain, ToolMetadata,
    ToolPresentation,
};

pub use tact_kernel::tool::{
    IntoToolCallResult, ToolCallResult, ToolEffect, ToolImage, YieldReason,
};

/// Historical name for [`YieldReason`], kept for source compatibility.
pub type GoalYieldReason = YieldReason;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::CapabilityRisk;
    use crate::permission::policy_ext::PermissionPolicyExt;
    use serde_json::json;
    use std::path::Path;
    use tact_protocol::{ToolPopupKind, ToolVisualKind};

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
