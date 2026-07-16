//! Active provider configuration and client construction.

use std::sync::RwLock;

use crate::anthropic;
use crate::client::LlmProvider;
use crate::deepseek;
use crate::kimi;
use crate::openai;
use crate::types::ProviderKind;

/// Holds private LLM configuration information.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub provider: ProviderKind,
}

impl Default for ProviderInfo {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            provider: ProviderKind::OpenAi,
        }
    }
}

impl ProviderInfo {
    /// Build an LLM client for this provider configuration.
    pub fn build_client(&self) -> anyhow::Result<LlmProvider> {
        match self.provider {
            ProviderKind::Anthropic => self.build_anthropic(),
            ProviderKind::DeepSeek => self.build_deepseek(),
            ProviderKind::Kimi => self.build_kimi(),
            ProviderKind::OpenAi => self.build_openai_compatible(),
        }
    }

    /// Build an Anthropic Messages API client.
    fn build_anthropic(&self) -> anyhow::Result<LlmProvider> {
        if self.api_key.is_empty() {
            anyhow::bail!("api_key not configured for provider '{}'", self.provider);
        }
        if self.base_url.is_empty() {
            anyhow::bail!("base_url not configured for provider '{}'", self.provider);
        }
        Ok(LlmProvider::Anthropic(anthropic::AnthropicAdapter::new(
            self.api_key.clone(),
            self.base_url.clone(),
        )))
    }

    /// Build a dedicated DeepSeek Chat Completions client.
    fn build_deepseek(&self) -> anyhow::Result<LlmProvider> {
        let config = self.openai_compatible_config()?;
        Ok(LlmProvider::DeepSeek(deepseek::DeepSeekAdapter::new(
            config,
        )))
    }

    /// Build a dedicated Kimi / Moonshot Chat Completions client.
    fn build_kimi(&self) -> anyhow::Result<LlmProvider> {
        let config = self.openai_compatible_config()?;
        Ok(LlmProvider::Kimi(kimi::KimiAdapter::new(
            config,
            self.model.clone(),
        )))
    }

    /// Build an OpenAI-compatible (Chat Completions API) client.
    fn build_openai_compatible(&self) -> anyhow::Result<LlmProvider> {
        let config = self.openai_compatible_config()?;
        let adapter = openai::OpenAiAdapter::new(config);
        Ok(LlmProvider::OpenAi(openai::OpenAiMultiModelAdapter::new(
            adapter,
        )))
    }

    fn openai_compatible_config(&self) -> anyhow::Result<openai::CompatibleConfig> {
        if self.api_key.is_empty() {
            anyhow::bail!("api_key not configured for provider '{}'", self.provider);
        }
        let base_url = if self.base_url.is_empty() {
            self.provider
                .default_base_url()
                .map(str::to_string)
                .ok_or_else(|| {
                    anyhow::anyhow!("no default base_url for provider '{}'", self.provider)
                })?
        } else {
            self.base_url.clone()
        };
        Ok(openai::CompatibleConfig::new(
            self.api_key.clone(),
            base_url,
        ))
    }

    /// Returns true if the active target is a Kimi/Moonshot endpoint.
    pub fn is_kimi(&self) -> bool {
        self.provider == ProviderKind::Kimi
            || self.base_url.contains("moonshot")
            || self.base_url.contains("kimi")
            || self.model.contains("kimi")
    }

    /// Returns true for the Kimi K2.x family (K2.5, K2.6, K2.7-code, ...).
    ///
    /// Also covers the stable `kimi-for-coding` model ID and the Kimi Code
    /// platform endpoint (`api.kimi.com/coding`), both of which always serve
    /// the latest K2.x coding model.
    pub fn is_kimi_k2x(&self) -> bool {
        if !self.is_kimi() {
            return false;
        }
        if self.model == "kimi-for-coding" || self.base_url.contains("kimi.com/coding") {
            return true;
        }
        self.model.contains("kimi-k2") || self.model.contains("k2.") || self.model.contains("k2-")
    }

