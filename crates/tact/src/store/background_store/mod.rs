//! SQLite-backed background task store.
//!
//! Background tasks live in the same `tact.db` as sessions and tasks, under
//! the `background_tasks` table. The database is the single source of truth:
//! there is no in-memory mirror (unlike the legacy JSON collection store).

pub use sqlite::SqliteBackgroundStore;

use anyhow::Result;
use async_trait::async_trait;

use crate::background::BackgroundTaskRecord;

/// Storage backend contract for the background manager.
#[async_trait]
pub trait BackgroundStore: Send + Sync {
    /// Inserts or replaces a record by id.
    async fn upsert(&self, record: &BackgroundTaskRecord) -> Result<()>;

    /// Reads a record by id (`None` when the id is unknown).
    async fn get(&self, id: &str) -> Result<Option<BackgroundTaskRecord>>;

    /// Lists all records sorted by start time.
    async fn list(&self) -> Result<Vec<BackgroundTaskRecord>>;
}

mod sqlite;
