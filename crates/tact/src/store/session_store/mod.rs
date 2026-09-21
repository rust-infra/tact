use std::{path::Path, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tact_llm::{Message, MessageContent, ProviderConversationState, Role};
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
    /// First text the session's user sent, so a front end can title the row
    /// instead of printing the raw id. `None` until the session has a user
    /// message, and for user messages that carry only images or tool results.
    pub first_user_text: Option<String>,
    /// The name the user gave the session. Takes precedence over
    /// `first_user_text`, which is only a derived label.
    pub title: Option<String>,
    /// When the session was archived, if it was. Archiving is a policy flag:
    /// the row and its messages stay, and clearing the flag restores it.
    pub archived_at: Option<DateTime<Utc>>,
    /// When the session was pinned, if it was.
    ///
    /// Pinning is a list-order policy, not a state: the row keeps every other
    /// property and clearing the flag returns it to the ordinary date order.
    pub pinned_at: Option<DateTime<Utc>>,
}

/// The first text a message carries, ignoring non-text blocks.
///
/// The session list wants a title, and a session's first message is often
/// blocks rather than plain text; this picks the first block that reads as
/// prose so the title is the user's own words.
pub(crate) fn first_text(content: &MessageContent) -> Option<String> {
    match content {
        MessageContent::Text { content } => {
            let trimmed = content.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        MessageContent::Blocks { content } => content.iter().find_map(|block| match block {
            tact_llm::ContentBlock::Text { text } => {
                let trimmed = text.trim();
                (!trimmed.is_empty()).then(|| trimmed.to_string())
            }
            _ => None,
        }),
    }
}

#[derive(Debug, Clone)]
pub struct MessageCountByPeriod {
    pub period: String,
    pub label: String,
    pub count: i64,
}

#[async_trait]
pub trait SessionStore: Send + Sync {
    /// Create or refresh a session. `ref_id` is the parent session id (`""` = top-level).
    async fn create_session(&self, id: &str, root_dir: &str, ref_id: &str) -> Result<()>;

    /// Insert a session row if missing; does not update metadata for existing rows.
    /// `ref_id` is the parent session id (`""` = top-level).
    async fn ensure_session_row(&self, id: &str, root_dir: &str, ref_id: &str) -> Result<()>;

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

    /// Load the durable provider conversation state (Responses input-item
    /// baseline) for a session, if one was stored.
    async fn load_provider_state(
        &self,
        session_id: &str,
    ) -> Result<Option<ProviderConversationState>>;

    /// Atomically replace the session messages and the provider conversation
    /// state in one transaction. `provider_state = None` deletes any stored
    /// state (non-Responses callers).
    async fn replace_session_messages_and_provider_state(
        &self,
        session_id: &str,
        messages: &[Message],
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<(i64, i64)>;

    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>>;

    /// Give a session a name. An empty or whitespace-only `title` clears it, so
    /// the session falls back to its derived label.
    async fn rename_session(&self, session_id: &str, title: &str) -> Result<()>;

    /// Set or clear a session's archived flag.
    ///
    /// This is deliberately not a delete: `archived_at` marks policy, and the
    /// session's row and messages must survive it so the flag can be cleared.
    async fn archive_session(&self, session_id: &str, archived: bool) -> Result<()>;

    /// Set or clear a session's pinned flag.
    ///
    /// Like archiving, this only moves a marker: the row, its messages, and its
    /// place in the store are untouched, so unpinning restores the session to
    /// the ordinary date order with nothing lost.
    async fn pin_session(&self, session_id: &str, pinned: bool) -> Result<()>;

    /// Copy a session's own row and messages under `new_id`.
    ///
    /// Subagent children (`ref_id`) belong to the original, and recorded token
    /// usage is the original's spend, so neither is copied. The copy carries no
    /// provider conversation state, which costs one full replay on its first
    /// turn and cannot drift from the messages that were copied. It is not
    /// archived either: a copy made on purpose is one the user is about to use.
    async fn duplicate_session(&self, session_id: &str, new_id: &str) -> Result<()>;

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

    /// Load the serialized request body from the most recent LLM call.
    async fn load_latest_request_body(&self, session_id: &str) -> Result<Option<Vec<u8>>>;

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
