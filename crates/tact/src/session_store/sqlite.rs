use std::path::Path;

use anyhow::{Context, Result};
use anthropic_ai_sdk::types::message::{Message, MessageContent, Role};
use chrono::{DateTime, NaiveDate, Utc};
use sqlx::{Row, SqlitePool};

use super::{MessageCountByPeriod, SessionSummary};

pub struct SqliteSessionStore {
    pool: SqlitePool,
}

impl SqliteSessionStore {
    pub async fn new(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.context("failed to create database directory")?;
        }
        let url = format!("sqlite:{}", path.display());
        let pool = SqlitePool::connect(&url)
            .await
            .with_context(|| format!("failed to open sqlite database at {}", path.display()))?;

        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY NOT NULL,
                title TEXT,
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
                created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );
            "#,
        )
        .execute(&pool)
        .await
        .context("failed to create messages table")?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_messages_session_id ON messages(session_id);",
        )
        .execute(&pool)
        .await
        .context("failed to create messages index")?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_messages_created_at ON messages(created_at);")
            .execute(&pool)
            .await
            .context("failed to create messages created_at index")?;

        Ok(Self { pool })
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
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
    async fn create_session(&self, id: &str, title: Option<&str>) -> Result<()> {
        let now = Self::now();
        sqlx::query(
            "INSERT INTO sessions (id, title, created_at, updated_at) VALUES (?, ?, ?, ?) ON CONFLICT(id) DO UPDATE SET updated_at = excluded.updated_at, title = COALESCE(excluded.title, sessions.title)",
        )
        .bind(id)
        .bind(title)
        .bind(now)
        .bind(now)
        .execute(&self.pool)
        .await
        .context("failed to create session")?;
        Ok(())
    }

    async fn update_session_title(&self, id: &str, title: Option<&str>) -> Result<()> {
        sqlx::query("UPDATE sessions SET title = ?, updated_at = ? WHERE id = ?")
            .bind(title)
            .bind(Self::now())
            .bind(id)
            .execute(&self.pool)
            .await
            .context("failed to update session title")?;
        Ok(())
    }

    async fn append_message(
        &self,
        session_id: &str,
        role: Role,
        content: &MessageContent,
        ordinal: i64,
    ) -> Result<i64> {
        let content_json = serde_json::to_string(content)
            .context("failed to serialize message content")?;
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
            let content: MessageContent =
                serde_json::from_str(&content_json).context("failed to deserialize message content")?;
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
                s.title,
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
                title: row.try_get("title")?,
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
        sqlx::query("DELETE FROM sessions WHERE id = ?")
            .bind(session_id)
            .execute(&self.pool)
            .await
            .context("failed to delete session")?;
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
    use tempfile::TempDir;

    use super::super::SessionStore;
    use super::SqliteSessionStore;

    #[tokio::test]
    async fn test_session_round_trip_and_stats() {
        let tmp = TempDir::new().unwrap();
        let db = tmp.path().join("test.db");
        let store = SqliteSessionStore::new(&db).await.unwrap();

        store.create_session("session-1", Some("first session")).await.unwrap();

        store
            .append_message(
                "session-1",
                Role::User,
                &MessageContent::Text { content: "hello".to_string() },
                1,
            )
            .await
            .unwrap();
        store
            .append_message(
                "session-1",
                Role::Assistant,
                &MessageContent::Text { content: "world".to_string() },
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
}
