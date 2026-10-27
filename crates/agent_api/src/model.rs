use std::{fmt, pin::Pin, str::FromStr, time::Duration};

use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};

use crate::{
    AgentContentBlock, AgentMessage, AgentRole, CancelToken, CursorUpdate, MessageId, ToolSpec,
};

/// Owned model event stream.
pub type ModelStream = Pin<Box<dyn Stream<Item = Result<ModelEvent, ModelError>> + Send + 'static>>;

/// Provider-neutral message handed to a model adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelMessage {
    pub role: AgentRole,
    pub content: Vec<AgentContentBlock>,
}

impl From<&AgentMessage> for ModelMessage {
    fn from(message: &AgentMessage) -> Self {
        Self {
            role: message.role,
            content: message.content.clone(),
        }
    }
}

/// Provider-neutral options attached to one model call.
///
/// Common request knobs stay typed. Provider-specific fields that do not
/// belong in the kernel contract live in `extra` and are interpreted by the
/// concrete model adapter.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelCallOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<ReasoningEffort>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking_budget: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_sequences: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(default, skip_serializing_if = "serde_json::Map::is_empty")]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Provider-neutral reasoning effort.
///
/// Providers that do not expose effort ignore this value in their adapter.
/// Keeping the closed set here prevents host and protocol layers from carrying
/// arbitrary strings into the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    None,
    Minimal,
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl ReasoningEffort {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl FromStr for ReasoningEffort {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "none" => Ok(Self::None),
            "minimal" => Ok(Self::Minimal),
            "low" => Ok(Self::Low),
            "medium" => Ok(Self::Medium),
            "high" => Ok(Self::High),
            "xhigh" => Ok(Self::Xhigh),
            "max" => Ok(Self::Max),
            other => Err(format!(
                "unknown reasoning effort {other:?}; expected none|minimal|low|medium|high|xhigh|max"
            )),
        }
    }
}

impl fmt::Display for ReasoningEffort {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl ModelCallOptions {
    pub fn extra_value(&self, key: &str) -> Option<&serde_json::Value> {
        self.extra.get(key)
    }
}

/// Complete request passed to a model port.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelRequest {
    pub model: String,
    pub system: String,
    pub messages: Vec<ModelMessage>,
    pub tools: Vec<ToolSpec>,
    pub max_output_tokens: u32,
    pub options: ModelCallOptions,
}

/// Opaque, versioned provider conversation cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCursor {
    pub provider: String,
    pub version: u32,
    pub payload: serde_json::Value,
}

/// Token accounting reported by a provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u32,
    #[serde(default)]
    pub output_tokens: u32,
    #[serde(default)]
    pub total_tokens: u32,
    #[serde(default)]
    pub cache_read_input_tokens: u32,
    #[serde(default)]
    pub cache_miss_input_tokens: u32,
    #[serde(default)]
    pub reasoning_tokens: u32,
}

/// Why a model response ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Refusal,
    PauseTurn,
    Unknown { raw: String },
}

/// Kind of model call for event and usage accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelCallKind {
    Stream,
    Complete,
    LocalCompact,
    NativeCompact,
}

/// Append-only display progress. The terminal response is authoritative.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MessageDelta {
    Text { delta: String },
    Thinking { phase: ThinkingDelta },
}

/// Thinking block lifecycle boundary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThinkingDelta {
    Started,
    Delta { delta: String },
    Finished,
}

/// Hosted provider activity, such as native web search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderActivity {
    pub id: String,
    pub step: u32,
    pub kind: ProviderActivityKind,
}

/// Closed set of hosted activities supported by the neutral contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ProviderActivityKind {
    WebSearch {
        phase: ProviderActivityPhase,
        query: Option<String>,
        failure: Option<String>,
        duration: Option<Duration>,
    },
}

/// Lifecycle phase of one hosted activity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderActivityPhase {
    Started,
    Completed,
    Failed,
}