    /// Returns true specifically for K2.7-code and the Kimi Code stable model.
    ///
    /// `kimi-for-coding` and the `api.kimi.com/coding` endpoint currently map
    /// to the latest K2.7-code model.
    pub fn is_kimi_k27(&self) -> bool {
        if !self.is_kimi() {
            return false;
        }
        if self.model == "kimi-for-coding" || self.base_url.contains("kimi.com/coding") {
            return true;
        }
        self.model.contains("k2.7") || self.model.contains("k2-7")
    }

    /// Returns true for the Kimi Code platform, which has no balance API.
    ///
    /// Matches the official endpoint (`api.kimi.com/coding`) as well as the
    /// stable `kimi-for-coding` model ID served through a custom proxy.
    pub fn is_kimi_coding(&self) -> bool {
        self.base_url.contains("kimi.com/coding") || self.model == "kimi-for-coding"
    }

    /// Returns true when Kimi balance queries are supported for the configured endpoint.
    pub fn is_kimi_balance_supported(&self) -> bool {
        self.is_kimi() && !self.is_kimi_coding()
    }

    /// Returns true when Kimi Code usage quota queries are supported.
    pub fn is_kimi_usage_supported(&self) -> bool {
        self.is_kimi_coding()
    }

    /// Returns true when account balance or usage quota queries are supported.
    pub fn is_account_query_supported(&self) -> bool {
        self.provider == ProviderKind::DeepSeek
            || self.base_url.contains("deepseek")
            || self.model.contains("deepseek")
            || self.is_kimi_balance_supported()
            || self.is_kimi_usage_supported()
    }
}

/// The active LLM provider configuration (mutable so `/model` can switch models).
static PROVIDER: RwLock<Option<ProviderInfo>> = RwLock::new(None);

/// Serialize tests that mutate/read the process-global provider snapshot.
#[cfg(test)]
pub(crate) fn lock_provider_for_tests() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().expect("provider test lock poisoned")
}

/// Install the active LLM provider configuration.
///
/// Safe to call again under `test-support` overrides; production `install` still
/// runs once per process.
pub fn init_provider(info: ProviderInfo) {
    let mut guard = PROVIDER.write().expect("LLM provider lock poisoned");
    *guard = Some(info);
}

/// Returns a snapshot of the active LLM provider configuration.
pub fn get_provider() -> ProviderInfo {
    PROVIDER
        .read()
        .expect("LLM provider lock poisoned")
        .clone()
        .expect("LLM provider not initialized; call tact_llm::init_provider first")
}

/// Read-only access to the global provider via a closure.
///
/// Avoids cloning fields the caller does not need. The closure runs with the
/// read lock held and may clone (or borrow) any fields it uses.
pub fn read_provider<F, R>(f: F) -> R
where
    F: FnOnce(&ProviderInfo) -> R,
{
    let guard = PROVIDER.read().expect("LLM provider lock poisoned");
    f(guard
        .as_ref()
        .expect("LLM provider not initialized; call tact_llm::init_provider first"))
}

/// Update only the active model id (used by the TUI `/model` command).
pub fn set_model(model: impl Into<String>) -> Result<(), String> {
    let model = model.into();
    if model.trim().is_empty() {
        return Err("model must not be empty".to_string());
    }
    let mut guard = PROVIDER.write().expect("LLM provider lock poisoned");
    let info = guard.as_mut().ok_or_else(|| {
        "LLM provider not initialized; call tact_llm::init_provider first".to_string()
    })?;
    info.model = model;
    Ok(())
}

/// Returns the active LLM client from the installed provider configuration.
pub fn get_llm_client() -> anyhow::Result<LlmProvider> {
    get_provider().build_client()
}

/// Returns `true` if the configured provider is DeepSeek.
///
/// DeepSeek can be configured either as the dedicated `"deepseek"`
/// provider or as an OpenAI-compatible endpoint that targets DeepSeek
/// (e.g. `provider = "openai"` with a `deepseek.com` base URL).
pub fn is_deepseek() -> bool {
    read_provider(|p| {
        p.provider == ProviderKind::DeepSeek
            || p.base_url.contains("deepseek")
            || p.model.contains("deepseek")
    })
}

