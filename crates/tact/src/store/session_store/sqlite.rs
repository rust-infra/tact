use std::path::Path;

use anthropic_ai_sdk::types::message::{Message, MessageContent, Role};
use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool};
use tact_protocol::TokenUsageInfo;

use super::{MAX_INPUT_HISTORY, MessageCountByPeriod, SessionSummary};

pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    pub async fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .context("failed to create database directory")?;
        }
        // sqlx may fail to open a non-existent database file in some environments;
        // create an empty file first to ensure it's present.
        if let Err(e) = tokio::fs::metadata(path).await {
            if e.kind() == std::io::ErrorKind::NotFound {
                tokio::fs::File::create(path)
                    .await
                    .context("failed to create database file")?;
            }
        }
        let url = format!("sqlite:{}", path.display());
        let pool = SqlitePool::connect(&url)
            .await
            .with_context(|| format!("failed to open sqlite database at {}", path.display()))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                updated_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create sessions table")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content JSON NOT NULL,
                ordinal INTEGER NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create messages table")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id);")
            .execute(&pool)
            .await
            .context("failed to create messages index")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at);")
            .execute(&pool)
            .await
            .context("failed to create messages created_at index")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS token_usages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                call_type TEXT NOT NULL,
                request_body BLOB NOT NULL,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                total_tokens INTEGER NOT NULL DEFAULT 0,
                prompt_cache_hit_tokens INTEGER NOT NULL DEFAULT 0,
                prompt_cache_miss_tokens INTEGER NOT NULL DEFAULT 0,
                reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                first_message_id INTEGER NOT NULL DEFAULT 0,
                last_message_id INTEGER NOT NULL DEFAULT 0,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create token_usages table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_token_usages_session_id ON token_usages(session_id);",
        )
        .execute(&pool)
        .await
        .context("failed to create token_usages index")?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS input_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create input_history table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_input_history_session_id ON input_history(session_id);",
        )
        .execute(&pool)
        .await
        .context("failed to create input_history index")?;

        // Migration: add request_body column for LLM call debugging.
        let _ = sqlx::query("ALTER TABLE token_usages ADD COLUMN request_body BLOB")
            .execute(&pool)
            .await;

        // Migration: add tool_schedule column (JSON summary of how the turn's
        // tool calls were scheduled into parallel waves).
        let _ = sqlx::query("ALTER TABLE token_usages ADD COLUMN tool_schedule TEXT")
            .execute(&pool)
            .await;

        // Migration: drop deprecated title column from sessions.
        let _ = sqlx::query("ALTER TABLE sessions DROP COLUMN title")
            .execute(&pool)
            .await;

        // Migration: process lock — 0 = unlocked, non-zero = holder PID.
        let _ = sqlx::query(
            "ALTER TABLE sessions ADD COLUMN locked_by INTEGER NOT NULL DEFAULT 0",
        )
        .execute(&pool)
        .await;

        Ok(Self { pool })
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    async fn trim_input_history(self: &Self, session_id: &str, keep: usize) -> Result<()> {
        let pool = self.pool.clone();
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM input_history WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&pool)
            .await
            .context("failed to count input history entries")?;
        let count: i64 = row.try_get("cnt")?;
        let excess = count - keep as i64;
        if excess <= 0 {
            return Ok(());
        }

        sqlx::query(
            r#"
            DELETE FROM input_history
            WHERE session_id = ?
              AND id IN (
                SELECT id FROM input_history
                WHERE session_id = ?
                ORDER BY id ASC
                LIMIT ?
              )
            "#,
        )
        .bind(session_id)
        .bind(session_id)
        .bind(excess)
        .execute(&pool)
        .await
        .context("failed to trim input history")?;
        Ok(())
    }
}

