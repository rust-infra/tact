//! Credential-free provider configuration snapshot and endpoint heuristics.
//!
//! [`ProviderProfile`] is the stable configuration type passed into adapters.
//! Secrets are resolved separately through [`crate::CredentialProvider`], so a
//! profile can be cloned, cached, and compared without leaking credentials.

use crate::{
    ChatCompletionsDialect, LlmError, OpenAiProtocol, ProviderKind,
    openai::responses::ResponsesCapabilities,
};

/// Configuration snapshot for one LLM provider, without credentials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderProfile {
    pub provider: ProviderKind,
    pub base_url: String,
    /// Static configured model id. Request-level model switches travel in
    /// [`crate::CreateMessageParams::model`] and may differ from this value.
    pub model: String,
    pub protocol: OpenAiProtocol,
    /// Optional OpenAI Responses `context_management.compact_threshold`.
    pub responses_compact_threshold: Option<u32>,
}

impl ProviderProfile {
    /// Returns the conservative capabilities for the configured Responses endpoint.
    pub fn responses_capabilities(&self) -> Option<ResponsesCapabilities> {
        if self.protocol != OpenAiProtocol::Responses {
            return None;
        }
        Some(match self.provider {
            ProviderKind::OpenAi => ResponsesCapabilities::official_openai(),
            ProviderKind::Anthropic => return None,
            ProviderKind::DeepSeek | ProviderKind::Kimi | ProviderKind::Custom(_) => {
                ResponsesCapabilities::custom_provider()
            }
        })
    }

