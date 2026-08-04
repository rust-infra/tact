use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

use crate::tool::ToolContext;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorktreeCreateInput {
    pub name: String,
    pub task_id: Option<u64>,
    pub base_ref: Option<String>,
}

pub const WORKTREE_CREATE_METADATA: ToolMetadata = ToolMetadata {
    name: "worktree_create",
    description: "Create an isolated git worktree lane.",
    permission: PermissionPolicy::Write,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🌿 Worktree Create",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

#[tool]
/// # Errors
///
/// Returns an error if the git worktree cannot be created (e.g., invalid
/// branch reference, name conflict, or git error).
pub async fn worktree_create(ctx: ToolContext, input: WorktreeCreateInput) -> Result<String> {
    ctx.worktree_manager.create(
        input.name,
        input.task_id,
        input.base_ref.unwrap_or_else(|| "HEAD".to_string()),
    )
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorktreeListInput {}

pub const WORKTREE_LIST_METADATA: ToolMetadata = ToolMetadata {
    name: "worktree_list",
    description: "List tracked worktree lanes.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🌿 Worktree List",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

#[tool]
/// # Errors
///
/// Returns an error if the worktree manager fails to retrieve the list.
pub async fn worktree_list(ctx: ToolContext, _input: WorktreeListInput) -> Result<String> {
    ctx.worktree_manager.list()
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorktreeNameInput {
    pub name: String,
}

pub const WORKTREE_STATUS_METADATA: ToolMetadata = ToolMetadata {
    name: "worktree_status",
    description: "Show git status for a worktree lane.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🌿 Worktree Status",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

#[tool]
/// # Errors
///
/// Returns an error if the worktree name does not exist or the git
/// status command fails.
pub async fn worktree_status(ctx: ToolContext, input: WorktreeNameInput) -> Result<String> {
    ctx.worktree_manager.status(&input.name)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorktreeRunInput {
    pub name: String,
    pub command: String,
}

pub const WORKTREE_RUN_METADATA: ToolMetadata = ToolMetadata {
    name: "worktree_run",
    description: "Run one shell command inside a named worktree.",
    permission: PermissionPolicy::ShellCommand {
        command_field: "command",
    },
    permission_prompt: PermissionPromptPolicy::Command { field: "command" },
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Command,
        display_name: "🌿 Worktree Run",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Command { field: "command" },
};

#[tool]
/// # Errors
///
/// Returns an error if the worktree name does not exist or the
/// command execution fails.
pub async fn worktree_run(ctx: ToolContext, input: WorktreeRunInput) -> Result<String> {
    ctx.worktree_manager.run(&input.name, &input.command)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorktreeEventsInput {
    pub limit: Option<usize>,
}

pub const WORKTREE_EVENTS_METADATA: ToolMetadata = ToolMetadata {
    name: "worktree_events",
    description: "List recent worktree lifecycle events.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🌿 Worktree Events",
        live_output: LiveOutputPolicy::Standard,
        detail: DetailPolicy::Result,
        popup: PopupPolicy::None,
        compact_result_to_meta: false,
    },
    output: OutputPolicy::KeepInline,
    argument_summary: ArgumentSummaryPolicy::Json,
};

#[tool]
/// # Errors
///
/// Returns an error if the worktree manager fails to retrieve events.
pub async fn worktree_events(ctx: ToolContext, input: WorktreeEventsInput) -> Result<String> {
    ctx.worktree_manager.events(input.limit.unwrap_or(20))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::{run_tool, test_context};

    #[tokio::test]
    async fn worktree_list_empty_by_default() {
        let context = test_context("worktree_list_empty_by_default");

        let output = run_tool(
            &context,
            WorktreeListTool,
            "worktree_list",
            serde_json::json!({}),
        )
        .await
        .unwrap();

        assert_eq!(output, "No worktrees.");
    }

    #[tokio::test]
    async fn worktree_events_empty_by_default() {
        let context = test_context("worktree_events_empty_by_default");

        let output = run_tool(
            &context,
            WorktreeEventsTool,
            "worktree_events",
            serde_json::json!({ "limit": 5 }),
        )
        .await
        .unwrap();

        assert_eq!(output, "");
    }
}
