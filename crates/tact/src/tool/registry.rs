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

    fn names(router: ToolRouter) -> Vec<String> {
        router
            .tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect()
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
