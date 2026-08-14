use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::Row;

use crate::background::{BackgroundTaskRecord, BackgroundTaskStatus};
use crate::store::sqlite::{PoolRef, open_pool};

use super::BackgroundStore;

/// SQLite-backed [`BackgroundStore`] implementation.
///
/// Shares `tact.db` with the session / task stores. Schema:
///
/// - `background_tasks(id, status, command, session_id, started_at,
///   finished_at, output, output_path)` — `id` is the timestamp-millis hex
///   string used by the manager, `status` is CHECK-constrained to
///   `running` / `completed` / `error`, timestamps are epoch millis,
///   `output_path` is the full-output log file (`''` when absent).
pub struct SqliteBackgroundStore {
    pool: PoolRef,
}

impl SqliteBackgroundStore {
    pub async fn new(path: &Path) -> Result<Self> {
        let pool = open_pool(path).await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS background_tasks (
                id          TEXT    PRIMARY KEY,
                status      TEXT    NOT NULL
                            CHECK (status IN ('running','completed','error')),
                command     TEXT    NOT NULL,
                session_id  TEXT    NOT NULL DEFAULT '',
                started_at  INTEGER NOT NULL,
                finished_at INTEGER,
                output      TEXT    NOT NULL DEFAULT '',
                output_path TEXT    NOT NULL DEFAULT ''
            );
            "#,
        )
        .execute(&*pool)
        .await
        .context("failed to create background_tasks table")?;

        // Migration for databases created before `output_path` existed:
        // `CREATE TABLE IF NOT EXISTS` cannot add columns, so check the
        // pragma and ALTER when the column is missing.
        let columns = sqlx::query("PRAGMA table_info(background_tasks)")
            .fetch_all(&*pool)
            .await
            .context("failed to read background_tasks columns")?;
        let has_output_path = columns.iter().any(|row| {
            row.try_get::<String, _>("name")
                .map(|name| name == "output_path")
                .unwrap_or(false)
        });
        if !has_output_path {
            sqlx::query(
                "ALTER TABLE background_tasks ADD COLUMN output_path TEXT NOT NULL DEFAULT ''",
            )
            .execute(&*pool)
            .await
            .context("failed to add background_tasks.output_path column")?;
        }

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_background_tasks_session_id ON background_tasks(session_id);",
        )
        .execute(&*pool)
        .await
        .context("failed to create background_tasks session_id index")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_background_tasks_started_at ON background_tasks(started_at);",
        )
        .execute(&*pool)
        .await
        .context("failed to create background_tasks started_at index")?;

        Ok(Self { pool })
    }
}

fn status_to_str(status: BackgroundTaskStatus) -> &'static str {
    match status {
        BackgroundTaskStatus::Running => "running",
        BackgroundTaskStatus::Completed => "completed",
        BackgroundTaskStatus::Error => "error",
    }
}

fn str_to_status(s: &str) -> Result<BackgroundTaskStatus> {
    match s {
        "running" => Ok(BackgroundTaskStatus::Running),
        "completed" => Ok(BackgroundTaskStatus::Completed),
        "error" => Ok(BackgroundTaskStatus::Error),
        other => anyhow::bail!("invalid background task status in database: {other}"),
    }
}

fn from_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or_else(Utc::now)
}

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> Result<BackgroundTaskRecord> {
    Ok(BackgroundTaskRecord {
        id: row.try_get("id")?,
        status: str_to_status(&row.try_get::<String, _>("status")?)?,
        command: row.try_get("command")?,
        session_id: row.try_get("session_id")?,
        started_at: from_millis(row.try_get("started_at")?),
        finished_at: row
            .try_get::<Option<i64>, _>("finished_at")?
            .map(from_millis),
        output: row.try_get("output")?,
        output_path: {
            let path: String = row.try_get("output_path")?;
            (!path.is_empty()).then_some(path)
        },
    })
}

