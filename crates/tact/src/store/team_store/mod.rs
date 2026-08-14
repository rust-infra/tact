//! SQLite-backed team store.
//!
//! Teammates and inbox messages live in the same `tact.db` as sessions,
//! tasks and background work, under the `teammates` /
//! `inbox_messages` tables. `name` is the natural primary key of a
//! teammate; messages are ordered by their autoincrement id (insertion
//! order, matching the legacy JSONL append semantics).

pub use sqlite::SqliteTeamStore;

use anyhow::Result;
use async_trait::async_trait;

use crate::team::{InboxMessage, TeammateRecord};

/// Storage backend contract for the teammate manager.
#[async_trait]
pub trait TeamStore: Send + Sync {
    /// Creates a teammate. Errors when a teammate with the same name
    /// already exists.
    async fn create_teammate(&self, name: String, role: String) -> Result<()>;

    /// Lists all teammates (unspecified order; the manager sorts).
    async fn list_teammates(&self) -> Result<Vec<TeammateRecord>>;

    /// Appends one message to an owner's inbox.
    async fn append_message(&self, owner: &str, message: &InboxMessage) -> Result<()>;

    /// Reads an owner's inbox in insertion order.
    async fn read_inbox(&self, owner: &str) -> Result<Vec<InboxMessage>>;
}

mod sqlite;
