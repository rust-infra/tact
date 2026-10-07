//! Built-in tool registration for the main agent and sub-agents.

use super::{
    ToolRouter,
    ask_user::AskUserTool,
    background_run::{BackgroundRunTool, CheckBackgroundTool, WaitBackgroundTool},
    bash::BashTool,
    compact::CompactTool,
    edit_file::EditFileTool,
    load_skill::LoadSkillTool,
    memory::SaveMemoryTool,
    read_file::ReadFileTool,
    read_image::ReadImageTool,
    sleep::SleepTool,
    subagent::{CancelSubagentTool, CheckSubagentTool, SpawnSubagentTool, WaitSubagentTool},
    task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool},
    team::{
        BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
        ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
    },
    worktree::{
        WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRemoveTool,
        WorktreeRunTool, WorktreeStatusTool,
    },
    write_file::WriteFileTool,
};

/// Assembles the full tool set for the main agent loop.
///
/// `memory_enabled` mirrors `[agent].memory_enabled`: when false, `save_memory`
/// is not registered, so it disappears from the advertised specs and dispatch
/// rejects it as an unknown tool.
fn try_toolset(memory_enabled: bool) -> anyhow::Result<ToolRouter> {
    let router = ToolRouter::new()
        .route(AskUserTool)?
        .route(BashTool)?
        .route(BackgroundRunTool)?
        .route(CheckBackgroundTool)?
        .route(WaitBackgroundTool)?
        .route(ReadFileTool)?
        .route(ReadImageTool)?
        .route(SleepTool)?
        .route(WriteFileTool)?
        .route(EditFileTool)?
        .route(LoadSkillTool)?;
    let router = if memory_enabled {
        router.route(SaveMemoryTool)?
    } else {
        router
    };
    router
        .route(CompactTool)?
        .route(SpawnSubagentTool)?
        .route(CheckSubagentTool)?
        .route(WaitSubagentTool)?
        .route(CancelSubagentTool)?
        .route(TaskCreateTool)?
        .route(TaskGetTool)?
        .route(TaskListTool)?
        .route(TaskUpdateTool)?
        .route(SpawnTeammateTool)?
        .route(ListTeammatesTool)?
        .route(SendMessageTool)?
        .route(BroadcastTool)?
        .route(ReadInboxTool)?
        .route(PlanApprovalTool)?
        .route(ShutdownRequestTool)?
        .route(ShutdownResponseTool)?
        .route(WorktreeCreateTool)?
        .route(WorktreeListTool)?
        .route(WorktreeStatusTool)?
        .route(WorktreeRunTool)?
        .route(WorktreeRemoveTool)?
        .route(WorktreeEventsTool)
}

pub fn toolset() -> ToolRouter {
    toolset_with_memory(true)
}

/// Tool set for the main agent, with Tact's persistent memory optionally off.
pub fn toolset_with_memory(memory_enabled: bool) -> ToolRouter {
    try_toolset(memory_enabled).expect("built-in tool metadata must be valid")
}

/// Assembles the restricted tool set for sub-agents.
fn try_subagent_toolset() -> anyhow::Result<ToolRouter> {
    ToolRouter::new()
        .route(BashTool)?
        .route(ReadFileTool)?
        .route(SleepTool)?
        .route(WriteFileTool)?
        .route(EditFileTool)
}

