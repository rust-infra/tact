use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use anthropic_ai_sdk::types::message::{Message, MessageContent, Role};
use async_trait::async_trait;
use chrono::{DateTime, Utc};

pub mod sqlite;

pub use sqlite::SqliteSessionStore;

#[derive(Debug, Clone)]
pub struct SessionSummary {
    pub id: String,
    pub title: Option<String>,
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
    async fn create_session(&self, id: &str, title: Option<&str>) -> Result<()>;

    async fn update_session_title(&self, id: &str, title: Option<&str>) -> Result<()>;

    async fn append_message(
        &self,
        session_id: &str,
        role: Role,
        content: &MessageContent,
        ordinal: i64,
    ) -> Result<i64>;

    async fn load_session(&self, session_id: &str) -> Result<Vec<Message>>;

    async fn list_sessions(&self) -> Result<Vec<SessionSummary>>;

    async fn delete_session(&self, session_id: &str) -> Result<()>;

    async fn count_messages_by_session(&self, session_id: &str) -> Result<i64>;

    async fn count_messages_daily(&self) -> Result<Vec<MessageCountByPeriod>>;

    async fn count_messages_weekly(&self) -> Result<Vec<MessageCountByPeriod>>;

    async fn count_messages_monthly(&self) -> Result<Vec<MessageCountByPeriod>>;

    async fn count_messages_total(&self) -> Result<i64>;

    async fn count_sessions_total(&self) -> Result<i64>;
}

pub type DynSessionStore = Arc<dyn SessionStore>;

pub async fn open_sqlite_session_store(path: &Path) -> Result<DynSessionStore> {
    let store = SqliteSessionStore::new(path).await?;
    Ok(Arc::new(store))
}