#[async_trait]
impl BackgroundStore for SqliteBackgroundStore {
    async fn upsert(&self, record: &BackgroundTaskRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO background_tasks
                (id, status, command, session_id, started_at, finished_at, output, output_path)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                status      = excluded.status,
                command     = excluded.command,
                session_id  = excluded.session_id,
                started_at  = excluded.started_at,
                finished_at = excluded.finished_at,
                output      = excluded.output,
                output_path = excluded.output_path
            "#,
        )
        .bind(&record.id)
        .bind(status_to_str(record.status))
        .bind(&record.command)
        .bind(&record.session_id)
        .bind(record.started_at.timestamp_millis())
        .bind(record.finished_at.map(|dt| dt.timestamp_millis()))
        .bind(&record.output)
        .bind(record.output_path.as_deref().unwrap_or(""))
        .execute(&*self.pool)
        .await
        .context("failed to upsert background task")?;
        Ok(())
    }

    async fn get(&self, id: &str) -> Result<Option<BackgroundTaskRecord>> {
        let row = sqlx::query(
            "SELECT id, status, command, session_id, started_at, finished_at, output, output_path
             FROM background_tasks WHERE id = ?",
        )
        .bind(id)
        .fetch_optional(&*self.pool)
        .await?;
        row.map(|row| row_to_record(&row)).transpose()
    }

    async fn list(&self) -> Result<Vec<BackgroundTaskRecord>> {
        let rows = sqlx::query(
            "SELECT id, status, command, session_id, started_at, finished_at, output, output_path
             FROM background_tasks ORDER BY started_at",
        )
        .fetch_all(&*self.pool)
        .await?;
        rows.iter().map(row_to_record).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-bgstore-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    fn record(id: &str, status: BackgroundTaskStatus) -> BackgroundTaskRecord {
        BackgroundTaskRecord {
            id: id.to_string(),
            status,
            command: "echo hi".to_string(),
            session_id: "sess-1".to_string(),
            started_at: Utc::now(),
            finished_at: None,
            output: String::new(),
            output_path: None,
        }
    }

    #[tokio::test]
    async fn upsert_get_round_trips_all_statuses() {
        let db = temp_db("round_trip");
        let store = SqliteBackgroundStore::new(&db).await.unwrap();

        let mut r = record("00000001", BackgroundTaskStatus::Running);
        store.upsert(&r).await.unwrap();
        assert_eq!(
            store.get("00000001").await.unwrap().unwrap().status,
            BackgroundTaskStatus::Running
        );

        r.status = BackgroundTaskStatus::Completed;
        r.finished_at = Some(Utc::now());
        r.output = "hello".to_string();
        r.output_path = Some("/tmp/bg/00000001.log".to_string());
        store.upsert(&r).await.unwrap();
        let got = store.get("00000001").await.unwrap().unwrap();
        assert_eq!(got.status, BackgroundTaskStatus::Completed);
        assert_eq!(got.output, "hello");
        assert_eq!(got.output_path.as_deref(), Some("/tmp/bg/00000001.log"));
        assert!(got.finished_at.is_some());
        assert_eq!(got.session_id, "sess-1");
    }

    #[tokio::test]
    async fn migrates_legacy_table_without_output_path() {
        let db = temp_db("migrate_output_path");
        // Create a table with the pre-`output_path` schema, as an existing
        // deployment would have on disk.
        let pool = crate::store::sqlite::open_pool(&db).await.unwrap();
        sqlx::query(
            r#"
            CREATE TABLE background_tasks (
                id          TEXT    PRIMARY KEY,
                status      TEXT    NOT NULL
                            CHECK (status IN ('running','completed','error')),
                command     TEXT    NOT NULL,
                session_id  TEXT    NOT NULL DEFAULT '',
                started_at  INTEGER NOT NULL,
                finished_at INTEGER,
                output      TEXT    NOT NULL DEFAULT ''
            );
            "#,
        )
        .execute(&*pool)
        .await
        .unwrap();
        drop(pool);

        let store = SqliteBackgroundStore::new(&db).await.unwrap();
        let mut r = record("00000001", BackgroundTaskStatus::Completed);
        r.output = "hello".to_string();
        r.output_path = Some("/tmp/bg/00000001.log".to_string());
        store.upsert(&r).await.unwrap();

        let got = store.get("00000001").await.unwrap().unwrap();
        assert_eq!(got.output_path.as_deref(), Some("/tmp/bg/00000001.log"));
    }

    #[tokio::test]
    async fn get_unknown_id_returns_none() {
        let db = temp_db("unknown_id");
        let store = SqliteBackgroundStore::new(&db).await.unwrap();
        assert!(store.get("deadbeef").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn store_is_persistent_across_reopen() {
        let db = temp_db("persistent");
        {
            let store = SqliteBackgroundStore::new(&db).await.unwrap();
            store
                .upsert(&record("00000001", BackgroundTaskStatus::Error))
                .await
                .unwrap();
        }
        let store = SqliteBackgroundStore::new(&db).await.unwrap();
        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].id, "00000001");
        assert_eq!(tasks[0].status, BackgroundTaskStatus::Error);
    }

    #[tokio::test]
    async fn list_is_sorted_by_started_at() {
        let db = temp_db("sorted");
        let store = SqliteBackgroundStore::new(&db).await.unwrap();
        let mut later = record("00000002", BackgroundTaskStatus::Completed);
        later.started_at = Utc::now() + chrono::Duration::seconds(10);
        let mut earlier = record("00000001", BackgroundTaskStatus::Error);
        earlier.started_at = Utc::now() - chrono::Duration::seconds(10);
        store.upsert(&later).await.unwrap();
        store.upsert(&earlier).await.unwrap();

        let tasks = store.list().await.unwrap();
        assert_eq!(tasks[0].id, "00000001");
        assert_eq!(tasks[1].id, "00000002");
    }
}
