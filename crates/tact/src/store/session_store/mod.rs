use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
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

    /// Replace all persisted messages for a session (used after compaction).
    async fn replace_session_messages(
        &self,
        session_id: &str,
        messages: &[Message],
    ) -> Result<(i64, i64)>;

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

pub async fn open_sqlite_session_store(path: &Path) -> Result<DynSessionStore> {
    let store = SqliteSessionStore::new(path).await?;
    Ok(Arc::new(store))
}
