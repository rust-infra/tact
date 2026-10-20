//! Durable task management.
//!
//! Tasks are persistent work items with status (Pending → InProgress →
//! Completed/Deleted), blocking relationships, and optional owners.
//!
//! - [`TaskManager`] is the core state machine backed by a SQLite store.
//! - [`SharedTaskManager`] shares a manager through `Arc` for concurrent
//!   access from tools; the SQLite connection pool serializes writes.
//! - [`TaskRecord`] is the wire format; it supports `blockedBy` / `blocks`
//!   for dependency tracking.
//! - [`render_task_json`] and [`render_task_list`] produce LLM-friendly
//!   textual representations.

use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use strum::EnumProperty;
use strum_macros::{Display, EnumProperty as EnumPropertyDerive, EnumString};

use crate::store::task_store::{SqliteTaskStore, TaskStore};

pub use display::{format_id_list, format_id_transition, format_task_tool_title};

mod display;

/// Task lifecycle states.
///
/// Each state has a visual marker for LLM-friendly list rendering
/// (`[ ]` → `[>]` → `[x]` / `[-]`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    EnumString,
    Display,
    EnumPropertyDerive,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum TaskStatus {
    #[strum(props(marker = "[ ]"))]
    Pending,
    #[strum(props(marker = "[>]"))]
    InProgress,
    #[strum(props(marker = "[x]"))]
    Completed,
    #[strum(props(marker = "[-]"))]
    Deleted,
}

impl TaskStatus {
    pub fn marker(self) -> &'static str {
        self.get_str("marker").unwrap_or("[?]")
    }
}

/// A record of a task in the manager.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRecord {
    pub id: u64,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Owning agent session id; empty when created outside a session.
    pub session_id: String,
    pub status: TaskStatus,
    #[serde(rename = "blockedBy", default)]
    pub blocked_by: Vec<u64>,
    #[serde(default)]
    pub blocks: Vec<u64>,
    #[serde(default)]
    pub owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub goal_id: Option<String>,
}

impl TaskRecord {
    /// Creates a new task record.
    pub fn new(id: u64, subject: String, description: Option<String>, session_id: String) -> Self {
        Self {
            id,
            subject,
            description,
            session_id,
            status: TaskStatus::Pending,
            blocked_by: Vec::new(),
            blocks: Vec::new(),
            owner: String::new(),
            created_at: Some(
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64,
            ),
            started_at: None,
            completed_at: None,
            goal_id: None,
        }
    }
}

/// A mutable update to apply to an existing task.
#[derive(Debug, Clone, Default)]
pub struct TaskUpdate {
    pub status: Option<TaskStatus>,
    pub owner: Option<String>,
    pub add_blocked_by: Vec<u64>,
    pub add_blocks: Vec<u64>,
}

/// Core task manager backed by a SQLite task store.
pub struct TaskManager {
    store: Box<dyn TaskStore>,
}

impl TaskManager {
    /// Creates a new task manager backed by the given SQLite database file.
    pub async fn new(db_path: &Path) -> Result<Self> {
        Ok(Self {
            store: Box::new(SqliteTaskStore::new(db_path).await?),
        })
    }

    /// Creates a new task with the given subject and description.
    pub async fn create(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
    ) -> Result<TaskRecord> {
        self.store.create(subject, description, session_id).await
    }

    /// Creates a new task associated with a Goal in this manager's database.
    ///
    /// The SQLite store validates the Goal in the same database and transaction
    /// before inserting the task.
    pub async fn create_for_goal(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
        goal_id: String,
    ) -> Result<TaskRecord> {
        self.store
            .create_for_goal(subject, description, session_id, goal_id)
            .await
    }

    /// Associates a task with a Goal in this manager's database.
    pub async fn associate_goal(&self, task_id: u64, goal_id: String) -> Result<TaskRecord> {
        self.store.associate_goal(task_id, goal_id).await
    }

    /// Lists non-deleted tasks associated with a Goal, including dependency edges.
    pub async fn list_for_goal(&self, goal_id: &str) -> Result<Vec<TaskRecord>> {
        self.store.list_for_goal(goal_id).await
    }

    /// Reads a task by ID, including its dependency edges.
    pub async fn get(&self, task_id: u64) -> Result<TaskRecord> {
        self.store.get(task_id).await
    }

