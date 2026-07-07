use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use anthropic_ai_sdk::types::message::{Message, MessageContent, Role};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tact_protocol::TokenUsageInfo;

pub mod sqlite;

pub use sqlite::SqliteSessionStore;

mod process_identity;
mod session_lock;
pub use session_lock::SessionLock;

/// Maximum input history entries retained per session.
pub const MAX_INPUT_HISTORY: usize = 100;

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub root_dir: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub message_count: i64,
}

#[derive(Debug, Clone)]
pub struct MessageCountByPeriod {
    pub period: String,
    pub label: String,
    pub count: i64,
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create_session(&self, id: &str, root_dir: &str) -> Result<()>;

    /// Insert a session row if missing; does not update metadata for existing rows.
    async fn ensure_session_row(&self, id: &str, root_dir: &str) -> Result<()>;

    /// Refresh `updated_at` and `root_dir` after the process lock is held.
    async fn touch_session(&self, id: &str, root_dir: &str) -> Result<()>;

    async fn append_message(
        &self,
        session_id: &str,
        role: Role,
        content: &MessageContent,
        ordinal: i64,
    ) -> Result<i64>;

    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>>;

    async fn list_sessions(&self, root_dir: Option<&str>) -> Result<Vec<SessionSummary>>;

    async fn delete_session(&self, session_id: &str) -> Result<()>;

    async fn count_messages_by_session(&self, session_id: &str) -> Result<i64>;

    async fn count_messages_daily(&self) -> Result<Vec<MessageCountByPeriod>>;

    async fn count_messages_weekly(&self) -> Result<Vec<MessageCountByPeriod>>;

    async fn count_messages_monthly(&self) -> Result<Vec<MessageCountByPeriod>>;

    async fn count_messages_total(&self) -> Result<i64>;

    async fn count_sessions_total(&self) -> Result<i64>;

    /// Record per-call token usage (cache hit/miss, reasoning, prompt, completion).
    /// `first_message_id` / `last_message_id` link this call to the message range sent.
    /// `request_body` is the serialized JSON body sent to the LLM API (debug).
    async fn record_token_usage(
        &self,
        session_id: &str,
        call_type: &str,
        usage: Option<&TokenUsageInfo>,
        first_message_id: i64,
        last_message_id: i64,
        request_body: Option<&[u8]>,
    ) -> Result<()>;

    /// Attach a serialized tool-schedule summary (JSON) to the most recent
    /// token-usage row for `last_message_id` — the LLM call whose tool calls
    /// were just scheduled. Links scheduling strategy to token usage for later
    /// performance/troubleshooting analysis.
    async fn record_tool_schedule(
        &self,
        session_id: &str,
        last_message_id: i64,
        schedule_json: &str,
    ) -> Result<()>;

    async fn load_input_history(&self, session_id: &str) -> Result<Vec<String>>;

    async fn append_input_history(&self, session_id: &str, content: &str) -> Result<()>;

    /// Acquire an exclusive process lock on a session. Returns an opaque `lock_epoch`
    /// token that must be passed to [`Self::release_session_lock`].
    async fn try_lock_session(&self, session_id: &str, pid: u32) -> Result<String>;

    /// Release the process lock when held by `pid` with the matching `lock_epoch`.
    async fn release_session_lock(
        &self,
        session_id: &str,
        pid: u32,
        lock_epoch: &str,
    ) -> Result<()>;
}

pub type DynSessionStore = Arc<dyn SessionStore>;

/// Resolve the session database path, migrating from `<workdir>/.claude/tact.db`
/// to `<workdir>/.tact/tact.db` when the new file does not yet exist.
pub async fn resolve_session_db_path(
    tact_path: &crate::consts::TactPath,
) -> Result<std::path::PathBuf> {
    let new_path = tact_path.session_db_path();
    if tokio::fs::metadata(&new_path).await.is_ok() {
        return Ok(new_path);
    }

    let legacy_path = tact_path.legacy_session_db_path();
    if tokio::fs::metadata(&legacy_path).await.is_err() {
        return Ok(new_path);
    }

    if let Some(parent) = new_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    tokio::fs::rename(&legacy_path, &new_path)
        .await
        .with_context(|| {
            format!(
                "failed to migrate session db from {} to {}",
                legacy_path.display(),
                new_path.display()
            )
        })?;

    for suffix in ["-wal", "-shm", "-journal"] {
        let legacy_sidecar = legacy_sidecar_path(&legacy_path, suffix);
        let new_sidecar = legacy_sidecar_path(&new_path, suffix);
        if tokio::fs::metadata(&legacy_sidecar).await.is_ok() {
            let _ = tokio::fs::rename(&legacy_sidecar, &new_sidecar).await;
        }
    }

    Ok(new_path)
}

fn legacy_sidecar_path(base: &Path, suffix: &str) -> std::path::PathBuf {
    let name = base
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "tact.db".to_string());
    base.with_file_name(format!("{name}{suffix}"))
}

pub async fn open_sqlite_session_store(path: &Path) -> Result<DynSessionStore> {
    let store = SqliteSessionStore::new(path).await?;
    Ok(Arc::new(store))
}

#[cfg(test)]
mod migrate_tests {
    use super::resolve_session_db_path;
    use crate::consts::TactPath;
    use tempfile::TempDir;

    #[tokio::test]
    async fn migrates_legacy_session_db() {
        let tmp = TempDir::new().unwrap();
        let tact_path = TactPath::new(tmp.path());

        let legacy_dir = tact_path.claude_dir();
        tokio::fs::create_dir_all(&legacy_dir).await.unwrap();
        let legacy_db = tact_path.legacy_session_db_path();
        tokio::fs::write(&legacy_db, b"legacy-db").await.unwrap();

        let resolved = resolve_session_db_path(&tact_path).await.unwrap();
        assert_eq!(resolved, tact_path.session_db_path());
        assert!(resolved.exists());
        assert!(tokio::fs::metadata(&legacy_db).await.is_err());

        let contents = tokio::fs::read(&resolved).await.unwrap();
        assert_eq!(contents, b"legacy-db");
    }
}
