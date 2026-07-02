use std::str::FromStr;

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use tool_refactor_macros::tool;

use crate::{
    task::{TaskStatus, TaskUpdate, render_task_json, render_task_list},
    tool::ToolContext,
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskCreateInput {
    #[schemars(description = "Short subject for the task.")]
    pub subject: String,
    #[schemars(description = "Optional detailed task description.")]
    pub description: Option<String>,
}

#[tool(name = "task_create", description = "Create a new persistent task.")]
pub async fn task_create(ctx: ToolContext, input: TaskCreateInput) -> Result<String> {
    let task = ctx.task_manager.create(
        input.subject,
        input
            .description
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()),
    )?;
    render_task_json(&task)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskGetInput {
    #[schemars(description = "Task id to fetch.")]
    pub task_id: u64,
}

#[tool(name = "task_get", description = "Get full details of a task by ID.")]
pub async fn task_get(ctx: ToolContext, input: TaskGetInput) -> Result<String> {
    let task = ctx.task_manager.get(input.task_id)?;
    render_task_json(&task)
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TaskListInput {}

#[tool(
    name = "task_list",
    description = "List all tasks with status summary."
)]
pub async fn task_list(ctx: ToolContext, _input: TaskListInput) -> Result<String> {
    Ok(render_task_list(ctx.task_manager.list()?))
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

#[tool(
    name = "task_update",
    description = "Update a task's status, owner, or dependencies."
)]
pub async fn task_update(ctx: ToolContext, input: TaskUpdateInput) -> Result<String> {
    let status = input
        .status
        .as_deref()
        .map(TaskStatus::from_str)
        .transpose()
        .map_err(|_| {
            anyhow::anyhow!("Invalid status. Use pending, in_progress, completed, or deleted")
        })?;

    let task = ctx.task_manager.update(
        input.task_id,
        TaskUpdate {
            status,
            owner: input.owner,
            add_blocked_by: input.add_blocked_by,
            add_blocks: input.add_blocks,
        },
    )?;
    render_task_json(&task)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        background::SharedBackgroundManager,
        cron::{CronScheduler, SharedCronScheduler},
        memory::MemoryManager,
        skill::SkillRegistry,
        store::StoreRoot,
        task::{SharedTaskManager, TaskManager},
        team::{SharedTeammateManager, TeammateManager},
        tool::ToolRouter,
        worktree::{SharedWorktreeManager, WorktreeManager},
    };

    use super::*;

    fn test_context(name: &str) -> ToolContext {
        let root_dir = std::env::temp_dir().join(format!("tact-task-tool-test-{name}"));
        let _ = std::fs::remove_dir_all(&root_dir);
        std::fs::create_dir_all(&root_dir).unwrap();
        let store_root = StoreRoot::new(root_dir.join(".claude")).unwrap();

        ToolContext {
            skill_registry: Arc::new(SkillRegistry::new(root_dir.join("skills"))),
            memory_manager: Arc::new(std::sync::Mutex::new(MemoryManager::new(
                root_dir.join(".claude/memory"),
            ))),
            work_dir: root_dir.clone(),
            task_manager: SharedTaskManager::new(TaskManager::new(&store_root).unwrap()),
            background_manager: SharedBackgroundManager::new(&store_root).unwrap(),
            cron_scheduler: SharedCronScheduler::new(CronScheduler::new(&store_root).unwrap()),
            teammate_manager: SharedTeammateManager::new(
                TeammateManager::new(&store_root).unwrap(),
            ),
            worktree_manager: SharedWorktreeManager::new(
                WorktreeManager::new(&store_root, root_dir).unwrap(),
            ),
            ui_tx: None,
        }
    }

    #[tokio::test]
    async fn task_create_returns_json() {
        let router = ToolRouter::new().route(TaskCreateTool);
        let context = test_context("task_create_returns_json");

        let output = router
            .call(
                &context,
                "task_create",
                serde_json::json!({
                    "subject": "Write tests",
                    "description": "Add task tool coverage"
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("\"subject\": \"Write tests\""));
        assert!(output.contains("\"description\": \"Add task tool coverage\""));
        assert!(output.contains("\"status\": \"pending\""));
    }

    #[tokio::test]
    async fn task_create_strips_empty_description() {
        let router = ToolRouter::new().route(TaskCreateTool);
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
    async fn task_get_returns_task() {
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .route(TaskGetTool);
        let context = test_context("task_get_returns_task");

        let created = router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Fetch me" }),
            )
            .await
            .unwrap();
        let id: u64 = serde_json::from_str::<serde_json::Value>(&created)
            .unwrap()
            .get("id")
            .unwrap()
            .as_u64()
            .unwrap();

        let output = router
            .call(&context, "task_get", serde_json::json!({ "task_id": id }))
            .await
            .unwrap();

        assert!(output.contains("\"subject\": \"Fetch me\""));
    }

    #[tokio::test]
    async fn task_list_shows_tasks() {
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .route(TaskListTool);
        let context = test_context("task_list_shows_tasks");

        router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Listed task" }),
            )
            .await
            .unwrap();

        let output = router
            .call(&context, "task_list", serde_json::json!({}))
            .await
            .unwrap();

        assert!(output.contains("[ ] #1: Listed task"));
    }

    #[tokio::test]
    async fn task_update_status() {
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .route(TaskUpdateTool);
        let context = test_context("task_update_status");

        let created = router
            .call(
                &context,
                "task_create",
                serde_json::json!({ "subject": "Update me" }),
            )
            .await
            .unwrap();
        let id: u64 = serde_json::from_str::<serde_json::Value>(&created)
            .unwrap()
            .get("id")
            .unwrap()
            .as_u64()
            .unwrap();

        let output = router
            .call(
                &context,
                "task_update",
                serde_json::json!({
                    "task_id": id,
                    "status": "in_progress",
                    "owner": "alice"
                }),
            )
            .await
            .unwrap();

        assert!(output.contains("\"status\": \"in_progress\""));
        assert!(output.contains("\"owner\": \"alice\""));
    }

    #[tokio::test]
    async fn task_update_rejects_invalid_status() {
        let router = ToolRouter::new()
            .route(TaskCreateTool)
            .route(TaskUpdateTool);
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

        assert!(error
            .to_string()
            .contains("Invalid status. Use pending, in_progress, completed, or deleted"));
    }
}
