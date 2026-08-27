//! Built-in tool registration for the main agent and sub-agents.

use super::{
    ToolRouter,
    ask_user::AskUserTool,
    background_run::{BackgroundRunTool, CheckBackgroundTool},
    bash::BashTool,
    compact::CompactTool,
    edit_file::EditFileTool,
    load_skill::LoadSkillTool,
    memory::SaveMemoryTool,
    read_file::ReadFileTool,
    read_image::ReadImageTool,
    sleep::SleepTool,
    subagent::{CheckSubagentTool, SpawnSubagentTool},
    task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool},
    team::{
        BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
        ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
    },
    worktree::{
        WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRunTool,
        WorktreeStatusTool,
    },
    write_file::WriteFileTool,
};

/// Assembles the full tool set for the main agent loop.
fn try_toolset() -> anyhow::Result<ToolRouter> {
    ToolRouter::new()
        .route(AskUserTool)?
        .route(BashTool)?
        .route(BackgroundRunTool)?
        .route(CheckBackgroundTool)?
        .route(ReadFileTool)?
        .route(ReadImageTool)?
        .route(SleepTool)?
        .route(WriteFileTool)?
        .route(EditFileTool)?
        .route(LoadSkillTool)?
        .route(SaveMemoryTool)?
        .route(CompactTool)?
        .route(SpawnSubagentTool)?
        .route(CheckSubagentTool)?
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
        .route(WorktreeEventsTool)
}

pub fn toolset() -> ToolRouter {
    try_toolset().expect("built-in tool metadata must be valid")
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
