//! SQLite-backed subagent run store.
//!
//! Subagent runs live in the same `tact.db` as sessions, tasks, and background
//! tasks, under the `subagent_runs` table. Unlike `background_run` (a command
//! the parent polls via `check_background`), a subagent summary must flow back
//! into the parent's conversation, so the run record is the crash-recovery
//! source of truth: orphan repair rewrites any `running` row to `failed` on
//! startup, and a finished `summary` lets the model `resume` a child or decide
//! to spawn a fresh one.

pub use sqlite::SqliteSubagentStore;

use anyhow::Result;
use async_trait::async_trait;

use crate::subagent::SubagentRun;

/// Storage backend contract for the subagent manager.
#[async_trait]
pub trait SubagentStore: Send + Sync {
    /// Inserts or replaces a record by child session id.
    async fn upsert(&self, record: &SubagentRun) -> Result<()>;

    /// Reads a record by child session id (`None` when the id is unknown).
    async fn get(&self, id: &str) -> Result<Option<SubagentRun>>;

    /// Lists all records sorted by start time.
    async fn list(&self) -> Result<Vec<SubagentRun>>;

    /// Lists records still marked `running` (orphan-repair input).
    async fn list_running(&self) -> Result<Vec<SubagentRun>>;
}

mod sqlite;
