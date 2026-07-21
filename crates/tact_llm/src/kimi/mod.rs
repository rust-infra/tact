//! Kimi / Moonshot Chat Completions adapter (OpenAI-compatible transport).
//!
//! Uses [`OpenAiAdapter`] for HTTP/SSE and always applies [`KimiBodyHook`]
//! for `thinking` / Preserved Thinking and `reasoning_content` echo.
//! Does not send `reasoning_effort` or DeepSeek-style `user_id` (not in Kimi
//! Chat Completions docs).

use serde_json::Value;
use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use crate::inject::{inject_reasoning_content, thinking_budget_enabled};
use crate::openai::body::{BodyHookCtx, OpenAiBodyHook, assemble_chat_completion_body};
use crate::openai::compat::{create_assembled, stream_assembled};
use crate::openai::{CompatibleConfig, OpenAiAdapter};
use crate::{
    ContentBlock, CreateMessageParams, LlmClient, LlmError, LlmRequestBody, ProviderInfo,
    ProviderKind, StopReason,
};

/// Kimi / Moonshot hook: `thinking` object + historical `reasoning_content`.
/// Does **not** send `reasoning_effort` (not in Kimi Chat Completions docs).
#[derive(Debug, Default, Clone, Copy)]
pub struct KimiBodyHook;

impl OpenAiBodyHook for KimiBodyHook {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>) {
        inject_kimi_thinking(body, ctx.request, ctx.provider);
        inject_reasoning_content(body, ctx.reasoning_per_message);
    }
}

fn inject_kimi_thinking(body: &mut Value, request: &CreateMessageParams, provider: &ProviderInfo) {
    // K2.7-code forces thinking on; passing `thinking` (esp. disabled) errors.
    if provider.is_kimi_k27() {
        return;
    }
    if !provider.is_kimi_k2x() {
        return;
    }
    // Kimi defaults thinking to enabled when omitted — send disabled explicitly.
    if thinking_budget_enabled(request).is_none() {
        body["thinking"] = serde_json::json!({ "type": "disabled" });
        return;
    }
    if provider.model.contains("k2.6") || provider.model.contains("k2-6") {
        body["thinking"] = serde_json::json!({
            "type": "enabled",
            "keep": "all",
        });
    } else {
        body["thinking"] = serde_json::json!({
            "type": "enabled",
        });
    }
}

/// Kimi client: OpenAI-compatible wire protocol with Kimi body extras.
#[derive(Clone)]
pub struct KimiAdapter {
    adapter: OpenAiAdapter,
    /// Snapshot used when the live global provider is no longer Kimi.
    model: String,
    base_url: String,
}

impl KimiAdapter {
    pub fn new(config: CompatibleConfig, model: impl Into<String>) -> Self {
        let adapter = OpenAiAdapter::new(config);
        let base_url = adapter.base_url().to_string();
        Self {
            adapter,
            model: model.into(),
            base_url,
        }
    }

    pub fn base_url(&self) -> &str {
        self.adapter.base_url()
    }

    /// Body-hook context is always [`ProviderKind::Kimi`].
    ///
    /// Model / base_url follow the live global provider when it is still Kimi
    /// (so `/model` updates thinking flavor); otherwise fall back to the
    /// construction snapshot. Never trust a non-Kimi global `provider` kind.
    fn body_provider(&self) -> ProviderInfo {
        crate::read_provider(|live| {
            let (model, base_url) = if live.provider == ProviderKind::Kimi {
                (live.model.clone(), live.base_url.clone())
            } else {
                (self.model.clone(), self.base_url.clone())
            };
            ProviderInfo {
                provider: ProviderKind::Kimi,
                protocol: crate::OpenAiProtocol::default(),
                reasoning_effort: None,
                api_key: String::new(),
                base_url,
                model,
            }
        })
    }

    fn assemble_body(
        &self,
        request: &CreateMessageParams,
        stream: bool,
    ) -> Result<Value, LlmError> {
        let provider = self.body_provider();
        assemble_chat_completion_body(request, stream, &provider, &KimiBodyHook)
    }
}

#[async_trait::async_trait]
impl LlmClient for KimiAdapter {
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
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
            Option<LlmRequestBody>,
        ),
        LlmError,
    > {
        create_assembled(&self.adapter, request, |r, s| self.assemble_body(r, s)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RequiredMessageParams;
    use crate::openai::body::test_util::*;

    #[test]
    fn kimi_hook_skips_thinking_for_k27() {
        let request = sample_request_with_thinking();
        let provider = provider(ProviderKind::Kimi, "kimi-k2.7-code", "");
        let mut body = empty_body();
        KimiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn kimi_hook_skips_for_kimi_code_stable_id() {
        let request = sample_request_with_thinking();
        let provider = provider(
            ProviderKind::OpenAi,
            "kimi-for-coding",
            "https://api.kimi.com/coding/v1",
        );
        let mut body = empty_body();
        KimiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn kimi_hook_uses_preserved_thinking_for_k26() {
        let request = sample_request_with_thinking();
        let provider = provider(ProviderKind::Kimi, "kimi-k2.6", "");
        let mut body = empty_body();
        KimiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["keep"], "all");
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn kimi_hook_sends_disabled_when_thinking_off() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "kimi-k2.6".to_string(),
            messages: vec![],
            max_tokens: 1,
        });
        let provider = provider(ProviderKind::Kimi, "kimi-k2.6", "");
        let mut body = empty_body();
        KimiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn kimi_hook_echoes_reasoning_content() {
        let request = sample_request_with_thinking();
        let provider = provider(
            ProviderKind::Kimi,
            "kimi-k2.5",
            "https://api.moonshot.cn/v1",
        );
        let mut body = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "", "tool_calls": []},
                {"role": "tool", "content": "ok", "tool_call_id": "1"}
            ]
        });
        let reasoning = vec![None, Some("let me think".to_string()), None];
        KimiBodyHook.inject(&mut body, &ctx(&request, &provider, &reasoning));
        assert_eq!(body["messages"][1]["reasoning_content"], "let me think");
        assert_eq!(body["thinking"]["type"], "enabled");
        assert!(body.get("reasoning_effort").is_none());
    }
}