    /// Returns the provider's default base URL when one is built in.
    pub fn default_base_url(&self) -> Option<&'static str> {
        self.provider.default_base_url()
    }

    /// True when the configured provider/target is Kimi/Moonshot.
    pub fn is_kimi(&self) -> bool {
        self.is_kimi_with(&self.model)
    }

    /// Kimi endpoint check for an arbitrary model id (per-request wire shape).
    pub fn is_kimi_with(&self, model: &str) -> bool {
        self.provider == ProviderKind::Kimi
            || self.base_url.contains("moonshot")
            || self.base_url.contains("kimi")
            || model.contains("kimi")
    }

    /// Returns true for the Kimi K2.x family (K2.5, K2.6, K2.7-code, ...).
    ///
    /// Also covers the stable `kimi-for-coding` model ID and the Kimi Code
    /// platform endpoint (`api.kimi.com/coding`), both of which always serve
    /// the latest K2.x coding model. `model` is the per-request model id so
    /// wire shape follows `/model` picks.
    pub fn is_kimi_k2x(&self, model: &str) -> bool {
        if !self.is_kimi_with(model) {
            return false;
        }
        if model == "kimi-for-coding" || self.base_url.contains("kimi.com/coding") {
            return true;
        }
        model.contains("kimi-k2") || model.contains("k2.") || model.contains("k2-")
    }

    /// Returns true specifically for K2.7-code and the Kimi Code stable model.
    ///
    /// `kimi-for-coding` and the `api.kimi.com/coding` endpoint currently map
    /// to the latest K2.7-code model. `model` is the per-request model id.
    pub fn is_kimi_k27(&self, model: &str) -> bool {
        if !self.is_kimi_with(model) {
            return false;
        }
        if model == "kimi-for-coding" || self.base_url.contains("kimi.com/coding") {
            return true;
        }
        model.contains("k2.7") || model.contains("k2-7")
    }

    /// Returns true for the Kimi Code platform, which has no balance API.
    ///
    /// Matches the official endpoint (`api.kimi.com/coding`) as well as the
    /// stable `kimi-for-coding` model ID served through a custom proxy.
    /// `model` is the per-request model id.
    pub fn is_kimi_coding(&self, model: &str) -> bool {
        self.base_url.contains("kimi.com/coding") || model == "kimi-for-coding"
    }

    /// Returns true when Kimi balance queries are supported for the configured endpoint.
    pub fn is_kimi_balance_supported(&self) -> bool {
        if !self.is_kimi() || self.is_kimi_coding(&self.model) {
            return false;
        }
        if self.base_url.is_empty() {
            return self.provider == ProviderKind::Kimi;
        }
        reqwest::Url::parse(&self.base_url).is_ok_and(|url| {
            url.scheme() == "https"
                && matches!(url.host_str(), Some("api.moonshot.cn" | "api.moonshot.ai"))
        })
    }

    /// Returns true when Kimi Code usage quota queries are supported.
    ///
    /// Kimi Code serves `GET /v1/usages` only on the official endpoint
    /// (`https://api.kimi.com/coding`); custom proxies / gateways are treated
    /// as unsupported so their API keys are never sent to `api.kimi.com`.
    pub fn is_kimi_usage_supported(&self) -> bool {
        if !self.is_kimi_coding(&self.model) {
            return false;
        }
        reqwest::Url::parse(&self.base_url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("api.kimi.com")
                && url.path().contains("coding")
        })
    }

    /// Returns true when DeepSeek balance queries are supported for the configured endpoint.
    ///
    /// DeepSeek serves `GET /user/balance` only on the official endpoint;
    /// custom proxies / gateways are treated as unsupported so their API keys
    /// are never sent to `api.deepseek.com`.
    pub fn is_deepseek_balance_supported(&self) -> bool {
        let is_deepseekish = self.provider == ProviderKind::DeepSeek
            || self.base_url.contains("deepseek")
            || self.model.contains("deepseek");
        if !is_deepseekish {
            return false;
        }
        if self.base_url.is_empty() {
            return self.provider == ProviderKind::DeepSeek;
        }
        reqwest::Url::parse(&self.base_url)
            .is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("api.deepseek.com"))
    }

    /// Returns true when account balance or usage quota queries are supported.
    pub fn is_account_query_supported(&self) -> bool {
        self.is_deepseek_balance_supported()
            || self.is_kimi_balance_supported()
            || self.is_kimi_usage_supported()
    }

    /// Whether this provider+model supports image (vision) input.
    ///
    /// Delegates to [`ProviderKind::supports_vision`]. We also check for
    /// DeepSeek-like endpoints routed through the OpenAI provider.
    pub fn supports_vision(&self) -> bool {
        if self.provider == ProviderKind::DeepSeek
            || self.base_url.contains("deepseek")
            || self.model.contains("deepseek")
        {
            return false;
        }
        self.provider.supports_vision()
    }

    /// Whether the given model id uses reasoning-effort semantics (vs thinking
    /// budget), following the same heuristics as the wire body hooks.
    pub fn model_uses_effort(&self, model: &str) -> bool {
        match &self.provider {
            ProviderKind::DeepSeek => true,
            ProviderKind::Anthropic => false,
            ProviderKind::Kimi => is_kimi_k3(model),
            ProviderKind::OpenAi | ProviderKind::Custom(_) => {
                if self.is_kimi_with(model) {
                    is_kimi_k3(model)
                } else {
                    true
                }
            }
        }
    }

    /// Returns the Chat Completions dialect for a request-level model.
    ///
    /// # Errors
    ///
    /// Returns [`LlmError::UnsupportedHook`] for providers that never speak
    /// Chat Completions (Anthropic).
    pub fn dialect_for(&self, model: &str) -> Result<ChatCompletionsDialect, LlmError> {
        match &self.provider {
            ProviderKind::DeepSeek => Ok(ChatCompletionsDialect::DeepSeek),
            ProviderKind::Kimi => Ok(ChatCompletionsDialect::Kimi),
            ProviderKind::OpenAi | ProviderKind::Custom(_) => {
                if self.is_kimi_with(model) {
                    Ok(ChatCompletionsDialect::Kimi)
                } else if self.base_url.contains("deepseek") || model.contains("deepseek") {
                    Ok(ChatCompletionsDialect::DeepSeek)
                } else {
                    Ok(ChatCompletionsDialect::Standard)
                }
            }
            ProviderKind::Anthropic => Err(LlmError::UnsupportedHook(
                "cannot use anthropic provider with openai-compatible body hooks".to_owned(),
            )),
        }
    }
}

