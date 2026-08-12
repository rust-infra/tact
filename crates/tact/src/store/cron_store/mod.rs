//! SQLite-backed cron store.
//!
//! Scheduled tasks live in the same `tact.db` as sessions and tasks, under
//! the `cron_tasks` table. IDs are `INTEGER PRIMARY KEY AUTOINCREMENT`
//! exposed to callers as 8-hex-digit strings (same wire contract as the
//! legacy JSON index), and every mutation runs in a single transaction.

pub use sqlite::SqliteCronStore;

use anyhow::Result;
use async_trait::async_trait;

/// A stored scheduled-task row.
#[derive(Debug, Clone)]
pub struct CronTaskRecord {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    pub session_id: String,
    pub created_at: i64,
}

/// Storage backend contract for the cron scheduler.
#[async_trait]
pub trait CronStore: Send + Sync {
    /// Creates a new scheduled task and returns the stored record with its
    /// assigned id.
    async fn create(
        &self,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
        session_id: String,
    ) -> Result<CronTaskRecord>;

    /// Deletes a scheduled task by id. Returns `true` when a row was
    /// removed, `false` when the id did not exist.
    async fn delete(&self, id: &str) -> Result<bool>;

    /// Lists all scheduled tasks sorted by id.
    async fn list(&self) -> Result<Vec<CronTaskRecord>>;
}

mod sqlite;
