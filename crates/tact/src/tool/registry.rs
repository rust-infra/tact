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
    subagent::{CancelSubagentTool, CheckSubagentTool, SpawnSubagentTool},
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

/// Builds the restricted subagent toolset, optionally keeping only the tools
/// named by a declarative agent definition's `tools:` frontmatter (Claude Code
/// naming: Read/Glob/Grep → `read_file`, Bash → `bash`, Edit → `edit_file`,
/// Write → `write_file`, Sleep → `sleep`). Unknown names are ignored while at
/// least one known name remains. `None` and an **empty** list keep the default
/// five-tool set (Claude semantics: an absent/empty `tools:` does not
/// restrict). When `keep` is non-empty but **no** name maps to a known tool,
/// the result is an empty router — callers must fail rather than silently
/// widen permissions to the default set.
pub fn subagent_toolset_for(keep: Option<&[String]>) -> ToolRouter {
    let Some(keep) = keep else {
        return subagent_toolset();
    };
    if keep.is_empty() {
        return subagent_toolset();
    }
    let allowed = allowed_tool_names(keep);
    if allowed.is_empty() {
        // Declared tools exist but none map to a Tact tool: return an empty
        // router so the caller can error out instead of granting the full set.
        return ToolRouter::new();
    }
    try_subagent_toolset_filtered(&allowed).expect("subagent tool metadata must be valid")
}

/// Maps Claude Code tool names to the Tact tool names available to subagents.
fn allowed_tool_names(keep: &[String]) -> std::collections::HashSet<&'static str> {
    let mut allowed = std::collections::HashSet::new();
    for name in keep {
        match name.to_ascii_lowercase().as_str() {
            "bash" => {
                allowed.insert("bash");
            }
            "read" | "glob" | "grep" => {
                allowed.insert("read_file");
            }
            "write" => {
                allowed.insert("write_file");
            }
            "edit" => {
                allowed.insert("edit_file");
            }
            "sleep" => {
                allowed.insert("sleep");
            }
            _ => {}
        }
    }
    allowed
}

fn try_subagent_toolset_filtered(
    allowed: &std::collections::HashSet<&'static str>,
) -> anyhow::Result<ToolRouter> {
    let mut router = ToolRouter::new();
    if allowed.contains("bash") {
        router = router.route(BashTool)?;
    }
    if allowed.contains("read_file") {
        router = router.route(ReadFileTool)?;
    }
    if allowed.contains("sleep") {
        router = router.route(SleepTool)?;
    }
    if allowed.contains("write_file") {
        router = router.route(WriteFileTool)?;
    }
    if allowed.contains("edit_file") {
        router = router.route(EditFileTool)?;
    }
    Ok(router)
}
