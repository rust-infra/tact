use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::Row;

use crate::store::sqlite::{PoolRef, open_pool};
use crate::task::{TaskRecord, TaskStatus, TaskUpdate};

use super::TaskStore;

/// SQLite-backed [`TaskStore`] implementation.
///
/// Shares `tact.db` with the session store. Schema:
///
/// - `tasks(id, subject, description, session_id, status, owner,
///   created_at, started_at, completed_at)`
/// - `task_dependencies(blocker_id, blocked_id)` — one row per edge,
///   composite PRIMARY KEY keeps edges unique.
pub struct SqliteTaskStore {
    pool: PoolRef,
}

impl SqliteTaskStore {
    pub async fn new(path: &Path) -> Result<Self> {
        let pool = open_pool(path).await?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS tasks (
                id           INTEGER PRIMARY KEY AUTOINCREMENT,
                subject      TEXT    NOT NULL,
                description  TEXT,
                session_id   TEXT    NOT NULL DEFAULT '',
                status       TEXT    NOT NULL DEFAULT 'pending'
                             CHECK (status IN ('pending','in_progress','completed','deleted')),
                owner        TEXT    NOT NULL DEFAULT '',
                created_at   INTEGER NOT NULL,
                started_at   INTEGER,
                completed_at INTEGER
            );
            "#,
        )
        .execute(&*pool)
        .await
        .context("failed to create tasks table")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tasks_session_id ON tasks(session_id);")
            .execute(&*pool)
            .await
            .context("failed to create tasks session_id index")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS task_dependencies (
                blocker_id INTEGER NOT NULL,
                blocked_id INTEGER NOT NULL,
                PRIMARY KEY (blocker_id, blocked_id)
            );
            "#,
        )
        .execute(&*pool)
        .await
        .context("failed to create task_dependencies table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_task_deps_blocked ON task_dependencies(blocked_id);",
        )
        .execute(&*pool)
        .await
        .context("failed to create task_dependencies blocked index")?;

        Ok(Self { pool })
    }

    /// Reads the task row plus its dependency edges.
    async fn read_task(&self, task_id: u64) -> Result<TaskRecord> {
        let row = sqlx::query(
            "SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at
             FROM tasks WHERE id = ?",
        )
        .bind(task_id as i64)
        .fetch_optional(&*self.pool)
        .await?
        .with_context(|| format!("Task {task_id} not found"))?;
        let mut task = row_to_task(&row)?;
        task.blocked_by = self.edges(task_id, EdgeDir::BlockedBy).await?;
        task.blocks = self.edges(task_id, EdgeDir::Blocks).await?;
        Ok(task)
    }

    async fn edges(&self, task_id: u64, dir: EdgeDir) -> Result<Vec<u64>> {
        let (sql, col) = match dir {
            EdgeDir::Blocks => (
                "SELECT blocked_id AS other FROM task_dependencies WHERE blocker_id = ?",
                "other",
            ),
            EdgeDir::BlockedBy => (
                "SELECT blocker_id AS other FROM task_dependencies WHERE blocked_id = ?",
                "other",
            ),
        };
        let rows = sqlx::query(sql)
            .bind(task_id as i64)
            .fetch_all(&*self.pool)
            .await?;
        let mut ids = rows
            .iter()
            .map(|row| row.try_get::<i64, _>(col).map(|v| v as u64))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        ids.sort_unstable();
        Ok(ids)
    }
}

#[derive(Clone, Copy)]
enum EdgeDir {
    /// `task -> other` (outgoing edges).
    Blocks,
    /// `other -> task` (incoming edges).
    BlockedBy,
}

#[async_trait]
impl TaskStore for SqliteTaskStore {
    async fn create(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
    ) -> Result<TaskRecord> {
        let task = TaskRecord::new(0, subject, description, session_id);
        let mut tx = self.pool.begin().await?;
        let id = sqlx::query(
            "INSERT INTO tasks (subject, description, session_id, status, owner, created_at)
             VALUES (?, ?, ?, 'pending', '', ?)",
        )
        .bind(&task.subject)
        .bind(&task.description)
        .bind(&task.session_id)
        .bind(task.created_at.unwrap_or(0))
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        tx.commit().await?;
        Ok(TaskRecord {
            id: id as u64,
            ..task
        })
    }

