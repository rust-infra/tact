//! [`LlmClient`] trait and [`LlmProvider`] enum.

use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use crate::anthropic;
use crate::deepseek;
use crate::kimi;
use crate::mock::MockClient;
use crate::openai;
use crate::{ContentBlock, CreateMessageParams, LlmError, StopReason};

/// Serialized JSON request body actually sent to the LLM API (for session debugging).
pub type LlmRequestBody = Vec<u8>;

/// Abstract interface for streaming and non-streaming LLM calls.
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    /// Stream a message request, emitting real-time updates via `ui_tx`.
    ///
    /// Returns content blocks, stop reason, token usage, and the serialized request body.
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    >;

    /// Non-streaming message request (used for context compaction).
    ///
    /// Returns content blocks, stop reason, token usage, and the serialized request body.
    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    >;
}

/// Supported LLM providers.
#[derive(Clone)]
pub enum LlmProvider {
    Anthropic(anthropic::AnthropicAdapter),
    OpenAi(openai::OpenAiMultiModelAdapter),
    DeepSeek(deepseek::DeepSeekAdapter),
    Kimi(kimi::KimiAdapter),
    /// Mock provider for integration tests. Returns predetermined responses.
    Mock(MockClient),
}

#[async_trait::async_trait]
impl LlmClient for LlmProvider {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        match self {
            LlmProvider::Anthropic(a) => a.stream_message(request, ui_tx).await,
            LlmProvider::OpenAi(o) => o.stream_message(request, ui_tx).await,
            LlmProvider::DeepSeek(d) => d.stream_message(request, ui_tx).await,
            LlmProvider::Kimi(k) => k.stream_message(request, ui_tx).await,
            LlmProvider::Mock(m) => m.stream_message(request, ui_tx).await,
        }
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        match self {
            LlmProvider::Anthropic(a) => a.create_message(request).await,
            LlmProvider::OpenAi(o) => o.create_message(request).await,
            LlmProvider::DeepSeek(d) => d.create_message(request).await,
            LlmProvider::Kimi(k) => k.create_message(request).await,
            LlmProvider::Mock(m) => m.create_message(request).await,
        }
    }
}

impl LlmProvider {
    /// Set a `user_id` on the underlying client adapter.
    ///
    /// DeepSeek injects top-level `"user_id"` for KV cache isolation.
    /// OpenAI multi-model adapter forwards it when the live hook is DeepSeek.
    /// Anthropic / Kimi / Mock — no-op.
    pub fn set_user_id(&mut self, user_id: &str) {
        match self {
            LlmProvider::OpenAi(o) => o.set_user_id(user_id.to_string()),
            LlmProvider::DeepSeek(d) => d.set_user_id(user_id.to_string()),
            LlmProvider::Anthropic(_) | LlmProvider::Kimi(_) | LlmProvider::Mock(_) => {}
        }
    }
}
