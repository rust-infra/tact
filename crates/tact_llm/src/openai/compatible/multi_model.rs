//! OpenAI-compatible multi-model client: live body-hook selection + transport.
//!
//! Built once for `ProviderKind::OpenAi`, then re-selects provider hooks each
//! request so `/model` (and other in-process provider updates) can switch
//! between OpenAI / DeepSeek-compatible / Kimi body shapes without rebuilding
//! the long-lived client.

use super::OpenAiAdapter;
use crate::{
    CreateMessageParams, LlmClient, LlmError, LlmResponse, ProviderConversationState,
    ProviderProfile, hook_select::hook_for_dialect,
};

use super::{
    body::assemble_chat_completion_body,
    compat::{create_assembled, stream_assembled},
};

/// Unified Chat Completions client that re-selects body hooks per request.
///
/// Holds an optional DeepSeek `user_id` (session id) so heuristic DeepSeek
/// endpoints still get KV-cache isolation after a mid-session model switch.
#[derive(Clone)]
pub struct ChatCompletionsAdapter {
    adapter: OpenAiAdapter,
    profile: ProviderProfile,
    user_id: Option<String>,
}

impl ChatCompletionsAdapter {
    pub fn new(profile: ProviderProfile, adapter: OpenAiAdapter) -> Self {
        Self {
            adapter,
            profile,
            user_id: None,
        }
    }

    pub fn base_url(&self) -> &str {
        self.adapter.base_url()
    }

    pub fn set_user_id(&mut self, user_id: String) {
        self.user_id = Some(user_id);
    }

    fn assemble_body(
        &self,
        request: &CreateMessageParams,
        stream: bool,
    ) -> Result<serde_json::Value, LlmError> {
        // Resolve the dialect from the profile each request so the body shape
        // follows the per-request model (`/model` picks) without rebuilding
        // the long-lived client.
        let dialect = self.profile.dialect_for(&request.model)?;
        let hook = hook_for_dialect(dialect, self.user_id.as_deref());
        assemble_chat_completion_body(request, stream, &self.profile, hook.as_ref())
    }
}

impl LlmClient for ChatCompletionsAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
        ui_tx: Option<tokio::sync::mpsc::UnboundedSender<tact_protocol::AgentUpdate>>,
    ) -> Result<LlmResponse, LlmError> {
        stream_assembled(&self.adapter, request, provider_state, ui_tx, |r, s| {
            self.assemble_body(r, s)
        })
        .await
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
    ) -> Result<LlmResponse, LlmError> {
        create_assembled(&self.adapter, request, provider_state, |r, s| {
            self.assemble_body(r, s)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::compatible::{
        CompatibleConfig, body::test_util::sample_request_with_thinking,
    };
    use crate::{OpenAiProtocol, ProviderKind, ProviderProfile};

    #[test]
    fn assemble_body_reselects_hook_after_model_switch() {
        let profile = ProviderProfile {
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
        };
        let adapter = ChatCompletionsAdapter::new(
            profile,
            OpenAiAdapter::new(CompatibleConfig::new(
                "sk-test",
                "https://api.openai.com/v1",
            )),
        );
        let mut request = sample_request_with_thinking()
            .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::Low));

        let openai_body = adapter.assemble_body(&request, false).unwrap();
        assert_eq!(openai_body["reasoning_effort"], "low");
        assert!(openai_body.get("thinking").is_none());

        request.model = "kimi-k2.5".to_string();
        let kimi_body = adapter.assemble_body(&request, false).unwrap();
        assert_eq!(kimi_body["thinking"]["type"], "enabled");
        assert!(kimi_body.get("reasoning_effort").is_none());
    }
}
