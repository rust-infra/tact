//! Chat Completions body assembly and provider hook trait.
//!
//! Transport stays in [`super::OpenAiAdapter`]. Provider-specific fields are
//! injected via [`OpenAiBodyHook`] after the shared typed request is built.

use serde_json::Value;

use crate::{
    CreateMessageParams, LlmError, ProviderProfile, convert::build_openai_request,
    inject::{inject_reasoning_content, inject_user_id, thinking_budget_enabled},
};

/// Context passed to [`OpenAiBodyHook::inject`].
pub struct BodyHookCtx<'a> {
    pub request: &'a CreateMessageParams,
    pub profile: &'a ProviderProfile,
    pub reasoning_per_message: &'a [Option<String>],
}

/// Hook for provider-specific Chat Completions body fields.
pub trait OpenAiBodyHook: Send + Sync {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>);
}

/// OpenAI hook: explicit per-request `reasoning_effort` (no budget bands).
#[derive(Debug, Default, Clone, Copy)]
pub struct StandardOpenAiBodyHook;

impl OpenAiBodyHook for StandardOpenAiBodyHook {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>) {
        crate::inject::inject_openai_reasoning_effort(body, ctx.request);
    }
}

/// DeepSeek hook (official OpenAI format):
/// `thinking` + `reasoning_effort` (`high` / `max`) + `user_id`.
/// Does not replay `reasoning_content` (see `deepseek` design notes).
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
        inject_kimi_thinking(body, ctx.request, ctx.profile);
        inject_reasoning_content(body, ctx.reasoning_per_message);
    }
}

fn inject_kimi_thinking(
    body: &mut Value,
    request: &CreateMessageParams,
    profile: &ProviderProfile,
) {
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
    if profile.is_kimi_k27(model) {
        return;
    }
    if !profile.is_kimi_k2x(model) {
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

/// Build a Chat Completions JSON body, then run `hook` for provider extras.
pub(crate) fn assemble_chat_completion_body(
    request: &CreateMessageParams,
    stream: bool,
    profile: &ProviderProfile,
    hook: &dyn OpenAiBodyHook,
) -> Result<Value, LlmError> {
    let (mut openai_request, reasoning_per_message) = build_openai_request(request);
    if stream {
        openai_request.stream = Some(true);
        openai_request.stream_options = Some(super::STREAM_OPTIONS_WITH_USAGE);
    } else {
        openai_request.stream = Some(false);
        openai_request.stream_options = None;
    }

    let mut body = serde_json::to_value(&openai_request)?;

    let ctx = BodyHookCtx {
        request,
        profile,
        reasoning_per_message: &reasoning_per_message,
    };
    hook.inject(&mut body, &ctx);
    Ok(body)
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use crate::{
        OpenAiProtocol, ProviderKind, ProviderProfile, RequiredMessageParams,
        types::{Thinking as RequestThinking, ThinkingType},
    };

    pub(crate) fn sample_request_with_thinking() -> CreateMessageParams {
        CreateMessageParams::new(RequiredMessageParams {
            model: "test-model".to_string(),
            messages: vec![],
            max_tokens: 1,
        })
        .with_thinking(RequestThinking {
            budget_tokens: 1000,
            type_: ThinkingType::Enabled,
        })
    }

    pub(crate) fn empty_body() -> Value {
        serde_json::json!({
            "model": "test",
            "messages": []
        })
    }

    pub(crate) fn ctx<'a>(
        request: &'a CreateMessageParams,
        profile: &'a ProviderProfile,
        reasoning: &'a [Option<String>],
    ) -> BodyHookCtx<'a> {
        BodyHookCtx {
            request,
            profile,
            reasoning_per_message: reasoning,
        }
    }

    pub(crate) fn provider(kind: ProviderKind, model: &str, base_url: &str) -> ProviderProfile {
        ProviderProfile {
            provider: kind,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{test_util::*, *};
    use crate::{
        ProviderKind, RequiredMessageParams,
        types::{Thinking as RequestThinking, ThinkingType},
    };

    #[test]
    fn openai_hook_injects_explicit_effort() {
        let request = sample_request_with_thinking()
            .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::Low));
        let provider = provider(ProviderKind::OpenAi, "o3-mini", "https://api.openai.com/v1");
        let mut body = empty_body();
        StandardOpenAiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn openai_hook_omits_reasoning_effort_when_none() {
        let request = CreateMessageParams::new(RequiredMessageParams {
            model: "o3-mini".to_string(),
            messages: vec![],
            max_tokens: 1,
        })
        .with_thinking(RequestThinking {
            budget_tokens: 0,
            type_: ThinkingType::Enabled,
        });
        let provider = provider(ProviderKind::OpenAi, "o3-mini", "https://api.openai.com/v1");
        let mut body = empty_body();
        StandardOpenAiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert!(body.get("reasoning_effort").is_none());
    }

    #[test]
    fn openai_hook_injects_explicit_max() {
        let request = sample_request_with_thinking()
            .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::Max));
        let provider = provider(ProviderKind::OpenAi, "gpt-5", "https://api.openai.com/v1");
        let mut body = empty_body();

        StandardOpenAiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));

        assert_eq!(body["reasoning_effort"], "max");
    }

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
