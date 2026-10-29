use std::path::Path;

use anyhow::{Context, Result};
use async_trait::async_trait;
use sqlx::{Row, Sqlite, Transaction};

use crate::store::sqlite::{PoolRef, begin_write, now_millis, open_pool};
use crate::task::{TaskRecord, TaskStatus, TaskUpdate};

use super::TaskStore;

/// SQLite-backed [`TaskStore`] implementation.
///
/// The task and Goal tables share the same database path. Goal-linked task
/// mutations validate the Goal row in that same database and transaction
/// before writing the task row.
/// Schema:
///
/// - `tasks(id, subject, description, session_id, status, owner,
///   created_at, started_at, completed_at, goal_id)`
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

        let columns = sqlx::query("PRAGMA table_info(tasks)")
            .fetch_all(&*pool)
            .await?;
        let has_goal_id = columns
            .iter()
            .any(|row| row.try_get::<String, _>("name").ok().as_deref() == Some("goal_id"));
        if !has_goal_id {
            sqlx::query("ALTER TABLE tasks ADD COLUMN goal_id TEXT")
                .execute(&*pool)
                .await
                .context("failed to migrate tasks goal_id column")?;
        }
        sqlx::query("CREATE INDEX IF NOT EXISTS idx_tasks_goal_id_id ON tasks(goal_id, id)")
            .execute(&*pool)
            .await
            .context("failed to create tasks goal_id index")?;

        // Initialize the Goal schema in this same database before any Goal
        // same-database Goal existence validation can run in task mutations.
        let _ = crate::store::goal_store::SqliteGoalStore::new(path).await?;

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
            "SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at, goal_id
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
            "INSERT INTO tasks (subject, description, session_id, status, owner, created_at, goal_id)
             VALUES (?, ?, ?, 'pending', '', ?, NULL)",
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

    async fn create_for_goal(
        &self,
        subject: String,
        description: Option<String>,
        session_id: String,
        goal_id: String,
    ) -> Result<TaskRecord> {
        validate_goal_id(&goal_id)?;
        let task = TaskRecord::new(0, subject, description, session_id);
        let mut tx = begin_write(&self.pool).await?;
        ensure_goal_exists(&mut tx, &goal_id).await?;
        let id = sqlx::query("INSERT INTO tasks (subject, description, session_id, status, owner, created_at, goal_id) VALUES (?, ?, ?, 'pending', '', ?, ?)")
            .bind(&task.subject).bind(&task.description).bind(&task.session_id)
            .bind(task.created_at.unwrap_or(0)).bind(&goal_id).execute(&mut *tx).await?.last_insert_rowid();
        tx.commit().await?;
        Ok(TaskRecord {
            id: id as u64,
            goal_id: Some(goal_id),
            ..task
        })
    }

    async fn associate_goal(&self, task_id: u64, goal_id: String) -> Result<TaskRecord> {
        validate_goal_id(&goal_id)?;
        let mut tx = begin_write(&self.pool).await?;
        ensure_goal_exists(&mut tx, &goal_id).await?;
        let row = sqlx::query("SELECT goal_id FROM tasks WHERE id = ?")
            .bind(task_id as i64)
            .fetch_optional(&mut *tx)
            .await?
            .with_context(|| format!("Task {task_id} not found"))?;
        let existing: Option<String> = row.try_get("goal_id")?;
        if let Some(existing) = existing
            && existing != goal_id
        {
            anyhow::bail!("Task {task_id} already belongs to goal {existing}");
        }
        sqlx::query("UPDATE tasks SET goal_id = ? WHERE id = ?")
            .bind(&goal_id)
            .bind(task_id as i64)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        self.read_task(task_id).await
    }

    async fn list_for_goal(&self, goal_id: &str) -> Result<Vec<TaskRecord>> {
        let rows = sqlx::query("SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at, goal_id FROM tasks WHERE goal_id = ? AND status != 'deleted' ORDER BY id")
            .bind(goal_id).fetch_all(&*self.pool).await?;
        let mut tasks = rows.iter().map(row_to_task).collect::<Result<Vec<_>>>()?;

        // Assemble dependency edges in one pass, like `list()`: the N+1 of two
        // `edges()` queries per task ran on every Goal activation cycle
        // (2026-10-16). Edges touching *any* task of the goal are selected —
        // the anchor is filtered into the result set below — so cross-goal
        // edges keep appearing on the anchor's side exactly as before.
        let edge_rows = sqlx::query(
            "SELECT blocker_id, blocked_id FROM task_dependencies \
             WHERE blocker_id IN (SELECT id FROM tasks WHERE goal_id = ?) \
                OR blocked_id IN (SELECT id FROM tasks WHERE goal_id = ?)",
        )
        .bind(goal_id)
        .bind(goal_id)
        .fetch_all(&*self.pool)
        .await?;
        let mut pos: std::collections::HashMap<u64, usize> = std::collections::HashMap::new();
        for (i, task) in tasks.iter().enumerate() {
            pos.insert(task.id, i);
        }
        for row in edge_rows {
            let blocker = row.try_get::<i64, _>("blocker_id")? as u64;
            let blocked = row.try_get::<i64, _>("blocked_id")? as u64;
            if let Some(&i) = pos.get(&blocker) {
                tasks[i].blocks.push(blocked);
            }
            if let Some(&j) = pos.get(&blocked) {
                tasks[j].blocked_by.push(blocker);
            }
        }
        for task in &mut tasks {
            task.blocks.sort_unstable();
            task.blocked_by.sort_unstable();
        }
        Ok(tasks)
    }

    async fn get(&self, task_id: u64) -> Result<TaskRecord> {
        self.read_task(task_id).await
    }

    async fn update(&self, task_id: u64, update: TaskUpdate) -> Result<TaskRecord> {
        // `BEGIN IMMEDIATE`, not `begin()`: this transaction reads the row
        // before it writes it, and a deferred transaction cannot wait once its
        // read snapshot exists. SQLite opens the snapshot at the SELECT, skips
        // the busy handler when the write half collides with another writer,
        // and returns SQLITE_BUSY (SQLITE_BUSY_SNAPSHOT once that writer
        // committed) immediately — `busy_timeout` never gets a chance. Taking
        // the write lock first moves the wait to a point where it works.
        let mut tx = begin_write(&self.pool).await?;

        // Read the current row (inside the write transaction so the read is
        // isolated from concurrent writers).
        let row = sqlx::query(
            "SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at, goal_id
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
            "SELECT id, subject, description, session_id, status, owner, created_at, started_at, completed_at, goal_id
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

fn validate_goal_id(goal_id: &str) -> Result<()> {
    if goal_id.trim().is_empty() {
        anyhow::bail!("goal ID must not be empty or whitespace");
    }
    Ok(())
}

async fn ensure_goal_exists(tx: &mut Transaction<'_, Sqlite>, goal_id: &str) -> Result<()> {
    let exists = sqlx::query("SELECT 1 FROM goals WHERE id = ?")
        .bind(goal_id)
        .fetch_optional(&mut **tx)
        .await?
        .is_some();
    if !exists {
        anyhow::bail!("Goal {goal_id} not found");
    }
    Ok(())
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
        goal_id: row.try_get("goal_id")?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::goal_store::GoalStore;

    fn temp_db(name: &str) -> std::path::PathBuf {
        crate::store::test_support::temp_db("tact-taskstore-test", name)
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
    async fn legacy_tasks_schema_migrates_and_reopens_idempotently() {
        let db = temp_db("legacy_migration");
        let pool = crate::store::sqlite::open_pool(&db).await.unwrap();
        sqlx::query(
            r#"
            CREATE TABLE tasks (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                subject TEXT NOT NULL,
                description TEXT,
                session_id TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                owner TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                started_at INTEGER,
                completed_at INTEGER
            )
            "#,
        )
        .execute(&*pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO tasks (subject, description, session_id, status, owner, created_at) VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind("legacy")
        .bind(Option::<String>::None)
        .bind("legacy-session")
        .bind("pending")
        .bind("")
        .bind(123_i64)
        .execute(&*pool)
        .await
        .unwrap();
        drop(pool);

        let store = SqliteTaskStore::new(&db).await.unwrap();
        let task = store.get(1).await.unwrap();
        assert_eq!(task.subject, "legacy");
        assert_eq!(task.goal_id, None);
        drop(store);

        let reopened = SqliteTaskStore::new(&db).await.unwrap();
        let task = reopened.get(1).await.unwrap();
        assert_eq!(task.goal_id, None);
    }

    #[tokio::test]
    async fn raw_goal_task_methods_reject_missing_and_empty_goals_without_mutation() {
        let db = temp_db("raw_goal_validation");
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let task = store
            .create("existing".into(), None, String::new())
            .await
            .unwrap();

        for goal_id in [String::new(), "missing".to_string()] {
            assert!(
                store
                    .create_for_goal("rejected".into(), None, String::new(), goal_id.clone())
                    .await
                    .is_err()
            );
            assert!(store.associate_goal(task.id, goal_id).await.is_err());
        }

        let tasks = store.list().await.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].goal_id, None);
    }

    #[tokio::test]
    async fn raw_association_allows_same_goal_and_rejects_other_goal_without_mutation() {
        let db = temp_db("raw_association");
        let goals = crate::store::goal_store::SqliteGoalStore::new(&db)
            .await
            .unwrap();
        let first = crate::goal::Goal::new(crate::goal::GoalCreate {
            title: "first".into(),
            objective: "first objective".into(),
            criteria: vec![crate::goal::GoalCriterion::required("done")],
        })
        .unwrap();
        let second = crate::goal::Goal::new(crate::goal::GoalCreate {
            title: "second".into(),
            objective: "second objective".into(),
            criteria: vec![crate::goal::GoalCriterion::required("done")],
        })
        .unwrap();
        goals.create(first.clone()).await.unwrap();
        goals.create(second.clone()).await.unwrap();
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let task = store
            .create("task".into(), None, String::new())
            .await
            .unwrap();

        store
            .associate_goal(task.id, first.id.clone())
            .await
            .unwrap();
        store
            .associate_goal(task.id, first.id.clone())
            .await
            .unwrap();
        assert!(store.associate_goal(task.id, second.id).await.is_err());
        assert_eq!(store.get(task.id).await.unwrap().goal_id, Some(first.id));
    }

    #[tokio::test]
    async fn list_for_goal_includes_both_dependency_directions() {
        let db = temp_db("goal_edges");
        let goals = crate::store::goal_store::SqliteGoalStore::new(&db)
            .await
            .unwrap();
        let goal = crate::goal::Goal::new(crate::goal::GoalCreate {
            title: "goal".into(),
            objective: "objective".into(),
            criteria: vec![crate::goal::GoalCriterion::required("done")],
        })
        .unwrap();
        goals.create(goal.clone()).await.unwrap();
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let blocker = store
            .create_for_goal("blocker".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
        let blocked = store
            .create_for_goal("blocked".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
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

        let listed = store.list_for_goal(&goal.id).await.unwrap();
        assert_eq!(listed[0].blocks, vec![blocked.id]);
        assert_eq!(listed[1].blocked_by, vec![blocker.id]);
    }

    /// The one-pass edge assembly keeps cross-goal edges on the anchor's side
    /// exactly as the per-task `edges()` queries did: a blocker outside the
    /// goal still shows up in the blocked task's `blocked_by`, and only the
    /// goal's own tasks are listed (2026-10-16).
    #[tokio::test]
    async fn list_for_goal_keeps_cross_goal_edges_on_the_anchor() {
        let db = temp_db("goal_cross_edges");
        let goals = crate::store::goal_store::SqliteGoalStore::new(&db)
            .await
            .unwrap();
        let goal = crate::goal::Goal::new(crate::goal::GoalCreate {
            title: "goal".into(),
            objective: "objective".into(),
            criteria: vec![crate::goal::GoalCriterion::required("done")],
        })
        .unwrap();
        goals.create(goal.clone()).await.unwrap();
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let in_goal = store
            .create_for_goal("in goal".into(), None, String::new(), goal.id.clone())
            .await
            .unwrap();
        let outside = store
            .create("outside".into(), None, String::new())
            .await
            .unwrap();
        store
            .update(
                outside.id,
                TaskUpdate {
                    add_blocks: vec![in_goal.id],
                    ..Default::default()
                },
            )
            .await
            .unwrap();

        let listed = store.list_for_goal(&goal.id).await.unwrap();
        assert_eq!(listed.len(), 1, "only the goal's own task is listed");
        assert_eq!(
            listed[0].blocked_by,
            vec![outside.id],
            "a cross-goal blocker still appears in the anchor's blocked_by"
        );
        assert!(
            listed[0].blocks.is_empty(),
            "the outside task is not listed here"
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

    /// `update` reads the row before it writes it. With a deferred `BEGIN`
    /// that read opens a snapshot, and SQLite then refuses to wait for another
    /// writer: the busy handler is skipped once a transaction is open, so the
    /// UPDATE fails in microseconds even with a five-second `busy_timeout`.
    /// Taking the write lock up front is what turns that into a wait, so this
    /// holds the lock from a second connection and expects `update` to ride it
    /// out instead of returning SQLITE_BUSY.
    #[tokio::test]
    async fn update_waits_for_a_concurrent_writer_instead_of_failing() {
        use sqlx::{Connection, SqliteConnection};

        let db = temp_db("update_contended");
        let store = SqliteTaskStore::new(&db).await.unwrap();
        let task = store
            .create("contended".into(), None, String::new())
            .await
            .unwrap();

        // A second connection stands in for another Tact instance holding
        // the write lock for a moment.
        let mut blocker = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .expect("open the second connection");
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut blocker)
            .await
            .unwrap();
        sqlx::query("UPDATE tasks SET owner = 'blocker' WHERE id = ?")
            .bind(task.id as i64)
            .execute(&mut blocker)
            .await
            .unwrap();

        let release = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
        });

        let updated = store
            .update(
                task.id,
                TaskUpdate {
                    status: Some(TaskStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .expect("update must wait out the writer instead of returning SQLITE_BUSY");

        release.await.unwrap();
        assert_eq!(updated.status, TaskStatus::Completed);
    }
}
