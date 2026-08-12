//! Select an [`OpenAiBodyHook`] from the live provider identity / heuristics.

use std::sync::Arc;

use crate::{
    ChatCompletionsDialect, LlmError, ProviderInfo, ProviderKind,
    openai::body::{DeepSeekBodyHook, KimiBodyHook, OpenAiBodyHook, StandardOpenAiBodyHook},
};

/// Pick the body hook for a resolved Chat Completions dialect.
///
/// `user_id` is only applied when the dialect is DeepSeek.
pub fn hook_for_dialect(
    dialect: ChatCompletionsDialect,
    user_id: Option<&str>,
) -> Arc<dyn OpenAiBodyHook> {
    match dialect {
        ChatCompletionsDialect::Standard => Arc::new(StandardOpenAiBodyHook),
        ChatCompletionsDialect::DeepSeek => {
            Arc::new(DeepSeekBodyHook::new(user_id.map(str::to_owned)))
        }
        ChatCompletionsDialect::Kimi => Arc::new(KimiBodyHook),
    }
}

/// Pick a body hook from the active provider identity / heuristics.
///
/// `user_id` is only applied when the selected hook is DeepSeek.
/// Pick a body hook from the provider identity / heuristics.
///
/// `user_id` is only applied when the selected hook is DeepSeek. `model` is
/// the per-request model id so the hook follows `/model` picks (wire shape
/// matches what is actually requested).
pub fn body_hook_for(
    info: &ProviderInfo,
    model: &str,
    user_id: Option<&str>,
) -> Result<Arc<dyn OpenAiBodyHook>, LlmError> {
    let deepseek = || DeepSeekBodyHook::new(user_id.map(str::to_owned));
    match &info.provider {
        ProviderKind::DeepSeek => Ok(Arc::new(deepseek())),
        ProviderKind::Kimi => Ok(Arc::new(KimiBodyHook)),
        ProviderKind::OpenAi | ProviderKind::Custom(_) => {
            // `provider = openai` may still point at a Moonshot/DeepSeek-compatible
            // base URL or model id — follow endpoint heuristics. Custom
            // providers reuse the same OpenAI-compatible heuristics.
            if info.is_kimi_with(model) {
                Ok(Arc::new(KimiBodyHook))
            } else if info.base_url.contains("deepseek") || model.contains("deepseek") {
                Ok(Arc::new(deepseek()))
            } else {
                Ok(Arc::new(StandardOpenAiBodyHook))
            }
        }
        // Anthropic uses Messages API (`build_anthropic`), never this path.
        ProviderKind::Anthropic => Err(LlmError::UnsupportedHook(
            "cannot use anthropic provider with openai-compatible body hooks".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        OpenAiProtocol, ProviderInfo, ProviderKind, ProviderProfile,
        openai::body::test_util::{ctx, empty_body, sample_request_with_thinking},
    };

    fn provider(kind: ProviderKind, model: &str, base_url: &str) -> ProviderInfo {
        ProviderInfo {
            provider: kind,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            api_key: String::new(),
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }

    fn profile(kind: ProviderKind, model: &str, base_url: &str) -> ProviderProfile {
        ProviderProfile {
            provider: kind,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn body_hook_for_selects_by_kind_and_heuristics() {
        let deepseek = provider(ProviderKind::DeepSeek, "deepseek-chat", "");
        let deepseek_profile = profile(ProviderKind::DeepSeek, "deepseek-chat", "");
        let kimi = provider(ProviderKind::Kimi, "kimi-k2.5", "");
        let kimi_profile = profile(ProviderKind::Kimi, "kimi-k2.5", "");
        let openai_kimi_url = provider(
            ProviderKind::OpenAi,
            "kimi-k2.5",
            "https://api.moonshot.cn/v1",
        );
        let openai_kimi_url_profile = profile(
            ProviderKind::OpenAi,
            "kimi-k2.5",
            "https://api.moonshot.cn/v1",
        );

        let request = sample_request_with_thinking()
            .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::High));
        let mut deepseek_body = empty_body();
        body_hook_for(&deepseek, "deepseek-chat", Some("u1"))
            .unwrap()
            .inject(&mut deepseek_body, &ctx(&request, &deepseek_profile, &[]));
        assert_eq!(deepseek_body["user_id"], "u1");
        assert_eq!(deepseek_body["thinking"]["type"], "enabled");
        assert_eq!(deepseek_body["reasoning_effort"], "high");

        let mut request_kimi = sample_request_with_thinking();
        request_kimi.model = "kimi-k2.5".to_string();
        let mut kimi_body = empty_body();
        body_hook_for(&kimi, "kimi-k2.5", None)
            .unwrap()
            .inject(&mut kimi_body, &ctx(&request_kimi, &kimi_profile, &[]));
        assert_eq!(kimi_body["thinking"]["type"], "enabled");
        assert!(kimi_body.get("reasoning_effort").is_none());

        let mut request_kimi2 = sample_request_with_thinking();
        request_kimi2.model = "kimi-k2.5".to_string();
        let mut heur_body = empty_body();
        body_hook_for(&openai_kimi_url, "kimi-k2.5", None)
            .unwrap()
            .inject(
                &mut heur_body,
                &ctx(&request_kimi2, &openai_kimi_url_profile, &[]),
            );
        assert_eq!(heur_body["thinking"]["type"], "enabled");
        assert!(heur_body.get("reasoning_effort").is_none());
    }
}