fn role_to_str(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

fn str_to_role(s: &str) -> Result<Role> {
    match s {
        "user" => Ok(Role::User),
        "assistant" => Ok(Role::Assistant),
        other => Err(anyhow::anyhow!("unknown role: {}", other)),
    }
}

#[async_trait::async_trait]
impl super::SessionStore for SqliteSessionStore {
    async fn create_session(&self, id: &str) -> Result<()> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO sessions (id, created_at, updated_at) VALUES (?, ?, ?) ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at",
        )
        .bind(id)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("failed to create session")?;
        Ok(())
    }

    async fn append_message(
        &self,
        session_id: &str,
        role: Role,
        content: &MessageContent,
        ordinal: i64,
    ) -> Result<i64> {
        let content_json =
            serde_json::to_string(content).context("failed to serialize message content")?;
        let now = Self::now();

        let id = sqlx::query(
            "INSERT INTO messages (session_id, role, content, ordinal, created_at) VALUES (?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(role_to_str(role))
        .bind(content_json)
        .bind(ordinal)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("failed to insert message")?
        .last_insert_rowid();

        sqlx::query("UPDATE sessions SET updated_at = ? WHERE id = ?")
            .bind(now)
            .bind(session_id)
            .execute(&self.pool)
            .await
            .context("failed to update session timestamp")?;

        Ok(id)
    }

    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>> {
        let rows = sqlx::query(
            "SELECT role, content FROM messages WHERE session_id = ? ORDER BY ordinal ASC, id ASC",
        )
        .bind(session_id)
        .fetch_all(&self.pool)
        .await
        .context("failed to load session messages")?;

        let mut messages = Vec::with_capacity(rows.len());
        for row in rows {
            let role_str: String = row.try_get("role")?;
            let content_json: String = row.try_get("content")?;
            let content: MessageContent = serde_json::from_str(&content_json)
                .context("failed to deserialize message content")?;
            messages.push(Message {
                role: str_to_role(&role_str)?,
                content,
            });
        }

        Ok(messages)
    }

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>> {
        let rows = sqlx::query(
            r#"
            SELECT
                s.id,
                s.created_at,
                s.updated_at,
                COUNT(m.id) as message_count
            FROM sessions s
            LEFT JOIN messages m ON m.session_id = s.id
            GROUP BY s.id
            ORDER BY s.updated_at DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to list sessions")?;

        let mut sessions = Vec::with_capacity(rows.len());
        for row in rows {
            sessions.push(SessionSummary {
                id: row.try_get("id")?,
                created_at: parse_timestamp(
                    &row,
                    "created_at",
                    "failed to parse session created_at",
                )?,
                updated_at: parse_timestamp(
                    &row,
                    "updated_at",
                    "failed to parse session updated_at",
                )?,
                message_count: row.try_get("message_count")?,
            });
        }

        Ok(sessions)
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        let mut tx = self
            .pool
            .begin()
            .await
            .context("failed to begin delete session transaction")?;

        sqlx::query("DELETE FROM messages WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .context("failed to delete session messages")?;
        sqlx::query("DELETE FROM token_usages WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .context("failed to delete session token usages")?;
        sqlx::query("DELETE FROM input_history WHERE session_id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .context("failed to delete session input history")?;
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .context("failed to delete session")?;

        tx.commit()
            .await
            .context("failed to commit delete session transaction")?;
        Ok(())
    }

    async fn count_messages_by_session(&self, session_id: &str) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM messages WHERE session_id = ?")
            .bind(session_id)
            .fetch_one(&self.pool)
            .await
            .context("failed to count messages by session")?;
        Ok(row.try_get("cnt")?)
    }

    async fn count_messages_daily(&self) -> Result<Vec<MessageCountByPeriod>> {
        let rows = sqlx::query(
            r#"
            SELECT
                DATE(created_at) as label,
                COUNT(*) as cnt
            FROM messages
            GROUP BY label
            ORDER BY label DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to count messages daily")?;

        Ok(rows
            .into_iter()
            .map(|row| MessageCountByPeriod {
                period: "daily".to_string(),
                label: row.try_get::<String, _>("label").unwrap_or_default(),
                count: row.try_get("cnt").unwrap_or(0),
            })
            .collect())
    }

    async fn count_messages_weekly(&self) -> Result<Vec<MessageCountByPeriod>> {
        let rows = sqlx::query(
            r#"
            SELECT
                strftime('%Y-W%W', created_at) as label,
                COUNT(*) as cnt
            FROM messages
            GROUP BY label
            ORDER BY label DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to count messages weekly")?;

        Ok(rows
            .into_iter()
            .map(|row| MessageCountByPeriod {
                period: "weekly".to_string(),
                label: row.try_get::<String, _>("label").unwrap_or_default(),
                count: row.try_get("cnt").unwrap_or(0),
            })
            .collect())
    }

    async fn count_messages_monthly(&self) -> Result<Vec<MessageCountByPeriod>> {
        let rows = sqlx::query(
            r#"
            SELECT
                strftime('%Y-%m', created_at) as label,
                COUNT(*) as cnt
            FROM messages
            GROUP BY label
            ORDER BY label DESC
            "#,
        )
        .fetch_all(&self.pool)
        .await
        .context("failed to count messages monthly")?;

        Ok(rows
            .into_iter()
            .map(|row| MessageCountByPeriod {
                period: "monthly".to_string(),
                label: row.try_get::<String, _>("label").unwrap_or_default(),
                count: row.try_get("cnt").unwrap_or(0),
            })
            .collect())
    }

    async fn count_messages_total(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM messages")
            .fetch_one(&self.pool)
            .await
            .context("failed to count total messages")?;
        Ok(row.try_get("cnt")?)
    }

    async fn count_sessions_total(&self) -> Result<i64> {
        let row = sqlx::query("SELECT COUNT(*) as cnt FROM sessions")
            .fetch_one(&self.pool)
            .await
            .context("failed to count total sessions")?;
        Ok(row.try_get("cnt")?)
    }

    async fn record_token_usage(
        &self,
        session_id: &str,
        call_type: &str,
        usage: Option<&TokenUsageInfo>,
        first_message_id: i64,
        last_message_id: i64,
        request_body: Option<&[u8]>,
    ) -> Result<()> {
        let (prompt, completion, total, cache_hit, cache_miss, reasoning) = usage
            .map(|u| {
                (
                    u.prompt as i64,
                    u.completion as i64,
                    u.total as i64,
                    u.prompt_cache_hit_tokens as i64,
                    u.prompt_cache_miss_tokens as i64,
                    u.reasoning_tokens as i64,
                )
            })
            .unwrap_or((0, 0, 0, 0, 0, 0));

        sqlx::query(
            "INSERT INTO token_usages (session_id, call_type, prompt_tokens, completion_tokens, total_tokens, prompt_cache_hit_tokens, prompt_cache_miss_tokens, reasoning_tokens, first_message_id, last_message_id, request_body) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(session_id)
        .bind(call_type)
        .bind(prompt)
        .bind(completion)
        .bind(total)
        .bind(cache_hit)
        .bind(cache_miss)
        .bind(reasoning)
        .bind(first_message_id)
        .bind(last_message_id)
        .bind(request_body)
        .execute(&self.pool)
        .await
        .context("failed to record token usage")?;
        Ok(())
    }

    async fn record_tool_schedule(
        &self,
        session_id: &str,
        last_message_id: i64,
        schedule_json: &str,
    ) -> Result<()> {
        // Update the latest token-usage row for this call window. Uses a
        // subquery because SQLite's UPDATE has no ORDER BY/LIMIT by default.
        sqlx::query(
            r#"
            UPDATE token_usages
            SET tool_schedule = ?
            WHERE id = (
                SELECT id FROM token_usages
                WHERE session_id = ? AND last_message_id = ?
                ORDER BY id DESC
                LIMIT 1
            )
            "#,
        )
        .bind(schedule_json)
        .bind(session_id)
        .bind(last_message_id)
        .execute(&self.pool)
        .await
        .context("failed to record tool schedule")?;
        Ok(())
    }

    async fn load_input_history(&self, session_id: &str) -> Result<Vec<String>> {
        let rows =
            sqlx::query("SELECT content FROM input_history WHERE session_id = ? ORDER BY id ASC")
                .bind(session_id)
                .fetch_all(&self.pool)
                .await
                .context("failed to load input history")?;

        let mut entries: Vec<String> = rows
            .into_iter()
            .filter_map(|row| row.try_get::<String, _>("content").ok())
            .collect();

        if entries.len() > MAX_INPUT_HISTORY {
            entries = entries.split_off(entries.len() - MAX_INPUT_HISTORY);
            self.trim_input_history(session_id, MAX_INPUT_HISTORY)
                .await?;
        }

        Ok(entries)
    }

    async fn append_input_history(&self, session_id: &str, content: &str) -> Result<()> {
        sqlx::query("INSERT INTO input_history (session_id, content) VALUES (?, ?)")
            .bind(session_id)
            .bind(content)
            .execute(&self.pool)
            .await
            .context("failed to append input history entry")?;
        Ok(())
    }

    async fn try_lock_session(&self, session_id: &str, pid: u32) -> Result<()> {
        let pid_i64 = i64::from(pid);
        let mut tx = self
            .pool
            .begin()
            .await
            .context("failed to begin session lock transaction")?;

        let row = sqlx::query("SELECT locked_by FROM sessions WHERE id = ?")
            .bind(session_id)
            .fetch_optional(&mut *tx)
            .await
            .context("failed to read session lock")?;

        let holder = match row {
            Some(r) => r.try_get::<i64, _>("locked_by")?,
            None => {
                anyhow::bail!("session not found: {session_id}");
            }
        };

        if holder != 0 && holder != pid_i64 && is_process_alive(holder as u32) {
            anyhow::bail!(
                "session {session_id} is locked by process {holder} (already open in another tact-ui instance)"
            );
        }

        sqlx::query("UPDATE sessions SET locked_by = ? WHERE id = ?")
            .bind(pid_i64)
            .bind(session_id)
            .execute(&mut *tx)
            .await
            .context("failed to acquire session lock")?;

        tx.commit()
            .await
            .context("failed to commit session lock")?;
        Ok(())
    }

    async fn release_session_lock(&self, session_id: &str, pid: u32) -> Result<()> {
        let pid_i64 = i64::from(pid);
        sqlx::query("UPDATE sessions SET locked_by = 0 WHERE id = ? AND locked_by = ?")
            .bind(session_id)
            .bind(pid_i64)
            .execute(&self.pool)
            .await
            .context("failed to release session lock")?;
        Ok(())
    }
}

fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(windows)]
    {
        use std::ffi::c_void;
        use std::ptr;

        const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

        extern "system" {
            fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
            fn CloseHandle(handle: *mut c_void) -> i32;
        }

        unsafe {
            let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if handle.is_null() || handle == ptr::null_mut() {
                return false;
            }
            CloseHandle(handle);
            true
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = pid;
        true
    }
}

fn parse_timestamp(row: &sqlx::sqlite::SqliteRow, col: &str, msg: &str) -> Result<DateTime<Utc>> {
    let s: String = row.try_get(col)?;
    // Try full timestamp first, then date-only fallback.
    if let Ok(dt) = s.parse::<DateTime<Utc>>() {
        return Ok(dt);
    }
    if let Ok(date) = NaiveDate::parse_from_str(&s, "%Y-%m-%d") {
        return Ok(DateTime::from_naive_utc_and_offset(
            date.and_hms_opt(0, 0, 0).unwrap_or_default(),
            Utc,
        ));
    }
    Err(anyhow::anyhow!("{}: {}", msg, s))
}

#[cfg(test)]
mod tests {
    use anthropic_ai_sdk::types::message::{MessageContent, Role};
    use sqlx::Row;
    use tempfile::TempDir;

    use super::super::SessionStore;
    use super::SqliteSessionStore;

    #[tokio::test]
    async fn test_session_round_trip_and_stats() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();

        store.create_session("session-1").await.unwrap();

        store
            .append_message(
                "session-1",
                Role::User,
                &MessageContent::Text {
                    content: "hello".to_string(),
                },
                1,
            )
            .await
            .unwrap();
        store
            .append_message(
                "session-1",
                Role::Assistant,
                &MessageContent::Text {
                    content: "world".to_string(),
                },
                2,
            )
            .await
            .unwrap();

        let messages = store.load_session("session-1").await.unwrap();
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].role, Role::User));
        assert!(matches!(messages[1].role, Role::Assistant));

        let count = store.count_messages_by_session("session-1").await.unwrap();
        assert_eq!(count, 2);

        let total = store.count_messages_total().await.unwrap();
        assert_eq!(total, 2);

        let sessions = store.list_sessions().await.unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 2);

        let daily = store.count_messages_daily().await.unwrap();
        assert_eq!(daily.iter().map(|d| d.count).sum::<i64>(), 2);

        store.delete_session("session-1").await.unwrap();
        let after = store.load_session("session-1").await.unwrap();
        assert!(after.is_empty());
    }

    #[tokio::test]
    async fn test_tool_schedule_attaches_to_token_usage() {
        use sqlx::Row;

        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();
        store.create_session("session-1").await.unwrap();

        // A token-usage row for an LLM call whose last message id is 7.
        store
            .record_token_usage("session-1", "stream", None, 1, 7, Some(b"{}"))
            .await
            .unwrap();

        // The schedule for that turn is recorded afterwards, keyed by the same id.
        let schedule = r#"{"total_tools":2,"wave_count":1,"max_parallelism":2,"waves":[]}"#;
        store
            .record_tool_schedule("session-1", 7, schedule)
            .await
            .unwrap();

        let row = sqlx::query("SELECT tool_schedule FROM token_usages WHERE last_message_id = 7")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let stored: Option<String> = row.try_get("tool_schedule").unwrap();
        assert_eq!(stored.as_deref(), Some(schedule));
    }

    #[tokio::test]
    async fn test_input_history_round_trip() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();

        store.create_session("session-1").await.unwrap();

        let entries = vec!["first", "second"];
        for entry in &entries {
            store
                .append_input_history("session-1", entry)
                .await
                .unwrap();
        }

        let loaded = store.load_input_history("session-1").await.unwrap();
        assert_eq!(loaded, entries);

        store
            .append_input_history("session-1", "third")
            .await
            .unwrap();
        let loaded = store.load_input_history("session-1").await.unwrap();
        assert_eq!(loaded, vec!["first", "second", "third"]);

        store.create_session("session-2").await.unwrap();
        assert!(
            store
                .load_input_history("session-2")
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn test_input_history_trims_on_load() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();

        store.create_session("session-1").await.unwrap();

        let entries: Vec<String> = (0..120).map(|i| format!("entry-{i}")).collect();
        for entry in &entries {
            store
                .append_input_history("session-1", entry)
                .await
                .unwrap();
        }

        let loaded = store.load_input_history("session-1").await.unwrap();
        assert_eq!(loaded.len(), super::super::MAX_INPUT_HISTORY);
        assert_eq!(loaded.first().map(String::as_str), Some("entry-20"));
        assert_eq!(loaded.last().map(String::as_str), Some("entry-119"));

        let reloaded = store.load_input_history("session-1").await.unwrap();
        assert_eq!(reloaded, loaded);
    }

    #[tokio::test]
    async fn test_session_lock_acquire_and_release() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();
        store.create_session("session-1").await.unwrap();

        let pid = std::process::id();
        store.try_lock_session("session-1", pid).await.unwrap();

        let row = sqlx::query("SELECT locked_by FROM sessions WHERE id = 'session-1'")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let locked_by: i64 = row.try_get("locked_by").unwrap();
        assert_eq!(locked_by, i64::from(pid));

        store.release_session_lock("session-1", pid).await.unwrap();

        let row = sqlx::query("SELECT locked_by FROM sessions WHERE id = 'session-1'")
            .fetch_one(&store.pool)
            .await
            .unwrap();
        let locked_by: i64 = row.try_get("locked_by").unwrap();
        assert_eq!(locked_by, 0);
    }

    #[tokio::test]
    async fn test_session_lock_rejects_second_holder() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();
        store.create_session("session-1").await.unwrap();

        let pid = std::process::id();
        store.try_lock_session("session-1", pid).await.unwrap();

        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep helper");
        let other_pid = child.id();
        let err = store
            .try_lock_session("session-1", other_pid)
            .await
            .unwrap_err();
        let _ = child.kill();
        let _ = child.wait();

        assert!(err.to_string().contains("locked by process"));
    }
}
