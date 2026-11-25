use std::{path::Path, sync::Arc};

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tact_agent_api::AgentMessage;
use tact_llm::{Message, MessageContent, ProviderConversationState, Role};
use tact_protocol::TokenUsageInfo;

pub mod sqlite;

pub use sqlite::SqliteSessionStore;

mod session_lock;
pub use session_lock::SessionLock;

/// The `<user_request>` body of a T3 Code prompt, when the text carries T3's
/// instruction wrapper.
///
/// T3 prepends `<t3_code_instructions>…</t3_code_instructions>` and a
/// `<user_request>…</user_request>` envelope on a thread's first turn. The
/// envelope is model-facing: session titles and transcript replay should show
/// only the request the user typed. Clients that send the text bare are
/// unaffected.
pub fn t3_user_request(text: &str) -> Option<&str> {
    let text = text.trim_start();
    if !text.starts_with("<t3_code_instructions>") {
        return None;
    }
    let body = text.split_once("<user_request>")?.1;
    let body = body.split_once("</user_request>")?.0;
    let body = body.trim();
    (!body.is_empty()).then_some(body)
}

/// Maximum input history entries retained per session.
pub const MAX_INPUT_HISTORY: usize = 100;

/// Default for `[agent] max_token_usage_bodies`: how many ordinary LLM-call
/// request bodies a session keeps in `token_usages`.
///
/// One is the smallest setting that still answers `/view-system-prompt`'s
/// assembled view, which reads the newest persisted request body. The column
/// stores the *whole serialized request* — system prompt, tool schemas and, on
/// `/responses`, the entire context, re-sent every call — so keeping them all
/// cost 8 GB in three weeks (one 9-hour session alone: 569 rows, 591 MiB, the
/// largest body 1.88 MB). Compaction rows (`compact`, `responses_compact`) are
/// exempt from the limit: that BLOB is the only place a compaction baseline and
/// its `encrypted_content` survive. Older rows keep every accounting column with
/// an **empty blob** as the "not retained" sentinel (`request_body` is
/// `NOT NULL`, so the sentinel is `X''`, not NULL).
pub const MAX_TOKEN_USAGE_BODIES: usize = 1;

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub root_dir: String,
    pub title: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    /// When the reader pinned this session, `None` when it is not pinned.
    ///
    /// A timestamp rather than a flag: the pinned block still has to order
    /// several pinned sessions against each other, and "when it was pinned" is
    /// the order the reader created.
    pub pinned_at: Option<DateTime<Utc>>,
    pub locked: bool,
    pub message_count: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArchiveFilter {
    #[default]
    Active,
    Archived,
    All,
}

#[derive(Debug, Clone, Default)]
pub struct SessionListQuery {
    pub root_dir: Option<String>,
    pub archive: ArchiveFilter,
    /// Case-insensitive title/id substring. Frontends may apply fuzzy ranking
    /// after this coarse filter.
    pub search: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MessageCountByPeriod {
    pub period: String,
    pub label: String,
    pub count: i64,
}

#[allow(clippy::double_must_use)] // async_trait generates #[must_use] on Pin<Box<dyn Future>> which is already #[must_use]
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

    /// Append one logical message with its explicit origin.
    async fn append_agent_message(
        &self,
        session_id: &str,
        message: &AgentMessage,
        ordinal: i64,
    ) -> Result<i64>;

    /// Atomically append a logical message and optionally replace or clear the
    /// provider cursor. `None` means retain the current cursor.
    async fn append_agent_message_and_provider_state(
        &self,
        session_id: &str,
        message: &AgentMessage,
        ordinal: i64,
        provider_state: Option<Option<&ProviderConversationState>>,
    ) -> Result<i64>;

    /// Replace all persisted messages for a session (used after compaction).
    async fn replace_session_messages(
        &self,
        session_id: &str,
        messages: &[Message],
    ) -> Result<(i64, i64)>;

    /// Atomically replace (or retain) the logical transcript and optionally
    /// replace or clear the provider cursor.
    async fn replace_agent_session_state(
        &self,
        session_id: &str,
        messages: Option<&[AgentMessage]>,
        provider_state: Option<Option<&ProviderConversationState>>,
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

    /// Load logical messages with explicit or strictly inferred origins.
    async fn load_agent_session(&self, session_id: &str) -> Result<Vec<AgentMessage>>;

    async fn list_sessions(&self, root_dir: Option<&str>) -> Result<Vec<SessionSummary>>;

    async fn list_sessions_filtered(&self, query: SessionListQuery) -> Result<Vec<SessionSummary>>;

    async fn set_session_title(&self, session_id: &str, title: Option<&str>) -> Result<()>;

    /// Copy a session's transcript into a new session, atomically.
    ///
    /// The copy is one transaction so a half-forked session cannot be left
    /// behind by a failure part-way through: either the new session exists with
    /// the whole transcript, or it does not exist at all.
    ///
    /// The fork is a **top-level** session, not a child: `ref_id` marks a
    /// subagent session, and the session list shows only rows with an empty
    /// `ref_id`. Recording the source there would hide the branch from the
    /// rail, which is the opposite of what a fork is for.
    ///
    /// The provider conversation state (`responses_states`) is deliberately
    /// **not** copied. It is a cursor into one provider's server-side
    /// conversation, and the fork's next turn may run against a different
    /// model or provider; carrying the cursor over would point the new session
    /// at a conversation whose contents the user did not fork. The transcript
    /// is the durable truth, so the fork rebuilds its context from it.
    ///
    /// Returns the number of messages copied.
    ///
    /// `upto` is an inclusive upper bound on the source's stored message
    /// `ordinal`; `None` copies the whole transcript. This is what lets a
    /// frontend fork "from this answer", keeping the branch point and dropping
    /// the turns that came after it.
    async fn fork_session(
        &self,
        source_id: &str,
        new_id: &str,
        root_dir: &str,
        upto: Option<i64>,
    ) -> Result<i64>;

    async fn set_session_archived(&self, session_id: &str, archived: bool) -> Result<()>;

    /// Pin a session to the top of the rail, or unpin it.
    ///
    /// Pinning records the moment it happened, so several pinned sessions keep
    /// a stable order among themselves; re-pinning an already-pinned session
    /// keeps the original moment rather than jumping it to the front.
    async fn set_session_pinned(&self, session_id: &str, pinned: bool) -> Result<()>;

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
    ///
    /// The body is kept for at most [`MAX_TOKEN_USAGE_BODIES`] ordinary calls
    /// per session; every compaction call keeps its body. Older bodies are
    /// blanked in place right after the insert (see
    /// `SqliteSessionStore::trim_token_usage_bodies`).
    async fn record_token_usage(
        &self,
        session_id: &str,
        call_type: &str,
        usage: Option<&TokenUsageInfo>,
        first_message_id: i64,
        last_message_id: i64,
        request_body: Option<&[u8]>,
    ) -> Result<()>;

    /// Record usage idempotently when `call_id` is present, while retaining the
    /// legacy nullable path for pre-kernel callers.
    #[allow(clippy::too_many_arguments)]
    async fn record_token_usage_with_call_id(
        &self,
        session_id: &str,
        call_id: Option<&str>,
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
