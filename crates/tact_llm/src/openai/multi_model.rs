//! OpenAI-compatible multi-model client: live body-hook selection + transport.
//!
//! Built once for `ProviderKind::OpenAi`, then re-selects provider hooks each
//! request so `/model` (and other in-process provider updates) can switch
//! between OpenAI / DeepSeek-compatible / Kimi body shapes without rebuilding
//! the long-lived client.

use crate::hook_select::body_hook_for;
use crate::openai::body::assemble_chat_completion_body;
use crate::openai::compat::{create_assembled, stream_assembled};
use crate::{CreateMessageParams, LlmClient, LlmError};

use super::OpenAiAdapter;

/// OpenAI-labeled client that re-selects body hooks from the live provider.
///
/// Holds an optional DeepSeek `user_id` (session id) so heuristic DeepSeek
/// endpoints still get KV-cache isolation after a mid-session model switch.
#[derive(Clone)]
pub struct OpenAiMultiModelAdapter {
    adapter: OpenAiAdapter,
    user_id: Option<String>,
}

impl OpenAiMultiModelAdapter {
    pub fn new(adapter: OpenAiAdapter) -> Self {
        Self {
            adapter,
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
        // Resolve the hook from the *live* provider each request so `/model`
        // (and other in-process provider updates) pick the right body shape
        // without rebuilding the long-lived client.
        crate::read_provider(|provider| {
            let hook = body_hook_for(provider, self.user_id.as_deref())?;
            assemble_chat_completion_body(request, stream, provider, hook.as_ref())
        })
    }
}

#[async_trait::async_trait]
impl LlmClient for OpenAiMultiModelAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<tokio::sync::mpsc::UnboundedSender<tact_protocol::AgentUpdate>>,
    ) -> Result<
        (
            Vec<crate::ContentBlock>,
            Option<crate::StopReason>,
            Option<tact_protocol::TokenUsageInfo>,
            Option<crate::LlmRequestBody>,
        ),
        LlmError,
    > {
        stream_assembled(&self.adapter, request, ui_tx, |r, s| {
            self.assemble_body(r, s)
        })
        .await
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<
        (
            Vec<crate::ContentBlock>,
            Option<crate::StopReason>,
            Option<tact_protocol::TokenUsageInfo>,
            Option<crate::LlmRequestBody>,
        ),
        LlmError,
    > {
        create_assembled(&self.adapter, request, |r, s| self.assemble_body(r, s)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::openai::CompatibleConfig;
    use crate::openai::body::test_util::sample_request_with_thinking;
    use crate::{ProviderInfo, ProviderKind};

    #[test]
    fn assemble_body_reselects_hook_after_model_switch() {
        let _guard = crate::provider::lock_provider_for_tests();
        // Long-lived adapter is built once; `/model` only updates the global
        // provider. Body hooks must follow the live model, not construction-time
        // flavor.
        crate::init_provider(ProviderInfo {
            provider: ProviderKind::OpenAi,
            protocol: crate::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: "sk-test".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4o".to_string(),
        });
        let adapter = OpenAiMultiModelAdapter::new(OpenAiAdapter::new(CompatibleConfig::new(
            "sk-test",
            "https://api.openai.com/v1",
        )));
        let request = sample_request_with_thinking();

        let openai_body = adapter.assemble_body(&request, false).unwrap();
        assert_eq!(openai_body["reasoning_effort"], "low");
        assert!(openai_body.get("thinking").is_none());

        crate::set_model("kimi-k2.5").unwrap();
        let kimi_body = adapter.assemble_body(&request, false).unwrap();
        assert_eq!(kimi_body["thinking"]["type"], "enabled");
        assert!(kimi_body.get("reasoning_effort").is_none());
    }
}
