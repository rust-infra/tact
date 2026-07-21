//! Chat Completions body assembly and provider hook trait.
//!
//! Transport stays in [`super::OpenAiAdapter`]. Provider-specific fields are
//! injected via [`OpenAiBodyHook`] after the shared typed request is built.

use serde_json::Value;

use crate::{CreateMessageParams, LlmError, ProviderInfo, convert::build_openai_request};

/// Context passed to [`OpenAiBodyHook::inject`].
pub struct BodyHookCtx<'a> {
    pub request: &'a CreateMessageParams,
    pub provider: &'a ProviderInfo,
    pub reasoning_per_message: &'a [Option<String>],
}

/// Hook for provider-specific Chat Completions body fields.
pub trait OpenAiBodyHook: Send + Sync {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>);
}

/// OpenAI hook: explicit `reasoning_effort`, falling back to budget bands.
#[derive(Debug, Default, Clone, Copy)]
pub struct StandardOpenAiBodyHook;

impl OpenAiBodyHook for StandardOpenAiBodyHook {
    fn inject(&self, body: &mut Value, ctx: &BodyHookCtx<'_>) {
        crate::inject::inject_openai_reasoning_effort(
            body,
            ctx.request,
            ctx.provider.reasoning_effort,
        );
    }
}

/// Build a Chat Completions JSON body, then run `hook` for provider extras.
pub(crate) fn assemble_chat_completion_body(
    request: &CreateMessageParams,
    stream: bool,
    provider: &ProviderInfo,
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

    let mut body =
        serde_json::to_value(&openai_request).map_err(|e| LlmError::Other(e.to_string()))?;

    let ctx = BodyHookCtx {
        request,
        provider,
        reasoning_per_message: &reasoning_per_message,
    };
    hook.inject(&mut body, &ctx);
    Ok(body)
}

#[cfg(test)]
pub(crate) mod test_util {
    use super::*;
    use crate::{
        ProviderKind, RequiredMessageParams,
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
        provider: &'a ProviderInfo,
        reasoning: &'a [Option<String>],
    ) -> BodyHookCtx<'a> {
        BodyHookCtx {
            request,
            provider,
            reasoning_per_message: reasoning,
        }
    }

    pub(crate) fn provider(kind: ProviderKind, model: &str, base_url: &str) -> ProviderInfo {
        ProviderInfo {
            provider: kind,
            protocol: crate::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: String::new(),
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
    fn openai_hook_uses_reasoning_effort_bands() {
        let request = sample_request_with_thinking();
        let provider = provider(ProviderKind::OpenAi, "o3-mini", "https://api.openai.com/v1");
        let mut body = empty_body();
        StandardOpenAiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));
        assert_eq!(body["reasoning_effort"], "low");
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn openai_hook_omits_reasoning_effort_when_budget_zero() {
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
    fn openai_hook_prefers_explicit_reasoning_effort() {
        let request = sample_request_with_thinking();
        let mut provider = provider(ProviderKind::OpenAi, "gpt-5", "https://api.openai.com/v1");
        provider.reasoning_effort = Some(crate::OpenAiReasoningEffort::Max);
        let mut body = empty_body();

        StandardOpenAiBodyHook.inject(&mut body, &ctx(&request, &provider, &[]));

        assert_eq!(body["reasoning_effort"], "max");
    }
}
