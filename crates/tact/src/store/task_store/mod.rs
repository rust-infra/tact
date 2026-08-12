//! SQLite-backed task store.
//!
//! Tasks live in the same `tact.db` as sessions, under `tasks` /
//! `task_dependencies` tables. Dependency edges are stored as rows
//! (no mirrored fields), and every mutation runs inside a single
//! `BEGIN IMMEDIATE` transaction.

pub use sqlite::SqliteTaskStore;

use anyhow::Result;
use async_trait::async_trait;

use crate::task::{TaskRecord, TaskUpdate};

/// Storage backend contract for the task manager.
#[async_trait]
pub trait TaskStore: Send + Sync {
    /// Creates a new task and returns the stored record with its assigned id.
    async fn create(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
    ) -> Result<TaskRecord>;

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
