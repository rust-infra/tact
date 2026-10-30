use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::{MessageRange, ModelCallKind, RunContext, TokenUsage};

/// Idempotent durable usage record keyed by `call_id`.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageRecord {
    pub call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<RunContext>,
    pub kind: ModelCallKind,
    pub usage: Option<TokenUsage>,
    pub request_body: Option<Vec<u8>>,
    pub message_range: Option<MessageRange>,
}

impl std::fmt::Debug for UsageRecord {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("UsageRecord")
            .field("call_id", &self.call_id)
            .field("run", &self.run)
            .field("kind", &self.kind)
            .field("usage", &self.usage)
            .field(
                "request_body",
                &self.request_body.as_ref().map(|_| "<redacted>"),
            )
            .field("message_range", &self.message_range)
            .finish()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_record_debug_redacts_request_body() {
        let record = UsageRecord {
            call_id: "call".into(),
            run: None,
            kind: ModelCallKind::Stream,
            usage: None,
            request_body: Some(b"SECRET-BODY".to_vec()),
            message_range: None,
        };
        let debug = format!("{record:?}");
        assert!(!debug.contains("SECRET-BODY"));
        assert!(debug.contains("redacted"));
    }
}