pub fn subagent_toolset() -> ToolRouter {
    try_subagent_toolset().expect("subagent tool metadata must be valid")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{
        ArgumentSummaryPolicy, PermissionPolicy, PermissionPromptPolicy, ResourcePolicy,
    };

    fn names(router: ToolRouter) -> Vec<String> {
        router
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect()
    }

    /// Every tool that names an input field names the *same* field everywhere.
    ///
    /// One metadata constant spells the name out in up to three places: the
    /// permission that decides the risk, the prompt that decides what an
    /// "always allow" answer is keyed on, and the argument summary that becomes
    /// the card's title. They are separate literals, so a typo in one of them
    /// paints a prompt about one path while the scheduler reserves another —
    /// and nothing downstream compares them, because each consumer only ever
    /// reads its own.
    ///
    /// This is the invariant the field-naming presets rely on, asserted over
    /// the assembled toolset rather than one constant at a time: a tool added
    /// with three disagreeing fields fails here even though every individual
    /// policy is well-formed.
    #[test]
    fn every_tool_names_one_input_field_across_its_policies() {
        let router = toolset();
        for spec in router.tool_specs() {
            let metadata = router
                .resolve(&spec.name)
                .unwrap_or_else(|_| panic!("{} came from the router and must resolve", spec.name))
                .metadata();

            match metadata.permission {
                PermissionPolicy::ShellCommand { command_field } => {
                    assert_eq!(
                        metadata.permission_prompt,
                        PermissionPromptPolicy::Command {
                            field: command_field
                        },
                        "{}: the prompt must key on the field the risk was decided from",
                        spec.name
                    );
                    assert_eq!(
                        metadata.argument_summary,
                        ArgumentSummaryPolicy::Command {
                            field: command_field
                        },
                        "{}: the summary must render the field the risk was decided from",
                        spec.name
                    );
                }
                PermissionPolicy::ReadPath { path_field }
                | PermissionPolicy::WritePath { path_field } => {
                    assert_eq!(
                        metadata.permission_prompt,
                        PermissionPromptPolicy::Path { field: path_field },
                        "{}: the prompt's path field",
                        spec.name
                    );
                    let resource_field = match metadata.resources {
                        ResourcePolicy::ReadPath { field }
                        | ResourcePolicy::WritePath { field } => field,
                        other => panic!(
                            "{}: a path policy needs a path resource, got {other:?}",
                            spec.name
                        ),
                    };
                    assert_eq!(
                        resource_field, path_field,
                        "{}: the scheduler would reserve a different path than the prompt asked about",
                        spec.name
                    );
                    assert_eq!(
                        metadata.argument_summary,
                        ArgumentSummaryPolicy::Path { field: path_field },
                        "{}: the summary's path field",
                        spec.name
                    );
                }
                PermissionPolicy::PatchPaths => {
                    let patch_field = match metadata.permission_prompt {
                        PermissionPromptPolicy::PatchTarget { patch_field } => patch_field,
                        other => panic!(
                            "{}: a patch policy needs a patch prompt, got {other:?}",
                            spec.name
                        ),
                    };
                    let (patch_field_resource, dry_run_field) = match metadata.resources {
                        ResourcePolicy::PatchFiles {
                            patch_field,
                            dry_run_field,
                        } => (patch_field, dry_run_field),
                        other => panic!(
                            "{}: a patch policy needs a patch resource, got {other:?}",
                            spec.name
                        ),
                    };
                    assert_eq!(patch_field_resource, patch_field, "{}", spec.name);
                    assert_eq!(
                        metadata.argument_summary,
                        ArgumentSummaryPolicy::PatchPreview { patch_field },
                        "{}: the summary must preview the patch the risk was decided from",
                        spec.name
                    );
                    assert_eq!(
                        dry_run_field, "dry_run",
                        "{}: the resource policy reads a dry-run flag the schema must have",
                        spec.name
                    );
                }
                // Fieldless policies cannot disagree with themselves.
                PermissionPolicy::Read | PermissionPolicy::Write | PermissionPolicy::High => {}
            }
        }
    }

    #[test]
    fn memory_is_on_in_the_default_toolset() {
        let names = names(toolset());
        assert!(names.contains(&"save_memory".to_string()), "{names:?}");
    }

    #[test]
    fn disabled_memory_removes_save_memory() {
        let names = names(toolset_with_memory(false));
        assert!(!names.contains(&"save_memory".to_string()), "{names:?}");
    }
}
