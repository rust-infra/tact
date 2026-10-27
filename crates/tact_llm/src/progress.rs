//! Provider-neutral streaming progress.

use tact_agent_api::{ProviderActivity, ReasoningEffort};

/// Token usage reported by a provider.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TokenUsageInfo {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
    pub prompt_cache_hit_tokens: u32,
    pub prompt_cache_miss_tokens: u32,
    pub reasoning_tokens: u32,
}

impl From<TokenUsageInfo> for tact_agent_api::TokenUsage {
    fn from(usage: TokenUsageInfo) -> Self {
        Self {
            input_tokens: usage.prompt,
            output_tokens: usage.completion,
            total_tokens: usage.total,
            cache_read_input_tokens: usage.prompt_cache_hit_tokens,
            cache_miss_input_tokens: usage.prompt_cache_miss_tokens,
            reasoning_tokens: usage.reasoning_tokens,
        }
    }
}

/// Parameters for one model call, without host presentation details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelCallParams {
    pub model: String,
    pub max_tokens: u32,
    pub thinking_budget: Option<u32>,
    pub reasoning_effort: Option<ReasoningEffort>,
    pub extra_body: Option<String>,
}

/// Lifecycle of one streamed thinking block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingChunk {
    Started,
    Delta(String),
    Finished,
}

/// Progress emitted while one provider response is in flight.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderProgress {
    ModelInfo(ModelCallParams),
    TextDelta(String),
    Thinking(ThinkingChunk),
    Usage(TokenUsageInfo),
    Activity(ProviderActivity),
}

/// Destination for provider progress.
pub trait ProviderProgressSink: Send + Sync {
    fn send(&self, progress: ProviderProgress);
}

/// Sink used when the caller does not display provider progress.
pub struct NoopProgressSink;

impl ProviderProgressSink for NoopProgressSink {
    fn send(&self, _progress: ProviderProgress) {}
}
