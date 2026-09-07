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
    ctx.worktree_manager
        .create(
            input.name,
            input.task_id,
            input.base_ref.unwrap_or_else(|| "HEAD".to_string()),
            ctx.session_id.clone().unwrap_or_default(),
        )
        .await
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
    ctx.worktree_manager.list().await
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
    ctx.worktree_manager.status(&input.name).await
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
    ctx.worktree_manager.run(&input.name, &input.command).await
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
    ctx.worktree_manager.events(input.limit.unwrap_or(20)).await
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WorktreeRemoveInput {
    #[schemars(description = "Worktree lane name to remove.")]
    pub name: String,
}

pub const WORKTREE_REMOVE_METADATA: ToolMetadata = ToolMetadata {
    name: "worktree_remove",
    description: "Remove a tracked git worktree lane (e.g. a finished subagent's lane). Fails if the worktree has uncommitted changes; refuses to remove the lane of a subagent that is still running.",
    permission: PermissionPolicy::Write,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Barrier,
    domain: ToolDomain::Generic,
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Generic,
        display_name: "🌿 Worktree Remove",
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
/// Returns an error if the worktree does not exist, `git worktree remove`
/// fails (e.g. the lane has uncommitted changes), or the lane belongs to a
/// subagent that is still running.
pub async fn worktree_remove(ctx: ToolContext, input: WorktreeRemoveInput) -> Result<String> {
    // Guard: never remove the lane of a subagent that is still running —
    // the child's tools still resolve against it and removal would be
    // destructive. `spawn_subagent` names isolated lanes `subagent-<child_id>`.
    if let Some(child_id) = input.name.strip_prefix("subagent-")
        && let Some(run) = ctx.subagent_manager.get(child_id).await?
        && run.status == crate::subagent::SubagentStatus::Running
    {
        anyhow::bail!(
            "cannot remove worktree {}: subagent {child_id} is still running; \
             cancel or wait for it first",
            input.name
        );
    }
    ctx.worktree_manager.remove(&input.name).await
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

    #[tokio::test]
    async fn worktree_remove_refuses_running_subagent_lane() {
        let context = test_context("worktree_remove_running_guard");
        // Seed a running subagent run so its `subagent-<id>` lane is protected.
        context
            .subagent_manager
            .start("child-1".to_string())
            .await
            .unwrap();

        let result = run_tool(
            &context,
            WorktreeRemoveTool,
            "worktree_remove",
            serde_json::json!({ "name": "subagent-child-1" }),
        )
        .await;

        let err = result.expect_err("must refuse a running subagent's lane");
        assert!(err.to_string().contains("still running"), "err: {err}");
    }

    #[tokio::test]
    async fn worktree_remove_unknown_name_errors() {
        let context = test_context("worktree_remove_unknown");

        let result = run_tool(
            &context,
            WorktreeRemoveTool,
            "worktree_remove",
            serde_json::json!({ "name": "no-such-lane" }),
        )
        .await;

        assert!(result.is_err(), "unknown worktree name must error");
    }
}