/// Returns true if the active provider/target is Kimi/Moonshot.
pub fn is_kimi() -> bool {
    read_provider(|p| p.is_kimi())
}

/// Returns true for the Kimi K2.x family.
pub fn is_kimi_k2x() -> bool {
    read_provider(|p| p.is_kimi_k2x())
}

/// Returns true specifically for kimi-k2.7-code.
pub fn is_kimi_k27() -> bool {
    read_provider(|p| p.is_kimi_k27())
}

/// Returns true for the Kimi Code platform (`api.kimi.com/coding`).
pub fn is_kimi_coding() -> bool {
    read_provider(|p| p.is_kimi_coding())
}

/// Returns true when Kimi balance queries are supported for the configured endpoint.
pub fn is_kimi_balance_supported() -> bool {
    read_provider(|p| p.is_kimi_balance_supported())
}

/// Returns true when Kimi Code usage quota queries are supported.
pub fn is_kimi_usage_supported() -> bool {
    read_provider(|p| p.is_kimi_usage_supported())
}

/// Returns true when account balance or usage quota queries are supported.
pub fn is_account_query_supported() -> bool {
    read_provider(|p| p.is_account_query_supported())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::{LlmClient, LlmProvider};
    use crate::mock::MockClient;
    use crate::types::{CreateMessageParams, RequiredMessageParams, StopReason};
    use tact_protocol::{AgentUpdate, TokenUsageInfo};

    fn provider_info(
        provider: ProviderKind,
        api_key: &str,
        base_url: &str,
        model: &str,
    ) -> ProviderInfo {
        ProviderInfo {
            provider,
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn build_client_requires_api_key() {
        let p = provider_info(ProviderKind::DeepSeek, "", "", "deepseek-chat");
        assert!(p.build_client().is_err());
    }

    #[test]
    fn openai_builds_openai_adapter_with_default_base_url() {
        let p = provider_info(ProviderKind::OpenAi, "sk-test", "", "gpt-4o");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAi(adapter) = result.unwrap() else {
            panic!("expected OpenAi adapter for openai");
        };
        assert_eq!(adapter.base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn deepseek_builds_deepseek_adapter_with_default_base_url() {
        let p = provider_info(ProviderKind::DeepSeek, "sk-test", "", "deepseek-chat");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::DeepSeek(adapter) = result.unwrap() else {
            panic!("expected DeepSeek adapter for deepseek");
        };
        assert_eq!(adapter.base_url(), "https://api.deepseek.com");
    }

    #[test]
    fn kimi_builds_kimi_adapter_with_default_base_url() {
        let p = provider_info(ProviderKind::Kimi, "sk-test", "", "kimi-k2.5");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::Kimi(adapter) = result.unwrap() else {
            panic!("expected Kimi adapter for kimi");
        };
        assert_eq!(adapter.base_url(), "https://api.moonshot.cn/v1");
    }

    #[test]
    fn custom_base_url_is_preserved() {
        let p = provider_info(
            ProviderKind::Kimi,
            "sk-test",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        let result = p.build_client().unwrap();
        let LlmProvider::Kimi(adapter) = result else {
            panic!("expected Kimi adapter");
        };
        assert_eq!(adapter.base_url(), "https://api.kimi.com/coding/v1");
    }

    #[test]
    fn is_kimi_detection() {
        assert!(provider_info(ProviderKind::Kimi, "", "", "kimi-k2.5").is_kimi());
        assert!(
            provider_info(ProviderKind::OpenAi, "", "https://api.moonshot.cn/v1", "").is_kimi()
        );
        assert!(
            provider_info(
                ProviderKind::OpenAi,
                "",
                "https://api.kimi.com/coding/v1",
                ""
            )
            .is_kimi()
        );
        assert!(provider_info(ProviderKind::OpenAi, "", "", "kimi-k2.5").is_kimi());
        assert!(!provider_info(ProviderKind::Anthropic, "", "", "claude-sonnet-4").is_kimi());
    }

    #[test]
    fn is_kimi_k2x_and_k27() {
        let k25 = provider_info(ProviderKind::Kimi, "", "", "kimi-k2.5");
        assert!(k25.is_kimi_k2x());
        assert!(!k25.is_kimi_k27());

        let k27 = provider_info(ProviderKind::Kimi, "", "", "kimi-k2.7");
        assert!(k27.is_kimi_k2x());
        assert!(k27.is_kimi_k27());

        let coding = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(coding.is_kimi_k2x());
        assert!(coding.is_kimi_k27());
    }

    #[test]
    fn is_kimi_coding_and_balance_supported() {
        let coding = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(coding.is_kimi_coding());
        assert!(!coding.is_kimi_balance_supported());
        assert!(coding.is_kimi_usage_supported());

        let cn = provider_info(
            ProviderKind::Kimi,
            "",
            "https://api.moonshot.cn/v1",
            "kimi-k2.5",
        );
        assert!(!cn.is_kimi_coding());
        assert!(cn.is_kimi_balance_supported());
        assert!(!cn.is_kimi_usage_supported());

        // kimi-for-coding behind a custom proxy is still Kimi Code:
        // no balance API, usage quota supported.
        let proxy = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://proxy.example.com/v1",
            "kimi-for-coding",
        );
        assert!(proxy.is_kimi_coding());
        assert!(!proxy.is_kimi_balance_supported());
        assert!(proxy.is_kimi_usage_supported());

        assert!(coding.is_account_query_supported());
        assert!(cn.is_account_query_supported());

        let anthropic = provider_info(
            ProviderKind::Anthropic,
            "",
            "https://api.anthropic.com",
            "claude-sonnet-4",
        );
        assert!(!anthropic.is_account_query_supported());
    }

    #[test]
    fn anthropic_build_client_requires_base_url() {
        let p = provider_info(ProviderKind::Anthropic, "sk-test", "", "claude-sonnet-4");
        assert!(p.build_client().is_err());
    }

    #[tokio::test]
    async fn mock_stream_emits_token_usage_when_configured() {
        use crate::ContentBlock;
        use tokio::sync::mpsc::unbounded_channel;

        let usage = TokenUsageInfo {
            prompt: 10,
            completion: 5,
            total: 15,
            prompt_cache_hit_tokens: 0,
            prompt_cache_miss_tokens: 10,
            reasoning_tokens: 1,
        };
        let client = MockClient::with_usage(vec![(
            vec![ContentBlock::Text {
                text: "hi".to_string(),
            }],
            Some(StopReason::EndTurn),
            usage.clone(),
        )]);

        let (tx, mut rx) = unbounded_channel();
        let (blocks, _, returned, _) = client
            .stream_message(
                &CreateMessageParams::new(RequiredMessageParams {
                    model: "mock".to_string(),
                    messages: vec![],
                    max_tokens: 100,
                }),
                Some(tx),
            )
            .await
            .expect("stream");

        assert_eq!(blocks.len(), 1);
        assert_eq!(returned.as_ref().map(|u| u.total), Some(15));

        let update = rx.try_recv().expect("TokenUsage event");
        assert!(matches!(
            update,
            AgentUpdate::TokenUsage(u) if u.total == usage.total
        ));
    }

    #[test]
    fn set_model_updates_and_rejects_empty() {
        let _guard = super::lock_provider_for_tests();
        init_provider(provider_info(
            ProviderKind::Kimi,
            "sk-test",
            "https://api.moonshot.cn/v1",
            "kimi-k2.5",
        ));
        set_model("kimi-for-coding").unwrap();
        assert_eq!(get_provider().model, "kimi-for-coding");

        assert!(set_model("").is_err());
        assert!(set_model("   ").is_err());
        assert_eq!(get_provider().model, "kimi-for-coding");
    }
}
