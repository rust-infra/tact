//! xAI (Grok) provider semantics.
//!
//! xAI's API is OpenAI-compatible at the transport level (Chat Completions +
//! SSE streaming, with reasoning arriving via `reasoning_content` deltas that
//! the shared OpenAI adapter already parses). Tact therefore reuses
//! `openai::OpenAiAdapter` for HTTP/SSE and keeps everything xAI-specific in
//! this module:
//!
//! - the default base URL (`https://api.x.ai/v1`)
//! - endpoint detection (`ProviderInfo::is_xai`)
//! - thinking control mapping: Anthropic-style `thinking` is never sent;
//!   models with an adjustable reasoning knob get `reasoning_effort` instead.

use crate::ProviderInfo;

/// Default xAI API endpoint (OpenAI-compatible Chat Completions).
pub const DEFAULT_BASE_URL: &str = "https://api.x.ai/v1";

impl ProviderInfo {
    /// Returns true if the active target is an xAI (Grok) endpoint.
    pub fn is_xai(&self) -> bool {
        self.provider == "xai"
            || self.base_url.contains("api.x.ai")
            || self.model.starts_with("grok-")
    }
}

/// `reasoning_effort` value for an xAI request, if any.
///
/// Per xAI's REST docs, `reasoning_effort` (`none|low|medium|high`) is only
/// accepted by models exposing an adjustable reasoning knob (currently the
/// grok-4.3 family). Always-on reasoning models such as grok-4 and grok-4.5
/// reject the parameter, so nothing is injected for them.
pub(crate) fn reasoning_effort(model: &str, thinking_requested: bool) -> Option<&'static str> {
    if !thinking_requested {
        return None;
    }
    if model.contains("grok-4.3") || model.contains("grok-4-3") {
        Some("high")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(provider: &str, base_url: &str, model: &str) -> ProviderInfo {
        ProviderInfo {
            provider: provider.to_string(),
            api_key: String::new(),
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn is_xai_detection() {
        assert!(provider("xai", "", "grok-4.5").is_xai());
        assert!(provider("openai", "https://api.x.ai/v1", "grok-4.5").is_xai());
        assert!(provider("openai", "https://proxy.example.com/v1", "grok-4.5").is_xai());
        assert!(!provider("openai", "https://api.openai.com/v1", "gpt-4o").is_xai());
        assert!(!provider("kimi", "", "kimi-k2.5").is_xai());
    }

    #[test]
    fn reasoning_effort_only_for_adjustable_models() {
        assert_eq!(reasoning_effort("grok-4.3", true), Some("high"));
        assert_eq!(reasoning_effort("grok-4-3-mini", true), Some("high"));
        assert_eq!(reasoning_effort("grok-4.5", true), None);
        assert_eq!(reasoning_effort("grok-4-0709", true), None);
    }

    #[test]
    fn reasoning_effort_requires_thinking() {
        assert_eq!(reasoning_effort("grok-4.3", false), None);
    }
}
