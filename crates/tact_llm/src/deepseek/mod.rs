//! DeepSeek Chat Completions adapter (OpenAI-compatible transport).
//!
//! Uses [`OpenAiAdapter`] for HTTP/SSE and always applies [`DeepSeekBodyHook`]
//! for `thinking` + `reasoning_effort` + `user_id`.
//!
//! Does **not** echo historical `reasoning_content`: live API accepts tool
//! turns without it, and omitting it preserves DeepSeek prefix KV-cache hits
//! (Kimi still requires echo via [`crate::kimi::KimiBodyHook`]).

use serde_json::Value;
use tact_protocol::{AgentUpdate, TokenUsageInfo};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    ContentBlock, CreateMessageParams, LlmClient, LlmError, LlmRequestBody, StopReason,
    inject::{inject_user_id, thinking_budget_enabled},
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

/// DeepSeek official pair: `thinking` + `reasoning_effort` (`high` / `max`).
///
/// Docs: native effort is `high`/`max`; `low`/`medium` map to `high`. Default
/// thinking toggle is enabled, so disabled must be sent explicitly.
fn inject_deepseek_thinking(body: &mut Value, request: &CreateMessageParams) {
    match thinking_budget_enabled(request) {
        Some(budget) => {
            body["thinking"] = serde_json::json!({ "type": "enabled" });
            // Map our budget bands onto DeepSeek's native high/max.
            let effort = if budget > 32_000 { "max" } else { "high" };
            body["reasoning_effort"] = Value::String(effort.to_owned());
        }
        None => {
            body["thinking"] = serde_json::json!({ "type": "disabled" });
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

#[async_trait::async_trait]
impl LlmClient for DeepSeekAdapter {
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
    use crate::{
        ProviderKind, RequiredMessageParams,
        openai::body::test_util::*,
        types::{Thinking as RequestThinking, ThinkingType},
    };

    #[test]
    fn deepseek_hook_pairs_thinking_and_effort() {
        let request = sample_request_with_thinking();
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
    fn deepseek_hook_maps_high_budget_to_max() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "deepseek-v4-pro".to_string(),
            messages: vec![],
            max_tokens: 1,
        })
        .with_thinking(RequestThinking {
            budget_tokens: 32_001,
            type_: ThinkingType::Enabled,
        });
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
    fn deepseek_hook_disables_thinking_when_off() {
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
        assert_eq!(body["thinking"]["type"], "disabled");
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
