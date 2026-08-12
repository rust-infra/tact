use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::{Row, SqlitePool};

use super::{CronStore, CronTaskRecord};

/// SQLite-backed [`CronStore`] implementation.
///
/// Shares `tact.db` with the session / task / background stores. Schema:
///
/// - `cron_tasks(id, cron, prompt, recurring, durable, session_id,
///   created_at)` — one row per scheduled prompt, `INTEGER PRIMARY KEY
///   AUTOINCREMENT` ids surfaced as 8-hex-digit strings.
pub struct SqliteCronStore {
    pool: SqlitePool,
}

impl SqliteCronStore {
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
            CREATE TABLE IF NOT EXISTS cron_tasks (
                id         INTEGER PRIMARY KEY AUTOINCREMENT,
                cron       TEXT    NOT NULL,
                prompt     TEXT    NOT NULL,
                recurring  INTEGER NOT NULL DEFAULT 0,
                durable    INTEGER NOT NULL DEFAULT 0,
                session_id TEXT    NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create cron_tasks table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_cron_tasks_session_id ON cron_tasks(session_id);",
        )
        .execute(&pool)
        .await
        .context("failed to create cron_tasks session_id index")?;

        Ok(Self { pool })
    }

    /// Parses an 8-hex-digit public id back into the row id.
    fn parse_id(id: &str) -> Option<i64> {
        i64::from_str_radix(id, 16).ok()
    }
}

#[async_trait]
impl CronStore for SqliteCronStore {
    async fn create(
        &self,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
        session_id: String,
    ) -> Result<CronTaskRecord> {
        let mut tx = self.pool.begin().await?;
        let rowid = sqlx::query(
            "INSERT INTO cron_tasks (cron, prompt, recurring, durable, session_id, created_at)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(&cron)
        .bind(&prompt)
        .bind(i64::from(recurring))
        .bind(i64::from(durable))
        .bind(&session_id)
        .bind(now_millis())
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        tx.commit().await?;
        Ok(CronTaskRecord {
            id: format!("{rowid:08x}"),
            cron,
            prompt,
            recurring,
            durable,
            session_id,
            created_at: now_millis(),
        })
    }

    async fn delete(&self, id: &str) -> Result<bool> {
        let Some(rowid) = Self::parse_id(id) else {
            return Ok(false);
        };
        let affected = sqlx::query("DELETE FROM cron_tasks WHERE id = ?")
            .bind(rowid)
            .execute(&self.pool)
            .await?
            .rows_affected();
        Ok(affected > 0)
    }

    async fn list(&self) -> Result<Vec<CronTaskRecord>> {
        let rows = sqlx::query(
            "SELECT id, cron, prompt, recurring, durable, session_id, created_at
             FROM cron_tasks ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?;
        rows.iter()
            .map(|row| {
                Ok(CronTaskRecord {
                    id: format!("{:08x}", row.try_get::<i64, _>("id")?),
                    cron: row.try_get("cron")?,
                    prompt: row.try_get("prompt")?,
                    recurring: row.try_get::<i64, _>("recurring")? != 0,
                    durable: row.try_get::<i64, _>("durable")? != 0,
                    session_id: row.try_get("session_id")?,
                    created_at: row.try_get("created_at")?,
                })
            })
            .collect()
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
        let dir = std::env::temp_dir().join(format!("tact-cronstore-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    #[tokio::test]
    async fn create_round_trips_fields_and_session_id() {
        let db = temp_db("round_trip");
        let store = SqliteCronStore::new(&db).await.unwrap();
        let task = store
            .create(
                "0 9 * * *".into(),
                "daily standup".into(),
                true,
                true,
                "sess-1".into(),
            )
            .await
            .unwrap();
        assert_eq!(task.id, "00000001");
        assert_eq!(task.cron, "0 9 * * *");
        assert!(task.recurring);
        assert!(task.durable);
        assert_eq!(task.session_id, "sess-1");

        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "00000001");
        assert_eq!(tasks[0].prompt, "daily standup");
        assert_eq!(tasks[0].session_id, "sess-1");
    }

    #[tokio::test]
    async fn store_is_persistent_across_reopen() {
        let db = temp_db("persistent");
        {
            let store = SqliteCronStore::new(&db).await.unwrap();
            store
                .create("a".into(), "one".into(), false, false, String::new())
                .await
                .unwrap();
            store
                .create("b".into(), "two".into(), true, false, "s1".into())
                .await
                .unwrap();
        }
        let store = SqliteCronStore::new(&db).await.unwrap();
        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "00000001");
        assert_eq!(tasks[1].id, "00000002");
        assert_eq!(tasks[1].session_id, "s1");
    }

    #[tokio::test]
    async fn autoincrement_ids_continue_after_delete() {
        let db = temp_db("autoincrement");
        let store = SqliteCronStore::new(&db).await.unwrap();
        let a = store
            .create("a".into(), "one".into(), false, false, String::new())
            .await
            .unwrap();
        let b = store
            .create("b".into(), "two".into(), false, false, String::new())
            .await
            .unwrap();
        assert_eq!(a.id, "00000001");
        assert_eq!(b.id, "00000002");
        assert!(store.delete(&a.id).await.unwrap());
        let c = store
            .create("c".into(), "three".into(), false, false, String::new())
            .await
            .unwrap();
        assert_eq!(c.id, "00000003");
    }

    #[tokio::test]
    async fn delete_missing_or_invalid_id_returns_false() {
        let db = temp_db("delete_missing");
        let store = SqliteCronStore::new(&db).await.unwrap();
        assert!(!store.delete("00000099").await.unwrap());
        assert!(!store.delete("not-hex").await.unwrap());
    }
}
