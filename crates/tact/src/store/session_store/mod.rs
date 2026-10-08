//! Compatibility facade for the independent session owner crate.

use std::path::Path;

use anyhow::Result;

pub use tact_session::{
    DynSessionStore, MAX_INPUT_HISTORY, MAX_TOKEN_USAGE_BODIES, MessageCountByPeriod, SessionLock,
    SessionStore, SessionSummary, SqliteSessionStore,
};

/// Preserve the pre-extraction nested path for downstream users.
pub mod sqlite {
    pub use tact_session::SqliteSessionStore;
}

pub async fn open_sqlite_session_store(path: &Path) -> Result<DynSessionStore> {
    let limit = crate::config::try_settings()
        .map(|settings| settings.agent.max_token_usage_bodies)
        .unwrap_or(MAX_TOKEN_USAGE_BODIES);
    let store = SqliteSessionStore::new_with_token_usage_body_limit(path, limit).await?;
    Ok(std::sync::Arc::new(store))
}