    /// Updates the task with the given ID using the given update.
    pub async fn update(&self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord> {
        self.store.update(task_id, update).await
    }

    /// Lists all tasks in the manager.
    pub async fn list(&self) -> Result<Vec<TaskRecord>> {
        self.store.list().await
    }

    /// Deletes the task with the given ID.
    pub async fn delete(&self, task_id: u64) -> Result<TaskRecord> {
        self.store.delete(task_id).await
    }
}

/// Thread-safe wrapper around [`TaskManager`].
///
/// The SQLite pool inside the task store already serializes writes, so no
/// extra mutex is needed.
#[derive(Clone)]
pub struct SharedTaskManager {
    inner: Arc<TaskManager>,
}

impl SharedTaskManager {
    /// Creates a new shared task manager with the given task manager.
    pub fn new(manager: TaskManager) -> Self {
        Self {
            inner: Arc::new(manager),
        }
    }

    /// Creates a new task in the manager.
    pub async fn create(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
    ) -> Result<TaskRecord> {
        self.inner.create(subject, description, session_id).await
    }

    /// Creates a new task associated with a Goal in the shared manager's database.
    pub async fn create_for_goal(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
        goal_id: String,
    ) -> Result<TaskRecord> {
        self.inner
            .create_for_goal(subject, description, session_id, goal_id)
            .await
    }

    /// Associates a task with a Goal in the shared manager's database.
    pub async fn associate_goal(&self, task_id: u64, goal_id: String) -> Result<TaskRecord> {
        self.inner.associate_goal(task_id, goal_id).await
    }

    /// Lists non-deleted tasks associated with a Goal, including dependency edges.
    pub async fn list_for_goal(&self, goal_id: &str) -> Result<Vec<TaskRecord>> {
        self.inner.list_for_goal(goal_id).await
    }

    /// Reads a task by ID, including its dependency edges.
    pub async fn get(&self, task_id: u64) -> Result<TaskRecord> {
        self.inner.get(task_id).await
    }