/// Streaming event emitted before the single terminal response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelEvent {
    MessageStarted {
        message_id: MessageId,
        role: AgentRole,
    },
    MessageDelta {
        message_id: MessageId,
        delta: MessageDelta,
    },
    ProviderActivity {
        activity: ProviderActivity,
    },
    Usage {
        usage: TokenUsage,
    },
    Finished(ModelResponse),
}

/// One complete model response. This is the authoritative source for the
/// assistant message, stop reason, usage, diagnostics, and cursor update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelResponse {
    pub message_id: MessageId,
    pub message: AgentMessage,
    pub stop_reason: StopReason,
    pub usage: Option<TokenUsage>,
    pub request_body: Option<Vec<u8>>,
    pub cursor: CursorUpdate,
}

/// Static provider capabilities the kernel needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    pub model: String,
    pub context_window: usize,
    pub max_output_tokens: u32,
    pub supports_streaming: bool,
    pub supports_native_compaction: bool,
}

/// Closed model error classes used for recovery decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelErrorKind {
    ContextOverflow,
    Transient,
    Permanent,
    Cancelled,
    Unsupported,
}

/// Model adapter failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct ModelError {
    pub kind: ModelErrorKind,
    pub message: String,
    pub retry_after: Option<Duration>,
}

impl ModelError {
    pub fn new(kind: ModelErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            retry_after: None,
        }
    }

    pub fn unsupported(capability: impl Into<String>) -> Self {
        Self::new(
            ModelErrorKind::Unsupported,
            format!("{} is not supported", capability.into()),
        )
    }
}

/// Required model capability.
#[async_trait]
pub trait ModelPort: Send + Sync {
    fn capabilities(&self) -> ModelCapabilities;

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError>;

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError>;

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let _ = (request, cursor, cancel);
        Err(ModelError::unsupported("native compaction"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_contracts_round_trip() {
        let cursor = ModelCursor {
            provider: "test".into(),
            version: 1,
            payload: serde_json::json!({"opaque": true}),
        };
        let update = CursorUpdate::Replace {
            cursor: Some(cursor.clone()),
        };
        let json = serde_json::to_string(&update).unwrap();
        assert_eq!(serde_json::from_str::<CursorUpdate>(&json).unwrap(), update);

        let activity = ProviderActivity {
            id: "search_1".into(),
            step: 0,
            kind: ProviderActivityKind::WebSearch {
                phase: ProviderActivityPhase::Completed,
                query: Some("rust".into()),
                failure: None,
                duration: Some(Duration::from_millis(5)),
            },
        };
        let json = serde_json::to_string(&activity).unwrap();
        assert_eq!(
            serde_json::from_str::<ProviderActivity>(&json).unwrap(),
            activity
        );
    }

    #[test]
    fn every_stop_reason_round_trips() {
        let values = [
            StopReason::EndTurn,
            StopReason::ToolUse,
            StopReason::MaxTokens,
            StopReason::StopSequence,
            StopReason::Refusal,
            StopReason::PauseTurn,
            StopReason::Unknown {
                raw: "future".into(),
            },
        ];
        for value in values {
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<StopReason>(&json).unwrap(), value);
        }
    }

    #[test]
    fn message_deltas_and_thinking_boundaries_round_trip() {
        let values = [
            MessageDelta::Text {
                delta: "hello".into(),
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Started,
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Delta {
                    delta: "reason".into(),
                },
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Finished,
            },
        ];
        for value in values {
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(serde_json::from_str::<MessageDelta>(&json).unwrap(), value);
        }
    }

    #[test]
    fn reasoning_effort_is_a_closed_round_tripping_set() {
        let values = [
            ReasoningEffort::None,
            ReasoningEffort::Minimal,
            ReasoningEffort::Low,
            ReasoningEffort::Medium,
            ReasoningEffort::High,
            ReasoningEffort::Xhigh,
            ReasoningEffort::Max,
        ];
        for value in values {
            assert_eq!(value.as_str().parse(), Ok(value));
            let json = serde_json::to_string(&value).unwrap();
            assert_eq!(
                serde_json::from_str::<ReasoningEffort>(&json).unwrap(),
                value
            );
        }
        assert!("extreme".parse::<ReasoningEffort>().is_err());
    }
}