    async fn get(&self, task_id: u64) -> Result<TaskRecord> {
        self.read_task(task_id).await
    }

    async fn update(&self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord> {
        let mut tx = self.pool.begin().await?;

        // Read the current row (inside the write transaction so the read is
        // isolated from concurrent writers).
        let row = sqlx::query(
            "SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at
             FROM tasks WHERE id = ?",
        )
        .bind(task_id as i64)
        .fetch_optional(&mut *tx)
        .await?
        .with_context(|| format!("Task {task_id} not found"))?;
        let mut task = row_to_task(&row)?;

        if let Some(owner) = update.owner {
            task.owner = owner;
        }

        let mut clear_edges = false;
        if let Some(status) = update.status {
            let now = now_millis();
            task.status = status;
            match status {
                TaskStatus::InProgress => task.started_at = Some(now),
                TaskStatus::Completed => {
                    task.completed_at = Some(now);
                    clear_edges = true;
                }
                _ => {}
            }
        }

        sqlx::query(
            "UPDATE tasks SET status = ?, owner = ?, started_at = ?, completed_at = ? WHERE id = ?",
        )
        .bind(task.status.to_string())
        .bind(&task.owner)
        .bind(task.started_at)
        .bind(task.completed_at)
        .bind(task_id as i64)
        .execute(&mut *tx)
        .await?;

        // A completed task no longer participates in any dependency edge.
        if clear_edges {
            sqlx::query("DELETE FROM task_dependencies WHERE blocker_id = ? OR blocked_id = ?")
                .bind(task_id as i64)
                .bind(task_id as i64)
                .execute(&mut *tx)
                .await?;
        }

        for blocker_id in &update.add_blocked_by {
            sqlx::query(
                "INSERT OR IGNORE INTO task_dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            )
            .bind(*blocker_id as i64)
            .bind(task_id as i64)
            .execute(&mut *tx)
            .await?;
        }
        for blocked_id in &update.add_blocks {
            sqlx::query(
                "INSERT OR IGNORE INTO task_dependencies (blocker_id, blocked_id) VALUES (?, ?)",
            )
            .bind(task_id as i64)
            .bind(*blocked_id as i64)
            .execute(&mut *tx)
            .await?;
        }

        tx.commit().await?;

        self.read_task(task_id).await
    }

    async fn list(&self) -> Result<Vec<TaskRecord>> {
        let rows = sqlx::query(
            "SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at
             FROM tasks ORDER BY id",
        )
        .fetch_all(&*self.pool)
        .await?;
        let mut tasks = rows.iter().map(row_to_task).collect::<Result<Vec<_>>>()?;

        // Assemble dependency edges in one pass.
        let edge_rows = sqlx::query("SELECT blocker_id, blocked_id FROM task_dependencies")
            .fetch_all(&*self.pool)
            .await?;
        for task in &mut tasks {
            task.blocked_by.clear();
            task.blocks.clear();
        }
        let mut pos: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (i, task) in tasks.iter().enumerate() {
            pos.insert(task.id, i);
        }
        for row in edge_rows {
            let blocker = row.try_get::<i64, _>("blocker_id")? as u64;
            let blocked = row.try_get::<i64, _>("blocked_id")? as u64;
            let blocker_pos = pos.get(&blocker).copied().ok_or_else(|| {
                anyhow::anyhow!("dangling dependency: task {blocker} does not exist")
            })?;
            let blocked_pos = pos.get(&blocked).copied().ok_or_else(|| {
                anyhow::anyhow!("dangling dependency: task {blocked} does not exist")
            })?;
            tasks[blocker_pos].blocks.push(blocked);
            tasks[blocked_pos].blocked_by.push(blocker);
        }
        for task in &mut tasks {
            task.blocks.sort_unstable();
            task.blocked_by.sort_unstable();
        }
        Ok(tasks)
    }

    async fn delete(&self, task_id: u64) -> Result<TaskRecord> {
        let mut tx = self.pool.begin().await?;
        let affected = sqlx::query("UPDATE tasks SET status = 'deleted' WHERE id = ?")
            .bind(task_id as i64)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if affected == 0 {
            anyhow::bail!("Task {task_id} not found");
        }
        tx.commit().await?;
        self.read_task(task_id).await
    }
}

fn row_to_task(row: &sqlx::sqlite::SqliteRow) -> Result<TaskRecord> {
    let status = row
        .try_get::<String, _>("status")?
        .parse::<TaskStatus>()
        .context("invalid task status in database")?;
    Ok(TaskRecord {
        id: row.try_get::<i64, _>("id")? as u64,
        subject: row.try_get("subject")?,
        description: row.try_get("description")?,
        session_id: row.try_get("session_id")?,
        status,
        blocked_by: Vec::new(),
        blocks: Vec::new(),
        owner: row.try_get("owner")?,
        created_at: row.try_get("created_at")?,
        started_at: row.try_get("started_at")?,
        completed_at: row.try_get("completed_at")?,
    })
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
    use crate::task::TaskUpdate;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-taskstore-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    #[tokio::test]
    async fn create_round_trips_session_id() {
        let db = temp_db("session_round_trip");
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let task = store
            .create("sub".into(), None, "sess-42".into())
            .await
            .unwrap();
        assert_eq!(task.id, 1);
        assert_eq!(task.session_id, "sess-42");
        let got = store.get(task.id).await.unwrap();
        assert_eq!(got.subject, "sub");
        assert_eq!(got.session_id, "sess-42");
    }

    #[tokio::test]
    async fn store_is_persistent_across_reopen() {
        let db = temp_db("persistent");
        {
            let store = SqliteTaskStore::new(&db).await.unwrap();
            store.create("a".into(), None, String::new()).await.unwrap();
            store.create("b".into(), None, "s1".into()).await.unwrap();
        }
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, 1);
        assert_eq!(tasks[1].session_id, "s1");
    }

    #[tokio::test]
    async fn autoincrement_ids_continue_after_delete() {
        let db = temp_db("autoincrement");
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let a = store.create("a".into(), None, String::new()).await.unwrap();
        let b = store.create("b".into(), None, String::new()).await.unwrap();
        assert_eq!(a.id, 1);
        assert_eq!(b.id, 2);
        // Soft delete must not reuse the id.
        store.delete(a.id).await.unwrap();
        let c = store.create("c".into(), None, String::new()).await.unwrap();
        assert_eq!(c.id, 3);
    }

    #[tokio::test]
    async fn check_rejects_invalid_status() {
        let db = temp_db("check_status");
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let err =
            sqlx::query("INSERT INTO tasks (subject, status, created_at) VALUES ('x', 'bogus', 0)")
                .execute(&*store.pool)
                .await
                .unwrap_err();
        assert!(
            err.to_string().contains("constraint"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn duplicate_dependency_edges_are_ignored() {
        let db = temp_db("dup_edges");
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let blocker = store
            .create("Blocker".into(), None, String::new())
            .await
            .unwrap();
        let blocked = store
            .create("Blocked".into(), None, String::new())
            .await
            .unwrap();

        for _ in 0..2 {
            store
                .update(
                    blocker.id,
                    TaskUpdate {
                        add_blocks: vec![blocked.id],
                        ..Default::default()
                    },
                )
                .await
                .unwrap();
        }
        let blocker = store.get(blocker.id).await.unwrap();
        let blocked = store.get(blocked.id).await.unwrap();
        assert_eq!(blocker.blocks, vec![blocked.id]);
        assert_eq!(blocked.blocked_by, vec![blocker.id]);
    }
}