    /// Updates a task in the manager.
    pub async fn update(&self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord> {
        self.inner.update(task_id, update).await
    }

    /// Lists all tasks in the manager.
    pub async fn list(&self) -> Result<Vec<TaskRecord>> {
        self.inner.list().await
    }

    /// Deletes a task from the manager.
    pub async fn delete(&self, task_id: u64) -> Result<TaskRecord> {
        self.inner.delete(task_id).await
    }
}

/// Renders a task as pretty-printed JSON.
pub fn render_task_json(task: &TaskRecord) -> Result<String> {
    serde_json::to_string_pretty(task).context("failed to serialize task")
}

/// Renders a list of tasks as a string.
pub fn render_task_list(tasks: Vec<TaskRecord>) -> String {
    if tasks.is_empty() {
        return "No tasks.".to_string();
    }

    tasks
        .into_iter()
        .map(|task| {
            let blocked = if task.blocked_by.is_empty() {
                String::new()
            } else {
                format!(" (blocked by: {:?})", task.blocked_by)
            };
            let owner = if task.owner.is_empty() {
                String::new()
            } else {
                format!(" owner={}", task.owner)
            };
            format!(
                "{} #{}: {}{}{}",
                task.status.marker(),
                task.id,
                task.subject,
                owner,
                blocked
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map store records to TUI snapshots, dropping soft-deleted tasks.
pub fn to_ui_snapshots(tasks: Vec<TaskRecord>) -> Vec<tact_protocol::TaskSnapshot> {
    tasks
        .into_iter()
        .filter(|t| t.status != TaskStatus::Deleted)
        .map(|t| tact_protocol::TaskSnapshot {
            id: t.id,
            subject: t.subject,
            session_id: t.session_id,
            status: match t.status {
                TaskStatus::Pending => tact_protocol::TaskStatusSnapshot::Pending,
                TaskStatus::InProgress => tact_protocol::TaskStatusSnapshot::InProgress,
                TaskStatus::Completed => tact_protocol::TaskStatusSnapshot::Completed,
                TaskStatus::Deleted => unreachable!("filtered above"),
            },
            owner: t.owner,
            blocks: t.blocks,
            blocked_by: t.blocked_by,
            created_at: t.created_at,
            started_at: t.started_at,
            completed_at: t.completed_at,
        })
        .collect()
}

/// Notify the TUI that the persistent task list changed (no-op without `ui_tx`).
pub fn emit_tasks_changed(
    ui_tx: &Option<tokio::sync::mpsc::UnboundedSender<tact_protocol::AgentUpdate>>,
    tasks: Vec<TaskRecord>,
    reason: tact_protocol::TasksChangeReason,
) {
    let Some(tx) = ui_tx else {
        return;
    };
    let _ = tx.send(tact_protocol::AgentUpdate::TasksChanged {
        tasks: to_ui_snapshots(tasks),
        reason,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_manager(name: &str) -> (TaskManager, std::path::PathBuf) {
        let root_dir = std::env::temp_dir().join(format!("tact-task-test-{name}"));
        let _ = std::fs::remove_dir_all(&root_dir);
        std::fs::create_dir_all(&root_dir).unwrap();
        let db = root_dir.join(".tact").join("tact.db");
        (TaskManager::new(&db).await.unwrap(), root_dir)
    }

    #[tokio::test]
    async fn create_assigns_incrementing_ids() {
        let (manager, _dir) = test_manager("create_assigns_incrementing_ids").await;

        let first = manager
            .create("First".to_string(), None, String::new())
            .await
            .unwrap();
        let second = manager
            .create(
                "Second".to_string(),
                Some("details".to_string()),
                String::new(),
            )
            .await
            .unwrap();

        assert_eq!(first.id, 1);
        assert_eq!(first.subject, "First");
        assert_eq!(first.status, TaskStatus::Pending);
        assert_eq!(second.id, 2);
        assert_eq!(second.description.as_deref(), Some("details"));
    }

    #[tokio::test]
    async fn update_changes_status_and_owner() {
        let (manager, _dir) = test_manager("update_changes_status_and_owner").await;
        let task = manager
            .create("Work".to_string(), None, String::new())
            .await
            .unwrap();

        let updated = manager
            .update(
                task.id,
                TaskUpdate {
                    status: Some(TaskStatus::InProgress),
                    owner: Some("alice".to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.status, TaskStatus::InProgress);
        assert_eq!(updated.owner, "alice");
    }

    #[tokio::test]
    async fn update_add_blocks_creates_reverse_dependency() {
        let (manager, _dir) = test_manager("update_add_blocks_creates_reverse_dependency").await;
        let blocker = manager
            .create("Blocker".to_string(), None, String::new())
            .await
            .unwrap();
        let blocked = manager
            .create("Blocked".to_string(), None, String::new())
            .await
            .unwrap();

        let updated = manager
            .update(
                blocker.id,
                TaskUpdate {
                    add_blocks: vec![blocked.id],
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.blocks, vec![blocked.id]);

        let blocked = manager.get(blocked.id).await.unwrap();
        assert_eq!(blocked.blocked_by, vec![blocker.id]);
    }

    #[tokio::test]
    async fn update_add_blocked_by_creates_reverse_outgoing_edge() {
        let (manager, _dir) =
            test_manager("update_add_blocked_by_creates_reverse_outgoing_edge").await;
        let blocker = manager
            .create("Blocker".to_string(), None, String::new())
            .await
            .unwrap();
        let blocked = manager
            .create("Blocked".to_string(), None, String::new())
            .await
            .unwrap();

        let updated = manager
            .update(
                blocked.id,
                TaskUpdate {
                    add_blocked_by: vec![blocker.id],
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(updated.blocked_by, vec![blocker.id]);

        // Mirror: the blocker must gain an outgoing edge so /tasks-dag
        // renders `T{blocker} --> T{blocked}`.
        let blocker = manager.get(blocker.id).await.unwrap();
        assert_eq!(blocker.blocks, vec![blocked.id]);
    }

    #[tokio::test]
    async fn completing_task_clears_blocked_by() {
        let (manager, _dir) = test_manager("completing_task_clears_blocked_by").await;
        let blocker = manager
            .create("Blocker".to_string(), None, String::new())
            .await
            .unwrap();
        let blocked = manager
            .create("Blocked".to_string(), None, String::new())
            .await
            .unwrap();
        manager
            .update(
                blocker.id,
                TaskUpdate {
                    add_blocks: vec![blocked.id],
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        manager
            .update(
                blocker.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let blocked = manager.get(blocked.id).await.unwrap();
        assert!(blocked.blocked_by.is_empty());
        // Mirror: the completed task must not keep ghost outgoing edges.
        let blocker = manager.get(blocker.id).await.unwrap();
        assert!(blocker.blocks.is_empty());
    }

    #[tokio::test]
    async fn render_task_list_empty_and_populated() {
        assert_eq!(render_task_list(vec![]), "No tasks.");

        let task = TaskRecord {
            id: 1,
            subject: "Ship".to_string(),
            description: None,
            session_id: "sess-1".to_string(),
            status: TaskStatus::InProgress,
            blocked_by: vec![2],
            blocks: vec![],
            owner: "bob".to_string(),
            created_at: None,
            started_at: None,
            completed_at: None,
            goal_id: None,
        };
        let rendered = render_task_list(vec![task]);
        assert!(rendered.contains("[>] #1: Ship"));
        assert!(rendered.contains("owner=bob"));
        assert!(rendered.contains("blocked by: [2]"));
    }

    #[tokio::test]
    async fn render_task_json_round_trip() {
        let task = TaskRecord::new(
            1,
            "Test".to_string(),
            Some("desc".to_string()),
            String::new(),
        );
        let json = render_task_json(&task).unwrap();
        assert!(json.contains("\"subject\": \"Test\""));
        assert!(json.contains("\"description\": \"desc\""));
        assert!(json.contains("\"status\": \"pending\""));
        assert!(!json.contains("\"goal_id\""));
        let decoded: TaskRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.goal_id, None);
    }

    #[tokio::test]
    async fn to_ui_snapshots_filters_deleted_and_maps_status() {
        let pending = TaskRecord::new(1, "a".into(), None, String::new());
        let mut active = TaskRecord::new(2, "b".into(), None, String::new());
        active.status = TaskStatus::InProgress;
        let mut gone = TaskRecord::new(3, "c".into(), None, String::new());
        gone.status = TaskStatus::Deleted;
        let snaps = to_ui_snapshots(vec![pending, active, gone]);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, 1);
        assert_eq!(
            snaps[1].status,
            tact_protocol::TaskStatusSnapshot::InProgress
        );
    }
}

#[cfg(test)]
mod task3_association_tests {
    use super::*;
    use crate::goal::{GoalCreate, GoalCriterion, GoalManager};

    fn db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-task-association-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    async fn goal_manager(path: &std::path::Path) -> GoalManager {
        GoalManager::new(path).await.unwrap()
    }

    async fn create_goal(manager: &GoalManager, title: &str) -> crate::goal::Goal {
        manager
            .create(GoalCreate {
                title: title.into(),
                objective: format!("objective for {title}"),
                criteria: vec![GoalCriterion::required("done")],
            })
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn goal_association_requires_existing_goal_and_preserves_task_state() {
        let path = db("unknown-goal");
        let manager = TaskManager::new(&path).await.unwrap();
        let task = manager
            .create("task".into(), None, String::new())
            .await
            .unwrap();
        assert!(
            manager
                .create_for_goal("bad".into(), None, String::new(), "missing".into())
                .await
                .is_err()
        );
        assert_eq!(manager.list().await.unwrap().len(), 1);
        assert!(
            manager
                .associate_goal(task.id, "missing".into())
                .await
                .is_err()
        );
        assert_eq!(manager.get(task.id).await.unwrap().goal_id, None);
    }

    #[tokio::test]
    async fn goal_association_and_filtering_are_validated_and_deterministic() {
        let path = db("association");
        let manager = TaskManager::new(&path).await.unwrap();
        let goals = goal_manager(&path).await;
        let goal = create_goal(&goals, "goal-a").await;
        let other = create_goal(&goals, "goal-b").await;
        let first = manager
            .create_for_goal("first".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
        let second = manager
            .create("second".into(), None, String::new())
            .await
            .unwrap();
        manager
            .associate_goal(second.id, goal.id.clone())
            .await
            .unwrap();
        let deleted = manager
            .create_for_goal("deleted".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
        manager.delete(deleted.id).await.unwrap();
        assert_eq!(first.goal_id.as_deref(), Some(goal.id.as_str()));
        assert_eq!(
            manager
                .list_for_goal(&goal.id)
                .await
                .unwrap()
                .iter()
                .map(|t| t.id)
                .collect::<Vec<_>>(),
            vec![first.id, second.id]
        );
        assert!(
            manager
                .associate_goal(first.id, other.id.clone())
                .await
                .is_err()
        );
        assert_eq!(
            manager.get(first.id).await.unwrap().goal_id.as_deref(),
            Some(goal.id.as_str())
        );
    }

    #[tokio::test]
    async fn list_for_goal_includes_dependency_edges_and_same_goal_association_is_idempotent() {
        let path = db("edges");
        let manager = TaskManager::new(&path).await.unwrap();
        let goals = goal_manager(&path).await;
        let goal = create_goal(&goals, "goal").await;
        let blocker = manager
            .create_for_goal("blocker".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
        let blocked = manager
            .create_for_goal("blocked".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
        manager
            .update(
                blocker.id,
                TaskUpdate {
                    add_blocks: vec![blocked.id],
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(
            manager
                .associate_goal(blocker.id, goal.id.clone())
                .await
                .unwrap()
                .goal_id
                .as_deref(),
            Some(goal.id.as_str())
        );
        let listed = manager.list_for_goal(&goal.id).await.unwrap();
        assert_eq!(listed[0].blocks, vec![blocked.id]);
        assert_eq!(listed[1].blocked_by, vec![blocker.id]);
    }
}
