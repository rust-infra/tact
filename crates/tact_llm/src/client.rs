//! [`LlmClient`] trait and [`LlmProvider`] enum.

use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    ContentBlock, CreateMessageParams, LlmError, ProviderConversationState, ProviderStateUpdate,
    StopReason, anthropic, mock::MockClient, openai,
};

/// Serialized JSON request body actually sent to the LLM API (for session debugging).
pub type LlmRequestBody = Vec<u8>;

/// Result of an LLM call: content blocks plus protocol metadata.
///
/// Replaces the previous 4-tuple `(blocks, stop_reason, usage, request_body)`
/// with a named struct so provider adapters can carry protocol state
/// (e.g. OpenAI Responses compaction) alongside the logical content.
#[derive(Debug, Clone)]
pub struct LlmResponse {
    /// Content blocks produced by the model.
    pub blocks: Vec<ContentBlock>,
    /// Why the model stopped generating.
    pub stop_reason: Option<StopReason>,
    /// Token usage reported by the provider (when available).
    pub usage: Option<TokenUsageInfo>,
    /// Serialized JSON request body actually sent (for session debugging).
    pub request_body: Option<LlmRequestBody>,
    /// Provider conversation state update produced by this call.
    pub state_update: ProviderStateUpdate,
}

/// Abstract interface for streaming and non-streaming LLM calls.
#[allow(async_fn_in_trait)]
pub trait LlmClient: Send + Sync {
    /// Stream a message request, emitting real-time updates via `ui_tx`.
    ///
    /// `provider_state` carries versioned provider-specific conversation
    /// state (currently only the OpenAI Responses protocol); non-Responses
    /// adapters ignore it.
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<LlmResponse, LlmError>;

    /// Non-streaming message request (used for context compaction).
    ///
    /// `provider_state` carries versioned provider-specific conversation
    /// state; non-Responses adapters ignore it.
    async fn create_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError>;

    /// Native provider compaction (Responses API only).
    ///
    /// The default implementation reports [`LlmError::Unsupported`];
    /// the OpenAI Responses adapter overrides this in a later task.
    async fn compact(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        let _ = (request, provider_state);
        Err(LlmError::Unsupported(
            "native compaction is not supported by this provider".to_string(),
        ))
    }
}

/// Supported LLM providers.
#[derive(Clone)]
pub enum LlmProvider {
    Anthropic(anthropic::AnthropicAdapter),
    ChatCompletions(openai::ChatCompletionsAdapter),
    OpenAiResponses(openai::responses::OpenAiResponsesAdapter),
    /// Mock provider for integration tests. Returns predetermined responses.
    Mock(MockClient),
}

impl LlmClient for LlmProvider {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<LlmResponse, LlmError> {
        match self {
            LlmProvider::Anthropic(a) => a.stream_message(request, provider_state, ui_tx).await,
            LlmProvider::ChatCompletions(c) => c.stream_message(request, provider_state, ui_tx).await,
            LlmProvider::OpenAiResponses(o) => {
                o.stream_message(request, provider_state, ui_tx).await
            }
            LlmProvider::Mock(m) => m.stream_message(request, provider_state, ui_tx).await,
        }
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        match self {
            LlmProvider::Anthropic(a) => a.create_message(request, provider_state).await,
            LlmProvider::ChatCompletions(c) => c.create_message(request, provider_state).await,
            LlmProvider::OpenAiResponses(o) => o.create_message(request, provider_state).await,
            LlmProvider::Mock(m) => m.create_message(request, provider_state).await,
        }
    }

    async fn compact(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        match self {
            LlmProvider::Anthropic(a) => a.compact(request, provider_state).await,
            LlmProvider::ChatCompletions(c) => c.compact(request, provider_state).await,
            LlmProvider::OpenAiResponses(o) => o.compact(request, provider_state).await,
            LlmProvider::Mock(m) => m.compact(request, provider_state).await,
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
            LlmProvider::ChatCompletions(c) => c.set_user_id(user_id.to_string()),
            LlmProvider::Anthropic(_)
            | LlmProvider::OpenAiResponses(_)
            | LlmProvider::Mock(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ContentBlock, RequiredMessageParams, StopReason, anthropic::AnthropicAdapter,
        mock::MockClient,
    };

    #[test]
    fn unchanged_provider_response_has_explicit_state_update() {
        let response = LlmResponse {
            blocks: Vec::new(),
            stop_reason: None,
            usage: None,
            request_body: None,
            state_update: ProviderStateUpdate::Unchanged,
        };
        assert!(matches!(
            response.state_update,
            ProviderStateUpdate::Unchanged
        ));
    }

    #[tokio::test]
    async fn mock_compact_returns_next_turn_with_unchanged_state() {
        let client = MockClient::new(vec![(
            vec![ContentBlock::Text {
                text: "compact".to_string(),
            }],
            Some(StopReason::EndTurn),
        )]);
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "mock".to_string(),
            messages: vec![],
            max_tokens: 100,
        });
        let response = client
            .compact(&request, None)
            .await
            .expect("mock compact should succeed");
        assert_eq!(response.blocks.len(), 1);
        assert_eq!(response.stop_reason, Some(StopReason::EndTurn));
        assert!(matches!(
            response.state_update,
            ProviderStateUpdate::Unchanged
        ));
    }

    #[tokio::test]
    async fn non_responses_compact_is_unsupported() {
        let client = LlmProvider::Anthropic(AnthropicAdapter::new(
            "sk-test",
            "https://api.anthropic.com",
        ));
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "claude-sonnet-4".to_string(),
            messages: vec![],
            max_tokens: 100,
        });
        let error = client
            .compact(&request, None)
            .await
            .expect_err("compact must be unsupported for non-Responses providers");
        assert!(matches!(error, LlmError::Unsupported(_)));
    }
}
