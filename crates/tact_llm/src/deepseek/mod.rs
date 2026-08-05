//! DeepSeek Chat Completions adapter (OpenAI-compatible transport).
//!
//! Uses [`OpenAiAdapter`] for HTTP/SSE and always applies [`DeepSeekBodyHook`]
//! for `thinking` + `reasoning_effort` + `user_id`.
//!
//! Does **not** echo historical `reasoning_content`: live API accepts tool
//! turns without it, and omitting it preserves DeepSeek prefix KV-cache hits
//! (Kimi still requires echo via [`crate::kimi::KimiBodyHook`]).

use serde_json::Value;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    CreateMessageParams, LlmClient, LlmError, LlmResponse, ProviderConversationState,
    inject::inject_user_id,
    openai::{
        CompatibleConfig, OpenAiAdapter,
        body::{BodyHookCtx, OpenAiBodyHook, assemble_chat_completion_body},
        compat::{create_assembled, stream_assembled},
    },
};

/// DeepSeek hook (official OpenAI format):
/// `thinking` + `reasoning_effort` (`high` / `max`) + `user_id`.
/// Does not replay `reasoning_content` (see module docs).
#[derive(Debug, Default, Clone)]
pub struct DeepSeekBodyHook {
    user_id: Option<String>,
}

impl DeepSeekBodyHook {
    pub fn new(user_id: Option<String>) -> Self {
        Self { user_id }
    }
}

impl OpenAiBodyHook for DeepSeekBodyHook {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>) {
        inject_deepseek_thinking(body, ctx.request);
        inject_user_id(body, self.user_id.as_deref());
    }
}

/// DeepSeek official thinking: effort-driven (no budget on the wire).
///
/// Docs (api-docs.deepseek.com/zh-cn/guides/thinking_mode): effort is
/// `low/high/xhigh/max`, forwarded as-is (the server maps per model, e.g.
/// flash vs pro). Thinking defaults ON with effort high, so `None` omits both;
/// `Some` sends `thinking: enabled` + the raw effort. `minimal`/`medium` are
/// not legal and must never reach this hook (UI tiers exclude them).
fn inject_deepseek_thinking(body: &mut Value, request: &CreateMessageParams) {
    match request.reasoning_effort {
        Some(effort) => {
            body["thinking"] = serde_json::json!({ "type": "enabled" });
            body["reasoning_effort"] = Value::String(effort.as_str().to_owned());
        }
        None => {
            // DeepSeek defaults thinking ON + effort high; omit = default.
        }
    }
}

/// DeepSeek client: OpenAI-compatible wire protocol with DeepSeek body extras.
#[derive(Clone)]
pub struct DeepSeekAdapter {
    adapter: OpenAiAdapter,
    user_id: Option<String>,
}

impl DeepSeekAdapter {
    pub fn new(config: CompatibleConfig) -> Self {
        Self {
            adapter: OpenAiAdapter::new(config),
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
    ) -> Result<Value, LlmError> {
        crate::read_provider(|provider| {
            assemble_chat_completion_body(
                request,
                stream,
                provider,
                &DeepSeekBodyHook::new(self.user_id.clone()),
            )
        })
    }
}

impl LlmClient for DeepSeekAdapter {
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
    use crate::{ProviderKind, RequiredMessageParams, openai::body::test_util::*};

    #[test]
    fn deepseek_hook_pairs_thinking_and_effort() {
        let request = sample_request_with_thinking()
            .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::High));
        let provider = provider(
            ProviderKind::DeepSeek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
        );
        let mut body = empty_body();
        let uid = "a1b2c3d4-5678-90ab-cdef-1234567890ab";
        DeepSeekBodyHook::new(Some(uid.to_string()))
            .inject(&mut body, &ctx(&request, &provider, &[]));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "high");
        assert_eq!(body["user_id"], uid);
    }

    #[test]
    fn deepseek_hook_forwards_max_effort_raw() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![],
            max_tokens: 1,
        })
        .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::Max));
        let provider = provider(
            ProviderKind::DeepSeek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
        );
        let mut body = empty_body();
        DeepSeekBodyHook::default().inject(&mut body, &ctx(&request, &provider, &[]));
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["reasoning_effort"], "max");
    }

    #[test]
    fn deepseek_hook_omits_fields_when_effort_unset() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![],
            max_tokens: 1,
        });
        let provider = provider(
            ProviderKind::DeepSeek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
        );
        let mut body = empty_body();
        DeepSeekBodyHook::default().inject(&mut body, &ctx(&request, &provider, &[]));
        // DeepSeek defaults thinking ON + effort high; None = omit both.
        assert!(body.get("thinking").is_none());
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_hook_does_not_echo_reasoning_content() {
        let request = sample_request_with_thinking();
        let provider = provider(
            ProviderKind::DeepSeek,
            "deepseek-v4-pro",
            "https://api.deepseek.com",
        );
        let mut body = serde_json::json!({
            "messages": [
                {"role": "user", "content": "hi"},
                {"role": "assistant", "content": "", "tool_calls": []},
                {"role": "tool", "content": "ok", "tool_call_id": "1"}
            ]
        });
        let reasoning = vec![None, Some("plan tool".to_string()), None];
        DeepSeekBodyHook::default().inject(&mut body, &ctx(&request, &provider, &reasoning));
        assert!(
            body["messages"][1].get("reasoning_content").is_none(),
            "DeepSeek must omit historical reasoning_content for prefix cache"
        );
    }
}
