//! SQLite-backed task store.
//!
//! Tasks live in the same `tact.db` as sessions, under `tasks` /
//! `task_dependencies` tables. Dependency edges are stored as rows
//! (no mirrored fields), and every mutation runs inside one transaction.
//!
//! Which `BEGIN` that transaction uses depends on its first statement: the
//! ones that read before writing (`update`, and the Goal-linked creates that
//! validate a Goal row first) open with `BEGIN IMMEDIATE` so a concurrent
//! writer is waited out; the ones that write first (`create`, `delete`) keep
//! the deferred `begin()`. See `store::sqlite` for why the difference matters.

pub use sqlite::SqliteTaskStore;

use anyhow::Result;
use async_trait::async_trait;

use crate::task::{TaskRecord, TaskUpdate};

/// Storage backend contract for the task manager.
#[allow(clippy::double_must_use)] // async_trait generates #[must_use] on Pin<Box<dyn Future>> which is already #[must_use]
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Creates a new task and returns the stored record with its assigned id.
    async fn create(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
    ) -> Result<TaskRecord>;

    /// Creates a task associated with a validated Goal.
    async fn create_for_goal(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
        goal_id: String,
    ) -> Result<TaskRecord>;

    /// Associates a task with a validated Goal.
    async fn associate_goal(&self, task_id: u64, goal_id: String) -> Result<TaskRecord>;

    /// Lists non-deleted tasks for a Goal, including dependency edges.
    async fn list_for_goal(&self, goal_id: &str) -> Result<Vec<TaskRecord>>;

    /// Reads a task by id, including its dependency edges.
    async fn get(&self, task_id: u64) -> Result<TaskRecord>;

    /// Applies a mutable update in one transaction and returns the stored
    /// record with refreshed dependency edges.
    async fn update(&self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord>;

    /// Lists all tasks sorted by id.
    async fn list(&self) -> Result<Vec<TaskRecord>>;

    /// Soft-deletes the task (status → Deleted).
    async fn delete(&self, task_id: u64) -> Result<TaskRecord>;
}

mod sqlite;
