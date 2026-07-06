//! Built-in tool registration for the main agent and sub-agents.

use super::apply_patch::ApplyPatchTool;
use super::ask_user::AskUserTool;
use super::background_run::{BackgroundRunTool, CheckBackgroundTool};
use super::bash::BashTool;
use super::batch_edit::BatchEditTool;
use super::batch_read::BatchReadTool;
use super::compact::CompactTool;
use super::cron::{CronCreateTool, CronDeleteTool, CronListTool};
use super::edit_file::EditFileTool;
use super::load_skill::LoadSkillTool;
use super::lsp_tool::QueryLspTool;
use super::memory::SaveMemoryTool;
use super::read_file::ReadFileTool;
use super::search_code::SearchCodeTool;
use super::sleep::SleepTool;
use super::subagent::TaskTool;
use super::task::{TaskCreateTool, TaskGetTool, TaskListTool, TaskUpdateTool};
use super::team::{
    BroadcastTool, ListTeammatesTool, PlanApprovalTool, ReadInboxTool, SendMessageTool,
    ShutdownRequestTool, ShutdownResponseTool, SpawnTeammateTool,
};
use super::web::{WebFetchTool, WebSearchTool};
use super::worktree::{
    WorktreeCreateTool, WorktreeEventsTool, WorktreeListTool, WorktreeRunTool, WorktreeStatusTool,
};
use super::write_file::WriteFileTool;
use super::ToolRouter;

/// Assembles the full tool set for the main agent loop.
pub fn toolset() -> ToolRouter {
    ToolRouter::new()
        .route(ApplyPatchTool)
        .route(AskUserTool)
        .route(BashTool)
        .route(BatchEditTool)
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
