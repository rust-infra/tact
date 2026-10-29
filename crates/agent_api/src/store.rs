use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{AgentMessage, MessageRange, ModelCursor, SessionRef};

/// Durable, non-transcript state needed to resume an activation safely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStateSnapshot {
    pub session: SessionRef,
    pub total_turns: u32,
    pub last_token_total: Option<u32>,
}

/// Cursor mutation committed atomically with a transcript write.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CursorUpdate {
    Unchanged,
    Replace { cursor: Option<ModelCursor> },
}

/// Transcript loaded from a durable store.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptSnapshot {
    pub transcript: Vec<AgentMessage>,
    pub cursor: Option<ModelCursor>,
    pub range: Option<MessageRange>,
}

/// Receipt for one appended logical message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageReceipt {
    pub range: MessageRange,
}

/// Receipt for an atomic replacement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReplaceReceipt {
    pub range: Option<MessageRange>,
}

/// Transcript adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum StoreError {
    #[error("transcript load failed: {0}")]
    Load(String),
    #[error("transcript append failed: {0}")]
    Append(String),
    #[error("transcript replace failed: {0}")]
    Replace(String),
    #[error("transcript contract violation: {0}")]
    Contract(String),
}

/// Persistence barrier used by the kernel.
#[async_trait]
pub trait TranscriptStore: Send + Sync {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError>;

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError>;

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError>;

    async fn load_state(
        &self,
        _session: &SessionRef,
    ) -> Result<Option<SessionStateSnapshot>, StoreError> {
        Ok(None)
    }

    async fn save_state(
        &self,
        _session: &SessionRef,
        _state: &SessionStateSnapshot,
    ) -> Result<(), StoreError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_update_distinguishes_retain_clear_and_unchanged() {
        let unchanged = CursorUpdate::Unchanged;
        let retain = CursorUpdate::Replace { cursor: None };
        let cursor = ModelCursor {
            provider: "test".into(),
            version: 1,
            payload: serde_json::json!({}),
        };
        let replace = CursorUpdate::Replace {
            cursor: Some(cursor),
        };
        assert_ne!(unchanged, retain);
        assert_ne!(retain, replace);
    }
}
