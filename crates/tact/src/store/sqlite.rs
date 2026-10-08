//! Shared SQLite pool for the domain stores.
//!
//! All domain stores (sessions, tasks, background, team, worktrees)
//! live in the same `<workdir>/.tact/tact.db`. This module owns the
//! open-or-create + busy-timeout boilerplate and caches **one**
//! `SqlitePool` per database file, shared by every store in the program.
//!
//! Pools are reference-counted: the registry drops a pool when the last
//! store holding it is dropped, so long test runs that create many
//! temporary databases do not leak file descriptors.

use chrono::{DateTime, Utc};
pub use tact_session::{PoolRef, open_pool};

/// Now, in the milliseconds every domain store persists time in.
///
/// The stores share one database and, in places, one row's timestamp — a store
/// that encoded time differently would be writable but unreadable by the
/// others, so the unit is stated once, here.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// A persisted millisecond timestamp as a `DateTime`.
///
/// This is also the one place that decides what an *unrepresentable* value
/// means: `Utc::now()` rather than an error, because the column is a display
/// field and a row whose timestamp cannot be read is still a row the panel has
/// to show. Three stores read timestamps back, so the alternative was three
/// answers to that question.
pub fn from_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or_else(Utc::now)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Deref;
    use std::path::Path;

    use crate::store::session_store::SqliteSessionStore;
    use crate::store::task_store::SqliteTaskStore;
    use crate::store::team_store::SqliteTeamStore;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-pool-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Pools cached for databases under `dir`. Other tests run in parallel
    /// and open pools for their own temp dirs, so the global map is only
    /// inspected through this per-directory view.
    fn pool_count_under(dir: &Path) -> usize {
        tact_session::pool_count_under(dir)
    }

    #[tokio::test]
    async fn same_path_returns_the_same_pool() {
        let dir = temp_dir("same_path");
        let db = dir.join("tact.db");
        let a = open_pool(&db).await.unwrap();
        let b = open_pool(&db).await.unwrap();
        let c = open_pool(&db).await.unwrap();
        assert_eq!(pool_count_under(&dir), 1);
        drop(a);
        drop(b);
        drop(c);
        assert_eq!(pool_count_under(&dir), 0);
    }

    #[tokio::test]
    async fn different_paths_get_their_own_pool() {
        let dir = temp_dir("different_paths");
        let a = open_pool(&dir.join("a.db")).await.unwrap();
        let b = open_pool(&dir.join("b.db")).await.unwrap();
        assert_eq!(pool_count_under(&dir), 2);
        drop(a);
        drop(b);
        assert_eq!(pool_count_under(&dir), 0);
    }

    #[tokio::test]
    async fn domain_stores_share_one_pool() {
        let dir = temp_dir("domain_stores");
        let db = dir.join("tact.db");
        {
            let _task = SqliteTaskStore::new(&db).await.unwrap();
            let _team = SqliteTeamStore::new(&db).await.unwrap();
            let _session = SqliteSessionStore::new(&db).await.unwrap();
            assert_eq!(pool_count_under(&dir), 1);
        }
        assert_eq!(pool_count_under(&dir), 0);
    }

    /// The TUI reads session history while the agent writes it; without WAL
    /// those lock each other out.
    #[tokio::test]
    async fn opened_pools_use_wal() {
        let dir = temp_dir("wal");
        let pool = open_pool(&dir.join("tact.db")).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool.deref())
            .await
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    /// `synchronous` is per-connection, so it is set through the connection
    /// options rather than a one-shot `PRAGMA` that later connections miss.
    #[tokio::test]
    async fn wal_pools_downgrade_synchronous() {
        let dir = temp_dir("synchronous");
        let pool = open_pool(&dir.join("tact.db")).await.unwrap();
        // NORMAL == 1; the rollback-journal default would be FULL == 2.
        let level: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(pool.deref())
            .await
            .unwrap();
        assert_eq!(level, 1);
    }

    #[test]
    fn a_persisted_timestamp_round_trips() {
        let millis = 1_700_000_000_000;
        assert_eq!(from_millis(millis).timestamp_millis(), millis);
        assert!(now_millis() > millis, "the clock is past 2023");
    }

    /// The one policy in this pair: a value that is not a representable instant
    /// becomes "now" rather than failing, because the column is a display field
    /// and a row nobody can date is still a row the panel has to show.
    #[test]
    fn an_unrepresentable_timestamp_becomes_now() {
        let before = Utc::now();
        let fallback = from_millis(i64::MAX);
        assert!(
            fallback >= before && fallback <= Utc::now(),
            "{fallback} is not between {before} and now"
        );
    }
}
