//! Built-in tool registration for the main agent and sub-agents.

use super::{
    ToolRouter,
    apply_patch::ApplyPatchTool,
    ask_user::AskUserTool,
    background_run::{BackgroundRunTool, CheckBackgroundTool},
    bash::BashTool,
    batch_read::BatchReadTool,
    compact::CompactTool,
    cron::{CronCreateTool, CronDeleteTool, CronListTool},
    edit_file::EditFileTool,
    load_skill::LoadSkillTool,
    lsp_tool::QueryLspTool,
    memory::SaveMemoryTool,
    read_file::ReadFileTool,
    search_code::SearchCodeTool,
    sleep::SleepTool,
    subagent::TaskTool,
    task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool},
    team::{
        BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
        ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
    },
    web::{WebFetchTool, WebSearchTool},
    worktree::{
        WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRunTool,
        WorktreeStatusTool,
    },
    write_file::WriteFileTool,
};

/// Assembles the full tool set for the main agent loop.
pub fn toolset() -> ToolRouter {
    ToolRouter::new()
        .route(ApplyPatchTool)
        .route(AskUserTool)
        .route(BashTool)
        .route(BatchReadTool)
        .route(BackgroundRunTool)
        .route(CheckBackgroundTool)
        .route(CronCreateTool)
        .route(CronDeleteTool)
        .route(CronListTool)
        .route(ReadFileTool)
        .route(SearchCodeTool)
        .route(SleepTool)
        .route(WriteFileTool)
        .route(EditFileTool)
        .route(LoadSkillTool)
        .route(QueryLspTool)
        .route(SaveMemoryTool)
        .route(CompactTool)
        .route(TaskTool)
        .route(TaskCreateTool)
        .route(TaskGetTool)
        .route(TaskListTool)
        .route(TaskUpdateTool)
        .route(SpawnTeammateTool)
        .route(ListTeammatesTool)
        .route(SendMessageTool)
        .route(BroadcastTool)
        .route(ReadInboxTool)
        .route(PlanApprovalTool)
        .route(ShutdownRequestTool)
        .route(ShutdownResponseTool)
        .route(WorktreeCreateTool)
        .route(WorktreeListTool)
        .route(WorktreeStatusTool)
        .route(WorktreeRunTool)
        .route(WorktreeEventsTool)
        .route(WebFetchTool)
        .route(WebSearchTool)
}

/// Assembles the restricted tool set for sub-agents.
pub fn subagent_toolset() -> ToolRouter {
    ToolRouter::new()
        .route(BashTool)
        .route(ReadFileTool)
        .route(SearchCodeTool)
        .route(SleepTool)
        .route(WriteFileTool)
        .route(EditFileTool)
}
