//! Select an [`OpenAiBodyHook`] from the live provider identity / heuristics.

use std::sync::Arc;

use crate::{
    LlmError, ProviderInfo, ProviderKind,
    deepseek::DeepSeekBodyHook,
    kimi::KimiBodyHook,
    openai::{OpenAiBodyHook, StandardOpenAiBodyHook},
};

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
    use crate::{ProviderKind, openai::body::test_util::*};

    #[test]
    fn body_hook_for_selects_by_kind_and_heuristics() {
        let deepseek = provider(ProviderKind::DeepSeek, "deepseek-chat", "");
        let kimi = provider(ProviderKind::Kimi, "kimi-k2.5", "");
        let openai_kimi_url = provider(
            ProviderKind::OpenAi,
            "kimi-k2.5",
            "https://api.moonshot.cn/v1",
        );

        let request = sample_request_with_thinking()
            .with_reasoning_effort(Some(crate::OpenAiReasoningEffort::High));
        let mut deepseek_body = empty_body();
        body_hook_for(&deepseek, "deepseek-chat", Some("u1"))
            .unwrap()
            .inject(&mut deepseek_body, &ctx(&request, &deepseek, &[]));
        assert_eq!(deepseek_body["user_id"], "u1");
        assert_eq!(deepseek_body["thinking"]["type"], "enabled");
        assert_eq!(deepseek_body["reasoning_effort"], "high");

        let mut request_kimi = sample_request_with_thinking();
        request_kimi.model = "kimi-k2.5".to_string();
        let mut kimi_body = empty_body();
        body_hook_for(&kimi, "kimi-k2.5", None)
            .unwrap()
            .inject(&mut kimi_body, &ctx(&request_kimi, &kimi, &[]));
        assert_eq!(kimi_body["thinking"]["type"], "enabled");
        assert!(kimi_body.get("reasoning_effort").is_none());

        let mut request_kimi2 = sample_request_with_thinking();
        request_kimi2.model = "kimi-k2.5".to_string();
        let mut heur_body = empty_body();
        body_hook_for(&openai_kimi_url, "kimi-k2.5", None)
            .unwrap()
            .inject(&mut heur_body, &ctx(&request_kimi2, &openai_kimi_url, &[]));
        assert_eq!(heur_body["thinking"]["type"], "enabled");
        assert!(heur_body.get("reasoning_effort").is_none());
    }
}
