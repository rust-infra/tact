use std::str::FromStr;

use crate::tool::{
    ArgumentSummaryPolicy, DetailPolicy, LiveOutputPolicy, OutputPolicy, PermissionPolicy,
    PermissionPromptPolicy, PopupPolicy, ResourcePolicy, TaskOperation, ToolDomain, ToolMetadata,
    ToolPresentation,
};
use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tact_protocol::ToolVisualKind;
use tool_refactor_macros::tool;

use crate::{
    task::{TaskStatus, TaskUpdate, emit_tasks_changed, render_task_json, render_task_list},
    tool::ToolContext,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskCreateInput {
    #[schemars(description = "Short subject for the task.")]
    pub subject: String,
    #[schemars(description = "Optional detailed task description.")]
    pub description: Option<String>,
}

pub const TASK_CREATE_METADATA: ToolMetadata = ToolMetadata {
    name: "task_create",
    description: "Create a new persistent task.",
    permission: PermissionPolicy::Write,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::SharedState { scope: "task" },
    domain: ToolDomain::Task(TaskOperation::Create),
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Task,
        display_name: "📋 Task",
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
/// Returns an error if the task manager fails to create the task
/// (e.g., storage error).
pub async fn task_create(ctx: ToolContext, input: TaskCreateInput) -> Result<String> {
    let task = ctx
        .task_manager
        .create(
            input.subject,
            input
                .description
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty()),
            ctx.session_id.clone().unwrap_or_default(),
        )
        .await?;
    let listed = ctx.task_manager.list().await.unwrap_or_default();
    emit_tasks_changed(
        &ctx.ui_tx,
        listed,
        tact_protocol::TasksChangeReason::Created,
    );
    render_task_json(&task)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskGetInput {
    #[schemars(description = "Task id to fetch.")]
    pub task_id: u64,
}

pub const TASK_GET_METADATA: ToolMetadata = ToolMetadata {
    name: "task_get",
    description: "Get full details of a task by ID.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Task(TaskOperation::Get),
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Task,
        display_name: "📋 Task",
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
/// Returns an error if the task ID does not exist.
pub async fn task_get(ctx: ToolContext, input: TaskGetInput) -> Result<String> {
    let task = ctx.task_manager.get(input.task_id).await?;
    render_task_json(&task)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskListInput {}

pub const TASK_LIST_METADATA: ToolMetadata = ToolMetadata {
    name: "task_list",
    description: "List all tasks with status summary.",
    permission: PermissionPolicy::Read,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::Independent,
    domain: ToolDomain::Task(TaskOperation::List),
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Task,
        display_name: "📋 Task",
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
/// Returns an error if the task manager fails to retrieve the task list.
pub async fn task_list(ctx: ToolContext, _input: TaskListInput) -> Result<String> {
    Ok(render_task_list(ctx.task_manager.list().await?))
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskUpdateInput {
    #[schemars(description = "Task id to update.")]
    pub task_id: u64,
    #[schemars(description = "Optional status: pending, in_progress, completed, or deleted.")]
    pub status: Option<String>,
    #[schemars(description = "Optional owner or teammate name.")]
    pub owner: Option<String>,
    #[serde(rename = "addBlockedBy", default)]
    #[schemars(description = "Task ids that block this task.")]
    pub add_blocked_by: Vec<u64>,
    #[serde(rename = "addBlocks", default)]
    #[schemars(description = "Task ids blocked by this task.")]
    pub add_blocks: Vec<u64>,
}

pub const TASK_UPDATE_METADATA: ToolMetadata = ToolMetadata {
    name: "task_update",
    description: "Update a task's status, owner, or dependencies.",
    permission: PermissionPolicy::Write,
    permission_prompt: PermissionPromptPolicy::Json,
    resources: ResourcePolicy::SharedState { scope: "task" },
    domain: ToolDomain::Task(TaskOperation::Update),
    presentation: ToolPresentation {
        visual_kind: ToolVisualKind::Task,
        display_name: "📋 Task",
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
/// Returns an error if:
/// - The status string is invalid (must be one of: pending, in_progress, completed, or deleted).
/// - The task ID does not exist.
/// - The task manager fails to update the task.
pub async fn task_update(ctx: ToolContext, input: TaskUpdateInput) -> Result<String> {
    let status = input
        .status
        .as_deref()
        .map(TaskStatus::from_str)
        .transpose()
        .map_err(|_| {
            anyhow::anyhow!("Invalid status. Use pending, in_progress, completed, or deleted")
        })?;

    let task = ctx
        .task_manager
        .update(
            input.task_id,
            TaskUpdate {
                status,
                owner: input.owner,
                add_blocked_by: input.add_blocked_by,
                add_blocks: input.add_blocks,
            },
        )
        .await?;
    let listed = ctx.task_manager.list().await.unwrap_or_default();
    emit_tasks_changed(
        &ctx.ui_tx,
        listed,
        tact_protocol::TasksChangeReason::Updated,
    );
    render_task_json(&task)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::{ToolRouter, test_support::test_context};
    use tact_protocol::{AgentUpdate, TasksChangeReason};

    #[tokio::test]
    async fn task_create_strips_empty_description() {
        let router = ToolRouter::new().route(TaskCreateTool).unwrap();
        let context = test_context("task_create_strips_empty_description");

        let output = router
            .call(
                &context,
                "task_create",
                serde_json::json!({
                    "subject": "No description",
                    "description": "   "
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("\"subject\": \"No description\""));
        assert!(!output.contains("\"description\""));
    }

    #[tokio::test]
    async fn task_update_rejects_invalid_status() {
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .unwrap()
            .route(TaskUpdateTool)
            .unwrap();
        let context = test_context("task_update_rejects_invalid_status");

        let created = router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Bad status" }),
            )
            .await
            .unwrap();
        let id: u64 = serde_json::from_str::<serde_json::Value>(&created)
            .unwrap()
            .get("id")
            .unwrap()
            .as_u64()
            .unwrap();

        let error = router
            .call(
                &context,
                "task_update",
                serde_json::json!({
                    "task_id": id,
                    "status": "not_a_status"
                }),
            )
            .await
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Invalid status. Use pending, in_progress, completed, or deleted")
        );
    }

    #[tokio::test]
    async fn task_create_emits_tasks_changed() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let router = ToolRouter::new().route(TaskCreateTool).unwrap();
        let mut context = test_context("task_create_emits");
        context.ui_tx = Some(tx);

        router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Ship panel" }),
            )
            .await
            .unwrap();

        let update = rx.try_recv().expect("TasksChanged");
        match update {
            AgentUpdate::TasksChanged { tasks, reason } => {
                assert!(matches!(reason, TasksChangeReason::Created));
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].subject, "Ship panel");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn task_update_emits_tasks_changed_and_filters_deleted() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .unwrap()
            .route(TaskUpdateTool)
            .unwrap();
        let mut context = test_context("task_update_emits");
        context.ui_tx = Some(tx.clone());

        let created = router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Temp" }),
            )
            .await
            .unwrap();
        let _ = rx.try_recv();
        let id: u64 = serde_json::from_str::<serde_json::Value>(&created)
            .unwrap()
            .get("id")
            .unwrap()
            .as_u64()
            .unwrap();

        context.ui_tx = Some(tx);
        router
            .call(
                &context,
                "task_update",
                serde_json::json!({
                    "task_id": id,
                    "status": "deleted"
                }),
            )
            .await
            .unwrap();

        let update = rx.try_recv().expect("TasksChanged after update");
        match update {
            AgentUpdate::TasksChanged { tasks, reason } => {
                assert!(matches!(reason, TasksChangeReason::Updated));
                assert!(tasks.is_empty(), "deleted tasks omitted from snapshot");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn task_list_does_not_emit_tasks_changed() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .unwrap()
            .route(TaskListTool)
            .unwrap();
        let mut context = test_context("task_list_no_emit");
        context.ui_tx = Some(tx.clone());
        let _ = router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "x" }),
            )
            .await
            .unwrap();
        while rx.try_recv().is_ok() {}

        context.ui_tx = Some(tx);
        router
            .call(&context, "task_list", serde_json::json!({}))
            .await
            .unwrap();
        assert!(rx.try_recv().is_err(), "task_list must not emit");
    }
}