/// True for the Kimi K3 family (`k3`, `k3-256k`) which exposes
/// `reasoning_effort: low/high/max` (default high).
pub fn is_kimi_k3(model: &str) -> bool {
    model == "k3" || model == "k3-256k"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(provider: ProviderKind, base_url: &str, model: &str) -> ProviderProfile {
        ProviderProfile {
            provider,
            base_url: base_url.to_string(),
            model: model.to_string(),
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
        }
    }

    #[test]
    fn kimi_heuristics_follow_kind_url_and_model() {
        let kimi = profile(
            ProviderKind::Kimi,
            "https://api.moonshot.cn/v1",
            "kimi-k2.5",
        );
        assert!(kimi.is_kimi());
        assert!(kimi.is_kimi_k2x("kimi-k2.5"));
        assert!(!kimi.is_kimi_k27("kimi-k2.5"));
        assert!(!kimi.is_kimi_coding("kimi-k2.5"));
        assert!(kimi.is_kimi_balance_supported());
        assert!(!kimi.is_kimi_usage_supported());

        let coding = profile(
            ProviderKind::OpenAi,
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(coding.is_kimi_k2x("kimi-for-coding"));
        assert!(coding.is_kimi_k27("kimi-for-coding"));
        assert!(coding.is_kimi_coding("kimi-for-coding"));
        assert!(!coding.is_kimi_balance_supported());
        assert!(coding.is_kimi_usage_supported());

        let proxy = profile(
            ProviderKind::OpenAi,
            "https://proxy.example.com/v1",
            "kimi-k2.5",
        );
        assert!(!proxy.is_kimi_balance_supported());
        assert!(!proxy.is_account_query_supported());
    }

    #[test]
    fn deepseek_balance_only_on_official_endpoint() {
        let official = profile(
            ProviderKind::DeepSeek,
            "https://api.deepseek.com/v1",
            "deepseek-chat",
        );
        assert!(official.is_deepseek_balance_supported());
        assert!(official.is_account_query_supported());

        let empty = profile(ProviderKind::DeepSeek, "", "deepseek-chat");
        assert!(empty.is_deepseek_balance_supported());

        let proxy = profile(
            ProviderKind::OpenAi,
            "https://proxy.example.com/v1",
            "deepseek-chat",
        );
        assert!(!proxy.is_deepseek_balance_supported());

        let spoofed = profile(
            ProviderKind::DeepSeek,
            "https://api.deepseek.com.evil.example/v1",
            "deepseek-chat",
        );
        assert!(!spoofed.is_deepseek_balance_supported());
    }

    #[test]
    fn supports_vision_rejects_deepseek_like_targets() {
        assert!(
            !profile(
                ProviderKind::DeepSeek,
                "https://api.deepseek.com",
                "deepseek-chat"
            )
            .supports_vision()
        );
        assert!(
            !profile(
                ProviderKind::OpenAi,
                "https://proxy.example.com/v1",
                "deepseek-chat"
            )
            .supports_vision()
        );
        assert!(
            profile(ProviderKind::OpenAi, "https://api.openai.com/v1", "gpt-5").supports_vision()
        );
    }

    #[test]
    fn responses_capabilities_follow_protocol_and_provider() {
        let mut responses = profile(
            ProviderKind::Custom("gateway".into()),
            "https://g.example",
            "gpt-5",
        );
        responses.protocol = OpenAiProtocol::Responses;
        let capabilities = responses
            .responses_capabilities()
            .expect("responses capabilities");
        assert!(capabilities.responses);
        assert_eq!(
            capabilities.hosted_tools,
            std::collections::BTreeSet::from([
                crate::openai::responses::ResponsesToolKind::WebSearch
            ])
        );

        let chat = profile(ProviderKind::OpenAi, "https://api.openai.com/v1", "gpt-5");
        assert!(chat.responses_capabilities().is_none());
    }

    #[test]
    fn dialect_for_matches_hook_selection_semantics() {
        assert_eq!(
            profile(ProviderKind::DeepSeek, "", "deepseek-chat")
                .dialect_for("deepseek-chat")
                .unwrap(),
            ChatCompletionsDialect::DeepSeek
        );
        assert_eq!(
            profile(ProviderKind::Kimi, "", "kimi-k2.5")
                .dialect_for("kimi-k2.5")
                .unwrap(),
            ChatCompletionsDialect::Kimi
        );
        assert_eq!(
            profile(ProviderKind::OpenAi, "https://api.moonshot.cn/v1", "")
                .dialect_for("x")
                .unwrap(),
            ChatCompletionsDialect::Kimi
        );
        assert_eq!(
            profile(ProviderKind::OpenAi, "https://api.deepseek.com", "")
                .dialect_for("x")
                .unwrap(),
            ChatCompletionsDialect::DeepSeek
        );
        assert_eq!(
            profile(ProviderKind::OpenAi, "https://api.openai.com/v1", "")
                .dialect_for("gpt-5")
                .unwrap(),
            ChatCompletionsDialect::Standard
        );
        assert!(
            profile(
                ProviderKind::Anthropic,
                "https://api.anthropic.com",
                "claude"
            )
            .dialect_for("claude")
            .is_err()
        );
    }

    #[test]
    fn dialect_user_id_support_is_deepseek_only() {
        assert!(ChatCompletionsDialect::DeepSeek.supports_user_id());
        assert!(!ChatCompletionsDialect::Standard.supports_user_id());
        assert!(!ChatCompletionsDialect::Kimi.supports_user_id());
    }
}
