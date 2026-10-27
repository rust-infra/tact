use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{MessageRange, ModelCallKind, TokenUsage};

/// Idempotent durable usage record keyed by `call_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub call_id: String,
    pub kind: ModelCallKind,
    pub usage: Option<TokenUsage>,
    pub request_body: Option<Vec<u8>>,
    pub message_range: Option<MessageRange>,
}

/// Durable usage sink failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
pub enum UsageError {
    #[error("usage record failed: {0}")]
    Record(String),
    #[error("usage flush failed: {0}")]
    Flush(String),
}

/// Authoritative usage and request-body persistence barrier.
#[async_trait]
pub trait UsageSink: Send + Sync {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageError>;

    async fn flush(&self) -> Result<(), UsageError>;
}
