//! Durable task management.
//!
//! Tasks are persistent work items with status (Pending → InProgress →
//! Completed/Deleted), blocking relationships, and optional owners.
//!
//! - [`TaskManager`] is the core state machine backed by a file store.
//! - [`SharedTaskManager`] wraps it in `Arc<Mutex<…>>` for concurrent
//!   access from tools.
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

    /// Gets the task with the given ID.
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

    /// Gets a task from the manager.
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

/// Renders a task as JSON.
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
