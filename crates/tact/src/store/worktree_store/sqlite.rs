use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::Row;

use crate::store::sqlite::{PoolRef, open_pool};
use crate::worktree::WorktreeRecord;

use super::WorktreeStore;

/// SQLite-backed [`WorktreeStore`] implementation.
///
/// Shares `tact.db` with the session / task / background / team
/// stores. Schema:
///
/// - `worktrees(id, name, path, branch, task_id, status, session_id,
///   created_at)` — `name` is UNIQUE; the autoincrement `id` preserves
///   insertion order.
/// - `worktree_events(id, event, created_at)` — the audit log; `id` is the
///   ordering key.
pub struct SqliteWorktreeStore {
    pool: PoolRef,
}

impl SqliteWorktreeStore {
    pub async fn new(path: &Path) -> Result<Self> {
        let pool = open_pool(path).await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS worktrees (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                name       TEXT    NOT NULL UNIQUE,
                path       TEXT    NOT NULL,
                branch     TEXT    NOT NULL,
                task_id    INTEGER,
                status     TEXT    NOT NULL DEFAULT 'active',
                session_id TEXT    NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&*pool)
        .await
        .context("failed to create worktrees table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_worktrees_session_id ON worktrees(session_id);",
        )
        .execute(&*pool)
        .await
        .context("failed to create worktrees session_id index")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS worktree_events (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                event      TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&*pool)
        .await
        .context("failed to create worktree_events table")?;

        Ok(Self { pool })
    }
}

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> Result<WorktreeRecord> {
    Ok(WorktreeRecord {
        name: row.try_get("name")?,
        path: row.try_get("path")?,
        branch: row.try_get("branch")?,
        task_id: row.try_get("task_id")?,
        status: row.try_get("status")?,
    })
}

#[async_trait]
impl WorktreeStore for SqliteWorktreeStore {
    async fn create_worktree(&self, record: &WorktreeRecord, session_id: &str) -> Result<bool> {
        let affected = sqlx::query(
            "INSERT OR IGNORE INTO worktrees (name, path, branch, task_id, status, session_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&record.name)
        .bind(&record.path)
        .bind(&record.branch)
        .bind(record.task_id.map(|id| id as i64))
        .bind(&record.status)
        .bind(session_id)
        .bind(now_millis())
        .execute(&*self.pool)
        .await?
        .rows_affected();
        Ok(affected > 0)
    }

    async fn find_worktree(&self, name: &str) -> Result<Option<WorktreeRecord>> {
        let row =
            sqlx::query("SELECT name, path, branch, task_id, status FROM worktrees WHERE name = ?")
                .bind(name)
                .fetch_optional(&*self.pool)
                .await?;
        row.map(|row| row_to_record(&row)).transpose()
    }

    async fn list_worktrees(&self) -> Result<Vec<WorktreeRecord>> {
        let rows =
            sqlx::query("SELECT name, path, branch, task_id, status FROM worktrees ORDER BY id")
                .fetch_all(&*self.pool)
                .await?;
        rows.iter().map(row_to_record).collect()
    }

    async fn append_event(&self, event: &str) -> Result<()> {
        sqlx::query("INSERT INTO worktree_events (event, created_at) VALUES (?, ?)")
            .bind(event)
            .bind(now_millis())
            .execute(&*self.pool)
            .await
            .context("failed to append worktree event")?;
        Ok(())
    }

    async fn recent_events(&self, limit: usize) -> Result<Vec<String>> {
        let rows = sqlx::query("SELECT event FROM worktree_events ORDER BY id DESC LIMIT ?")
            .bind(limit as i64)
            .fetch_all(&*self.pool)
            .await?;
        let mut events = rows
            .iter()
            .map(|row| row.try_get::<String, _>("event"))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        events.reverse();
        Ok(events)
    }
}

fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-wtstore-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    fn record(name: &str, task_id: Option<u64>) -> WorktreeRecord {
        WorktreeRecord {
            name: name.to_string(),
            path: format!("/tmp/{name}"),
            branch: format!("wt/{name}"),
            task_id,
            status: "active".to_string(),
        }
    }

    #[tokio::test]
    async fn create_find_list_round_trip() {
        let db = temp_db("round_trip");
        let store = SqliteWorktreeStore::new(&db).await.unwrap();
        assert!(
            store
                .create_worktree(&record("a", None), "sess-1")
                .await
                .unwrap()
        );
        assert!(
            store
                .create_worktree(&record("b", Some(42)), "")
                .await
                .unwrap()
        );

        let found = store.find_worktree("a").await.unwrap().unwrap();
        assert_eq!(found.name, "a");
        assert_eq!(found.branch, "wt/a");

        let list = store.list_worktrees().await.unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "a");
        assert_eq!(list[1].task_id, Some(42));
    }

    #[tokio::test]
    async fn duplicate_name_is_rejected() {
        let db = temp_db("duplicate");
        let store = SqliteWorktreeStore::new(&db).await.unwrap();
        assert!(store.create_worktree(&record("a", None), "").await.unwrap());
        assert!(
            !store
                .create_worktree(&record("a", Some(1)), "")
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn events_return_most_recent_in_order() {
        let db = temp_db("events");
        let store = SqliteWorktreeStore::new(&db).await.unwrap();
        for i in 0..5 {
            store.append_event(&format!("event {i}")).await.unwrap();
        }
        let events = store.recent_events(3).await.unwrap();
        assert_eq!(events, vec!["event 2", "event 3", "event 4"]);
    }

    #[tokio::test]
    async fn store_is_persistent_across_reopen() {
        let db = temp_db("persistent");
        {
            let store = SqliteWorktreeStore::new(&db).await.unwrap();
            store
                .create_worktree(&record("a", None), "sess-9")
                .await
                .unwrap();
            store.append_event("created").await.unwrap();
        }
        let store = SqliteWorktreeStore::new(&db).await.unwrap();
        assert_eq!(store.list_worktrees().await.unwrap().len(), 1);
        assert_eq!(store.recent_events(10).await.unwrap(), vec!["created"]);
    }
}
