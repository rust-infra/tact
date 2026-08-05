//! Kimi / Moonshot Chat Completions adapter (OpenAI-compatible transport).
//!
//! Uses [`OpenAiAdapter`] for HTTP/SSE and always applies [`KimiBodyHook`]
//! for `thinking` / Preserved Thinking and `reasoning_content` echo.
//! Does not send `reasoning_effort` or DeepSeek-style `user_id` (not in Kimi
//! Chat Completions docs).

use serde_json::Value;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    CreateMessageParams, LlmClient, LlmError, LlmResponse, ProviderConversationState, ProviderInfo,
    ProviderKind,
    inject::{inject_reasoning_content, thinking_budget_enabled},
    openai::{
        CompatibleConfig, OpenAiAdapter,
        body::{BodyHookCtx, OpenAiBodyHook, assemble_chat_completion_body},
        compat::{create_assembled, stream_assembled},
    },
};

/// Kimi / Moonshot hook: `thinking` object + historical `reasoning_content`.
///
/// - k3 / k3-256k: explicit `reasoning_effort` (low/high/max, default high;
///   [docs](https://www.kimi.com/code/docs/kimi-code/models.html)). `Some` →
///   thinking enabled + raw effort; `None` → omit (server default enabled +
///   high). Never send `thinking: disabled` — it routes K3/K2.7 to K2.6.
/// - kimi-for-coding / highspeed (K2.7 Code): Thinking:ON fixed, skip.
/// - other K2.x: budget>0 → enabled (keep all for k2.6), else disabled.
#[derive(Debug, Default, Clone, Copy)]
pub struct KimiBodyHook;

impl OpenAiBodyHook for KimiBodyHook {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>) {
        inject_kimi_thinking(body, ctx.request, ctx.provider);
        inject_reasoning_content(body, ctx.reasoning_per_message);
    }
}

fn inject_kimi_thinking(body: &mut Value, request: &CreateMessageParams, provider: &ProviderInfo) {
    let model = request.model.as_str();
    // K3 / K3-256k: effort-driven.
    if crate::is_kimi_k3(model) {
        if let Some(effort) = request.reasoning_effort {
            body["thinking"] = serde_json::json!({ "type": "enabled" });
            body["reasoning_effort"] = Value::String(effort.as_str().to_owned());
        }
        // None → omit: Kimi K3 defaults thinking enabled + effort high.
        return;
    }
    // K2.7-code forces thinking on; passing `thinking` (esp. disabled) errors.
    if provider.is_kimi_k27(model) {
        return;
    }
    if !provider.is_kimi_k2x(model) {
        return;
    }
    // Kimi defaults thinking to enabled when omitted — send disabled explicitly.
    if thinking_budget_enabled(request).is_none() {
        body["thinking"] = serde_json::json!({ "type": "disabled" });
        return;
    }
    if model.contains("k2.6") || model.contains("k2-6") {
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
    /// Static provider identity snapshot (model / base_url / kind).
    provider: ProviderInfo,
}

impl KimiAdapter {
    pub fn new(config: CompatibleConfig, model: impl Into<String>) -> Self {
        let adapter = OpenAiAdapter::new(config);
        let base_url = adapter.base_url().to_string();
        Self {
            adapter,
            provider: ProviderInfo {
                provider: ProviderKind::Kimi,
                protocol: crate::OpenAiProtocol::default(),
                responses_compact_threshold: None,
                api_key: String::new(),
                base_url,
                model: model.into(),
            },
        }
    }

    pub fn base_url(&self) -> &str {
        self.adapter.base_url()
    }

    fn assemble_body(
        &self,
        request: &CreateMessageParams,
        stream: bool,
    ) -> Result<Value, LlmError> {
        assemble_chat_completion_body(request, stream, &self.provider, &KimiBodyHook)
    }
}

impl LlmClient for KimiAdapter {
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        provider_state: Option<&ProviderConversationState>,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
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
    use crate::{RequiredMessageParams, openai::body::test_util::*};

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
        let mut request = sample_request_with_thinking();
        request.model = "kimi-for-coding".to_string();
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
        let mut request = sample_request_with_thinking();
        request.model = "kimi-k2.6".to_string();
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
        let mut request = sample_request_with_thinking();
        request.model = "kimi-k2.5".to_string();
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
