//! SQLite-backed worktree store.
//!
//! Worktree metadata and the lifecycle audit log live in the same
//! `tact.db` as sessions, tasks, cron, background, and team state, under
//! the `worktrees` / `worktree_events` tables. The autoincrement `id` on
//! `worktrees` preserves insertion order (the legacy `index.json` vector
//! semantics); `worktree_events` rows are ordered by their own `id`.

pub use sqlite::SqliteWorktreeStore;

use anyhow::Result;
use async_trait::async_trait;

use crate::worktree::WorktreeRecord;

/// Storage backend contract for the worktree manager.
#[async_trait]
pub trait WorktreeStore: Send + Sync {
    /// Inserts a worktree record. Returns `false` when a worktree with the
    /// same name already exists (nothing is inserted).
    async fn create_worktree(&self, record: &WorktreeRecord, session_id: &str) -> Result<bool>;

    /// Reads a worktree by name.
    async fn find_worktree(&self, name: &str) -> Result<Option<WorktreeRecord>>;

    /// Lists all worktrees in insertion order.
    async fn list_worktrees(&self) -> Result<Vec<WorktreeRecord>>;

    /// Appends one audit-log event.
    async fn append_event(&self, event: &str) -> Result<()>;

    /// Returns the most recent `limit` events in chronological order.
    async fn recent_events(&self, limit: usize) -> Result<Vec<String>>;
}

mod sqlite;
