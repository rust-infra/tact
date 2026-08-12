use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::{Row, SqlitePool};

use crate::team::{InboxMessage, TeammateRecord};

use super::TeamStore;

/// SQLite-backed [`TeamStore`] implementation.
///
/// Shares `tact.db` with the session / task / cron / background stores.
/// Schema:
///
/// - `teammates(name, role, status)` — `name` is the PRIMARY KEY; spawning
///   a duplicate name fails with a UNIQUE constraint.
/// - `inbox_messages(id, owner, from_name, to_name, body, kind,
///   created_at)` — one row per inbox entry, autoincrement `id` preserves
///   insertion order (the legacy JSONL append semantics).
pub struct SqliteTeamStore {
    pool: SqlitePool,
}

impl SqliteTeamStore {
    pub async fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create database directory")?;
        }
        // sqlx may fail to open a non-existent database file in some environments;
        // create an empty file first to ensure it's present.
        if let Err(e) = tokio::fs::metadata(path).await
            && e.kind() == std::io::ErrorKind::NotFound
        {
            tokio::fs::File::create(path)
                .await
                .context("failed to create database file")?;
        }
        let url = format!("sqlite:{}", path.display());
        let pool = SqlitePool::connect(&url)
            .await
            .with_context(|| format!("failed to open sqlite database at {}", path.display()))?;

        // Wait up to 5s for a concurrent writer (cross-process access to the
        // same workdir) instead of failing with SQLITE_BUSY immediately.
        sqlx::query("PRAGMA busy_timeout = 5000")
            .execute(&pool)
            .await
            .context("failed to set busy_timeout")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS teammates (
                name   TEXT PRIMARY KEY NOT NULL,
                role   TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'idle'
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create teammates table")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS inbox_messages (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                owner      TEXT NOT NULL,
                from_name  TEXT NOT NULL,
                to_name    TEXT NOT NULL,
                body       TEXT NOT NULL,
                kind       TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create inbox_messages table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_inbox_messages_owner ON inbox_messages(owner);",
        )
        .execute(&pool)
        .await
        .context("failed to create inbox_messages owner index")?;

        Ok(Self { pool })
    }
}

fn from_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or_else(Utc::now)
}

fn row_to_message(row: &sqlx::sqlite::SqliteRow) -> Result<InboxMessage> {
    Ok(InboxMessage {
        from: row.try_get("from_name")?,
        to: row.try_get("to_name")?,
        body: row.try_get("body")?,
        kind: row.try_get("kind")?,
        created_at: from_millis(row.try_get("created_at")?),
    })
}

#[async_trait]
impl TeamStore for SqliteTeamStore {
    async fn create_teammate(&self, name: String, role: String) -> Result<()> {
        // INSERT OR IGNORE + rows_affected: a duplicate name (including a
        // concurrent one) reports zero rows instead of surfacing the raw
        // UNIQUE-constraint error.
        let affected = sqlx::query("INSERT OR IGNORE INTO teammates (name, role) VALUES (?, ?)")
            .bind(&name)
            .bind(&role)
            .execute(&self.pool)
            .await?
            .rows_affected();
        if affected == 0 {
            anyhow::bail!("teammate {name} already exists");
        }
        Ok(())
    }

    async fn list_teammates(&self) -> Result<Vec<TeammateRecord>> {
        let rows = sqlx::query("SELECT name, role, status FROM teammates")
            .fetch_all(&self.pool)
            .await?;
        rows.iter()
            .map(|row| {
                Ok(TeammateRecord {
                    name: row.try_get("name")?,
                    role: row.try_get("role")?,
                    status: row.try_get("status")?,
                })
            })
            .collect()
    }

    async fn append_message(&self, owner: &str, message: &InboxMessage) -> Result<()> {
        sqlx::query(
            "INSERT INTO inbox_messages (owner, from_name, to_name, body, kind, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(owner)
        .bind(&message.from)
        .bind(&message.to)
        .bind(&message.body)
        .bind(&message.kind)
        .bind(message.created_at.timestamp_millis())
        .execute(&self.pool)
        .await
        .context("failed to append inbox message")?;
        Ok(())
    }

    async fn read_inbox(&self, owner: &str) -> Result<Vec<InboxMessage>> {
        let rows = sqlx::query(
            "SELECT from_name, to_name, body, kind, created_at
             FROM inbox_messages WHERE owner = ? ORDER BY id",
        )
        .bind(owner)
        .fetch_all(&self.pool)
        .await?;
        rows.iter().map(row_to_message).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-teamstore-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    fn message(body: &str) -> InboxMessage {
        InboxMessage {
            from: "lead".to_string(),
            to: "alice".to_string(),
            body: body.to_string(),
            kind: "message".to_string(),
            created_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_teammate_round_trips() {
        let db = temp_db("round_trip");
        let store = SqliteTeamStore::new(&db).await.unwrap();
        store
            .create_teammate("alice".into(), "reviewer".into())
            .await
            .unwrap();
        let teammates = store.list_teammates().await.unwrap();
        assert_eq!(teammates.len(), 1);
        assert_eq!(teammates[0].name, "alice");
        assert_eq!(teammates[0].role, "reviewer");
        assert_eq!(teammates[0].status, "idle");
    }

    #[tokio::test]
    async fn duplicate_teammate_name_is_rejected() {
        let db = temp_db("duplicate");
        let store = SqliteTeamStore::new(&db).await.unwrap();
        store
            .create_teammate("alice".into(), "reviewer".into())
            .await
            .unwrap();
        let err = store
            .create_teammate("alice".into(), "other".into())
            .await
            .unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
    }

    #[tokio::test]
    async fn inbox_appends_in_insertion_order() {
        let db = temp_db("inbox_order");
        let store = SqliteTeamStore::new(&db).await.unwrap();
        store
            .append_message("alice", &message("first"))
            .await
            .unwrap();
        store
            .append_message("alice", &message("second"))
            .await
            .unwrap();
        store
            .append_message("bob", &message("bob-only"))
            .await
            .unwrap();

        let inbox = store.read_inbox("alice").await.unwrap();
        assert_eq!(inbox.len(), 2);
        assert_eq!(inbox[0].body, "first");
        assert_eq!(inbox[1].body, "second");
        assert_eq!(inbox[0].from, "lead");
        assert_eq!(inbox[0].kind, "message");
        assert_eq!(inbox[0].to, "alice");

        // Unknown owner reads empty.
        assert!(store.read_inbox("nobody").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn store_is_persistent_across_reopen() {
        let db = temp_db("persistent");
        {
            let store = SqliteTeamStore::new(&db).await.unwrap();
            store
                .create_teammate("alice".into(), "reviewer".into())
                .await
                .unwrap();
            store
                .append_message("alice", &message("hello"))
                .await
                .unwrap();
        }
        let store = SqliteTeamStore::new(&db).await.unwrap();
        let teammates = store.list_teammates().await.unwrap();
        assert_eq!(teammates.len(), 1);
        let inbox = store.read_inbox("alice").await.unwrap();
        assert_eq!(inbox.len(), 1);
        assert_eq!(inbox[0].body, "hello");
    }
}
