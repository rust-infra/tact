//! Session persistence contracts shared by the runtime and storage backends.
//!
//! This crate intentionally contains no SQLite implementation. It provides
//! the storage boundary first, so a backend can move independently from the
//! agent facade without introducing a dependency on `tact`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tact_llm::{Message, MessageContent, ProviderConversationState, Role};
use tact_protocol::TokenUsageInfo;

mod pool;
mod process_identity;
mod session_lock;
pub mod sqlite;

pub use pool::{PoolRef, open_pool, pool_count_under};
pub use session_lock::SessionLock;
pub use sqlite::SqliteSessionStore;

/// Maximum input history entries retained per session.
pub const MAX_INPUT_HISTORY: usize = 100;

/// Default number of ordinary LLM request bodies retained per session.
pub const MAX_TOKEN_USAGE_BODIES: usize = 1;

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

/// Persistence boundary for a Tact session.
///
/// The trait mirrors the current store contract. Keeping the existing value
/// types and method shapes avoids a second serialization or provider-state
/// model while SQLite remains behind the legacy `tact` facade.
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create_session(&self, id: &str, root_dir: &str, ref_id: &str) -> Result<()>;
    async fn ensure_session_row(&self, id: &str, root_dir: &str, ref_id: &str) -> Result<()>;
    async fn touch_session(&self, id: &str, root_dir: &str) -> Result<()>;

    async fn append_message(
        &self,
        session_id: &str,
        role: Role,
        content: &MessageContent,
        ordinal: i64,
    ) -> Result<i64>;

    async fn replace_session_messages(
        &self,
        session_id: &str,
        messages: &[Message],
    ) -> Result<(i64, i64)>;

    async fn load_provider_state(
        &self,
        session_id: &str,
    ) -> Result<Option<ProviderConversationState>>;

    async fn replace_session_messages_and_provider_state(
        &self,
        session_id: &str,
        messages: &[Message],
        provider_state: Option<&ProviderConversationState>,
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

    async fn record_token_usage(
        &self,
        session_id: &str,
        call_type: &str,
        usage: Option<&TokenUsageInfo>,
        first_message_id: i64,
        last_message_id: i64,
        request_body: Option<&[u8]>,
    ) -> Result<()>;

    async fn load_latest_request_body(&self, session_id: &str) -> Result<Option<Vec<u8>>>;

    async fn record_tool_schedule(
        &self,
        session_id: &str,
        last_message_id: i64,
        schedule_json: &str,
    ) -> Result<()>;

    async fn load_input_history(&self, session_id: &str) -> Result<Vec<String>>;
    async fn append_input_history(&self, session_id: &str, content: &str) -> Result<()>;
    async fn try_lock_session(&self, session_id: &str, pid: u32) -> Result<String>;
    async fn release_session_lock(
        &self,
        session_id: &str,
        pid: u32,
        lock_epoch: &str,
    ) -> Result<()>;
}

pub type DynSessionStore = Arc<dyn SessionStore>;

pub async fn open_sqlite_session_store(path: &std::path::Path) -> Result<DynSessionStore> {
    Ok(Arc::new(SqliteSessionStore::new(path).await?))
}
