//! Active provider configuration and client construction.

use std::sync::{Arc, RwLock};

use async_openai::config::Config;
use secrecy::ExposeSecret;

use crate::{
    ApiKeyProvider, CredentialProvider, ProviderProfile, SharedHttpClient, anthropic,
    client::LlmProvider,
    openai,
    types::{OpenAiProtocol, ProviderKind},
};

/// Holds private LLM configuration information.
///
/// This is a **static** snapshot installed at startup (see [`init_provider`]).
/// It is never mutated at runtime: per-request model / reasoning effort are
/// carried by [`crate::CreateMessageParams`], per-agent by `AgentSettings`.
#[derive(Debug, Clone)]
pub struct ProviderInfo {
    pub api_key: String,
    pub base_url: String,
    /// Static configured model id (used by config-level heuristics such as
    /// `is_kimi()` / account queries). The request model travels in
    /// `CreateMessageParams.model` and may differ after `/model` picks.
    pub model: String,
    pub provider: ProviderKind,
    pub protocol: OpenAiProtocol,
    /// Optional OpenAI Responses `context_management.compact_threshold`
    /// (tokens). Only meaningful for `protocol = "responses"`; `None` omits
    /// `context_management` from ordinary `/responses` requests.
    pub responses_compact_threshold: Option<u32>,
}

impl Default for ProviderInfo {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            base_url: String::new(),
            model: String::new(),
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
        }
    }
}

impl ProviderInfo {
    /// Convert this compatibility snapshot to the credential-free profile.
    pub fn to_profile(&self) -> ProviderProfile {
        ProviderProfile {
            provider: self.provider.clone(),
            base_url: self.base_url.clone(),
            model: self.model.clone(),
            protocol: self.protocol,
            responses_compact_threshold: self.responses_compact_threshold,
        }
    }

    /// Returns the conservative capabilities for the configured Responses endpoint.
    pub fn responses_capabilities(&self) -> Option<openai::responses::ResponsesCapabilities> {
        if self.protocol != OpenAiProtocol::Responses {
            return None;
        }
        Some(match self.provider {
            ProviderKind::OpenAi => openai::responses::ResponsesCapabilities::official_openai(),
            ProviderKind::Anthropic => return None,
            ProviderKind::DeepSeek | ProviderKind::Kimi | ProviderKind::Custom(_) => {
                openai::responses::ResponsesCapabilities::custom_provider()
            }
        })
    }

    /// Build an LLM client for this provider configuration.
    pub fn build_client(&self) -> anyhow::Result<LlmProvider> {
        match self.provider {
            ProviderKind::Anthropic => self.build_anthropic(),
            ProviderKind::DeepSeek | ProviderKind::Kimi => match self.protocol {
                OpenAiProtocol::ChatCompletions => self.build_openai_compatible(),
                OpenAiProtocol::Responses => self.build_openai_responses(),
            },
            ProviderKind::OpenAi => match self.protocol {
                OpenAiProtocol::ChatCompletions => self.build_openai_compatible(),
                OpenAiProtocol::Responses => self.build_openai_responses(),
            },
            // Custom providers reuse the OpenAI protocol end to end.
            ProviderKind::Custom(_) => match self.protocol {
                OpenAiProtocol::ChatCompletions => self.build_openai_compatible(),
                OpenAiProtocol::Responses => self.build_openai_responses(),
            },
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

    /// Build a unified Chat Completions client for OpenAI-compatible endpoints.
    ///
    /// DeepSeek and Kimi are wire dialects selected per request, so all three
    /// share one adapter and transport.
    fn build_openai_compatible(&self) -> anyhow::Result<LlmProvider> {
        let config = self.openai_compatible_config()?;
        let profile = self.profile_for_base_url(config.api_base())?;
        let adapter = openai::compatible::OpenAiAdapter::new(config);
        Ok(LlmProvider::ChatCompletions(
            openai::compatible::ChatCompletionsAdapter::new(profile, adapter),
        ))
    }

    fn profile_for_base_url(&self, base_url: &str) -> anyhow::Result<ProviderProfile> {
        Ok(ProviderProfile {
            provider: self.provider.clone(),
            base_url: base_url.to_string(),
            model: self.model.clone(),
            protocol: self.protocol,
            responses_compact_threshold: self.responses_compact_threshold,
        })
    }

    /// Build the official OpenAI Responses API client.
    fn build_openai_responses(&self) -> anyhow::Result<LlmProvider> {
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
        // Hosted web search is a Responses-protocol capability, independent of
        // the endpoint/provider: the adapter injects `Tool::WebSearch` on
        // every ordinary request for OpenAI, DeepSeek, and custom
        // OpenAI-compatible endpoints alike (the `/responses/compact` path
        // never sends tools).
        Ok(LlmProvider::OpenAiResponses(
            openai::responses::OpenAiResponsesAdapter::new(
                self.api_key.clone(),
                base_url,
                self.responses_compact_threshold,
            ),
        ))
    }

    fn openai_compatible_config(&self) -> anyhow::Result<openai::compatible::CompatibleConfig> {
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
        Ok(openai::compatible::CompatibleConfig::new(
            self.api_key.clone(),
            base_url,
        ))
    }

    /// Returns true if the active target is a Kimi/Moonshot endpoint
    /// (config-level: uses the static configured model).
    pub fn is_kimi(&self) -> bool {
        self.is_kimi_with(&self.model)
    }

    /// Kimi endpoint check for an arbitrary model id (per-request wire shape).
    pub(crate) fn is_kimi_with(&self, model: &str) -> bool {
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
        let deepseekish = self.provider == ProviderKind::DeepSeek
            || self.base_url.contains("deepseek")
            || self.model.contains("deepseek");
        if deepseekish {
            return crate::types::is_deepseek_vision_model(&self.model);
        }
        self.provider.supports_vision()
    }
}

/// Credential-free entry point for building an [`LlmProvider`].
///
/// Configuration and authentication are orthogonal: the profile describes the
/// endpoint/wire dialect, while the credential provider may be a static API
/// key today or a browser-OAuth flow later.
#[derive(Debug)]
pub struct Client;

impl Client {
    /// Build a provider client from an explicit profile and credentials.
    ///
    /// Resolves the credential once at construction so misconfiguration fails
    /// fast; adapters still resolve again per request so expiring credentials
    /// can be refreshed without rebuilding the client.
    ///
    /// # Errors
    ///
    /// Returns an error when the credential is empty, the base URL is not
    /// configured, or the configured provider has no default base URL.
    #[allow(clippy::new_ret_no_self)] // deliberate factory: `Client` is a marker namespace
    pub async fn new(
        profile: ProviderProfile,
        credentials: Arc<dyn CredentialProvider>,
    ) -> anyhow::Result<LlmProvider> {
        let credentials_for_responses = credentials.clone();
        let secret = credentials.resolve().await.map_err(anyhow::Error::from)?;
        if secret.expose_secret().is_empty() {
            anyhow::bail!("api_key not configured for provider '{}'", profile.provider);
        }
        let base_url = if profile.base_url.is_empty() {
            profile
                .default_base_url()
                .map(str::to_string)
                .ok_or_else(|| {
                    anyhow::anyhow!("no default base_url for provider '{}'", profile.provider)
                })?
        } else {
            profile.base_url.clone()
        };

        match profile.provider {
            ProviderKind::Anthropic => Ok(LlmProvider::Anthropic(
                anthropic::AnthropicAdapter::new(secret.expose_secret().clone(), base_url),
            )),
            ProviderKind::DeepSeek
            | ProviderKind::Kimi
            | ProviderKind::OpenAi
            | ProviderKind::Custom(_)
                if profile.protocol == OpenAiProtocol::Responses =>
            {
                Ok(LlmProvider::OpenAiResponses(
                    openai::responses::OpenAiResponsesAdapter::new_with_auth(
                        credentials_for_responses,
                        base_url,
                        profile.responses_compact_threshold,
                        SharedHttpClient::default(),
                    ),
                ))
            }
            ProviderKind::DeepSeek
            | ProviderKind::Kimi
            | ProviderKind::OpenAi
            | ProviderKind::Custom(_) => {
                let adapter = openai::compatible::OpenAiAdapter::with_auth(
                    base_url,
                    SharedHttpClient::default(),
                    credentials,
                );
                Ok(LlmProvider::ChatCompletions(
                    openai::compatible::ChatCompletionsAdapter::new(profile, adapter),
                ))
            }
        }
    }
}

/// The active LLM provider configuration — **static in production**.
///
/// `/model` picks no longer mutate this: per-agent model lives in
/// `AgentSettings.model`, per-request in `CreateMessageParams.model`.
/// `RwLock<Option<…>>` is retained (instead of `OnceLock`) so `test-support`
/// overrides and unit tests can reinstall; production `install` runs once.
static PROVIDER: RwLock<Option<ProviderInfo>> = RwLock::new(None);

/// The process-global credential provider installed alongside
/// [`PROVIDER`]. Kept separate so a future browser-OAuth flow can be injected
/// without changing the provider snapshot.
static CREDENTIALS: RwLock<Option<Arc<dyn CredentialProvider>>> = RwLock::new(None);

/// Serialize tests that mutate/read the process-global provider snapshot.
#[cfg(test)]
pub(crate) fn lock_provider_for_tests() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().expect("provider test lock poisoned")
}

/// Install the active LLM provider configuration with its static API key.
///
/// Safe to call again under `test-support` overrides; production `install` still
/// runs once per process.
pub fn init_provider(info: ProviderInfo) {
    let credentials = Arc::new(ApiKeyProvider::new(info.api_key.clone()));
    init_provider_with_credentials(info, credentials);
}

/// Install the active LLM provider configuration with an explicit credential
/// provider (e.g. a future browser-OAuth flow).
///
/// Safe to call again under `test-support` overrides; production `install` still
/// runs once per process.
pub fn init_provider_with_credentials(
    info: ProviderInfo,
    credentials: Arc<dyn CredentialProvider>,
) {
    let mut guard = PROVIDER.write().expect("LLM provider lock poisoned");
    *guard = Some(info);
    drop(guard);
    let mut guard = CREDENTIALS.write().expect("LLM credential lock poisoned");
    *guard = Some(credentials);
}

pub(crate) fn active_credential_provider() -> Arc<dyn CredentialProvider> {
    try_active_credential_provider()
        .expect("LLM credentials not initialized; call tact_llm::init_provider first")
}

pub(crate) fn try_active_credential_provider() -> Option<Arc<dyn CredentialProvider>> {
    CREDENTIALS
        .read()
        .expect("LLM credential lock poisoned")
        .clone()
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

/// Returns the active LLM client from the installed provider configuration.
///
/// The credential is resolved once here so misconfiguration fails fast;
/// adapters still resolve again per request so expiring credentials can be
/// refreshed without rebuilding the client.
pub async fn get_llm_client() -> anyhow::Result<LlmProvider> {
    let profile = get_provider().to_profile();
    Client::new(profile, active_credential_provider()).await
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

/// Returns true for the Kimi K2.x family (config-level: static model).
pub fn is_kimi_k2x() -> bool {
    read_provider(|p| p.is_kimi_k2x(&p.model))
}

/// Returns true specifically for kimi-k2.7-code (config-level: static model).
pub fn is_kimi_k27() -> bool {
    read_provider(|p| p.is_kimi_k27(&p.model))
}

/// Returns true for the Kimi Code platform (`api.kimi.com/coding`).
pub fn is_kimi_coding() -> bool {
    read_provider(|p| p.is_kimi_coding(&p.model))
}

/// Returns true when DeepSeek balance queries are supported for the configured endpoint.
pub fn is_deepseek_balance_supported() -> bool {
    read_provider(|p| p.is_deepseek_balance_supported())
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

/// Returns false if the current model is known to be text-only (e.g. DeepSeek V4).
///
/// Use this to gate image attachments before they reach the LLM layer.
pub fn supports_vision() -> bool {
    read_provider(|p| p.supports_vision())
}

/// Whether the given model id uses reasoning-effort semantics (vs thinking
/// budget) for the configured provider, following the same heuristics as the
/// wire body hooks so the `/model` second step always matches what is sent.
///
/// - openai (standard) → effort
/// - deepseek (incl. openai + deepseek base_url/model) → effort
/// - kimi k3 / k3-256k → effort; kimi coding 系 → budget (Thinking:ON)
/// - anthropic → budget (native)
pub fn model_uses_effort(model: &str, info: &ProviderInfo) -> bool {
    match &info.provider {
        ProviderKind::DeepSeek => true,
        ProviderKind::Anthropic => false,
        ProviderKind::Kimi => is_kimi_k3(model),
        ProviderKind::OpenAi | ProviderKind::Custom(_) => {
            if info.is_kimi_with(model) {
                is_kimi_k3(model)
            } else {
                // standard OpenAI or deepseek-compatible endpoint → effort
                true
            }
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
    use std::sync::atomic::{AtomicUsize, Ordering};

    use futures_util::future::BoxFuture;
    use secrecy::SecretString;
    use tact_protocol::{AgentUpdate, TokenUsageInfo};

    use super::*;
    use crate::{
        ApiKeyProvider, LlmError,
        client::{LlmClient, LlmProvider},
        mock::MockClient,
        types::{CreateMessageParams, RequiredMessageParams, StopReason},
    };

    #[derive(Debug, Clone)]
    struct CountingCredential {
        key: Arc<SecretString>,
        calls: Arc<AtomicUsize>,
    }

    impl CountingCredential {
        fn new(key: &str) -> Self {
            Self {
                key: Arc::new(SecretString::from(key.to_string())),
                calls: Arc::new(AtomicUsize::new(0)),
            }
        }

        fn calls(&self) -> usize {
            self.calls.load(Ordering::Relaxed)
        }
    }

    impl CredentialProvider for CountingCredential {
        fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            let key = self.key.clone();
            Box::pin(async move { Ok((*key).clone()) })
        }
    }

    #[derive(Debug)]
    struct FailingCredential;

    impl CredentialProvider for FailingCredential {
        fn resolve(&self) -> BoxFuture<'_, Result<SecretString, LlmError>> {
            Box::pin(async { Err(LlmError::Auth("test credential unavailable".to_string())) })
        }
    }

    fn provider_info(
        provider: ProviderKind,
        api_key: &str,
        base_url: &str,
        model: &str,
    ) -> ProviderInfo {
        ProviderInfo {
            provider,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
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
    fn supports_vision_accepts_deepseek_vision_models_only() {
        let vision = provider_info(
            ProviderKind::DeepSeek,
            "",
            "",
            "deepseek-v4-flash-vision-exp",
        );
        assert!(vision.supports_vision(), "vision-exp must be image-capable");

        let text = provider_info(ProviderKind::DeepSeek, "", "", "deepseek-v4-flash");
        assert!(!text.supports_vision(), "text-only v4 must reject images");

        let via_openai = provider_info(
            ProviderKind::Custom("proxy".into()),
            "",
            "https://proxy.deepseek.example/v1",
            "deepseek-v4-flash-vision-exp",
        );
        assert!(
            via_openai.supports_vision(),
            "deepseek-ish base_url + vision id is image-capable"
        );
    }

    #[test]
    fn openai_builds_chat_completions_adapter_with_default_base_url() {
        let p = provider_info(ProviderKind::OpenAi, "sk-test", "", "gpt-4o");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::ChatCompletions(adapter) = result.unwrap() else {
            panic!("expected Chat Completions adapter for openai");
        };
        assert_eq!(adapter.base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn openai_responses_exposes_core_capabilities() {
        let info = ProviderInfo {
            api_key: "key".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-5".into(),
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::Responses,
            responses_compact_threshold: None,
        };
        let capabilities = info.responses_capabilities().unwrap();
        assert!(capabilities.responses);
        assert!(capabilities.streaming);
        assert!(capabilities.compact);
        // Hosted web search is a Responses-protocol capability — present for
        // official OpenAI, DeepSeek, and custom endpoints alike.
        assert_eq!(
            capabilities.hosted_tools,
            std::collections::BTreeSet::from([
                super::super::openai::responses::ResponsesToolKind::WebSearch
            ])
        );
    }

    #[test]
    fn openai_responses_protocol_builds_responses_adapter() {
        let mut p = provider_info(ProviderKind::OpenAi, "sk-test", "", "gpt-5");
        p.protocol = OpenAiProtocol::Responses;
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAiResponses(adapter) = result.unwrap() else {
            panic!("expected OpenAI Responses adapter");
        };
        assert_eq!(adapter.base_url(), "https://api.openai.com/v1");
    }

    #[test]
    fn deepseek_builds_chat_completions_adapter_with_default_base_url() {
        let p = provider_info(ProviderKind::DeepSeek, "sk-test", "", "deepseek-chat");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::ChatCompletions(adapter) = result.unwrap() else {
            panic!("expected Chat Completions adapter for deepseek");
        };
        assert_eq!(adapter.base_url(), "https://api.deepseek.com");
    }

    #[test]
    fn deepseek_responses_protocol_builds_responses_adapter() {
        let mut p = provider_info(ProviderKind::DeepSeek, "sk-test", "", "deepseek-v4-flash");
        p.protocol = OpenAiProtocol::Responses;
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAiResponses(adapter) = result.unwrap() else {
            panic!("expected OpenAI Responses adapter for deepseek responses");
        };
        assert_eq!(adapter.base_url(), "https://api.deepseek.com");
    }

    #[test]
    fn custom_responses_protocol_builds_responses_adapter() {
        // Hosted web search is a Responses-protocol capability, independent
        // of the endpoint: custom OpenAI-compatible gateways get it too.
        // (Request-level injection is covered by
        // `convert::tests::native_web_search_*`.)
        let mut p = provider_info(
            ProviderKind::Custom("my-gateway".into()),
            "sk-test",
            "https://gateway.example.com/v1",
            "gpt-5",
        );
        p.protocol = OpenAiProtocol::Responses;
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAiResponses(adapter) = result.unwrap() else {
            panic!("expected OpenAI Responses adapter for custom responses");
        };
        assert_eq!(adapter.base_url(), "https://gateway.example.com/v1");
    }

    #[test]
    fn kimi_builds_chat_completions_adapter_with_default_base_url() {
        let p = provider_info(ProviderKind::Kimi, "sk-test", "", "kimi-k2.5");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::ChatCompletions(adapter) = result.unwrap() else {
            panic!("expected Chat Completions adapter for kimi");
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
        let LlmProvider::ChatCompletions(adapter) = result else {
            panic!("expected Chat Completions adapter");
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
        assert!(k25.is_kimi_k2x("kimi-k2.5"));
        assert!(!k25.is_kimi_k27("kimi-k2.5"));

        let k27 = provider_info(ProviderKind::Kimi, "", "", "kimi-k2.7");
        assert!(k27.is_kimi_k2x("kimi-k2.7"));
        assert!(k27.is_kimi_k27("kimi-k2.7"));

        let coding = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(coding.is_kimi_k2x("kimi-for-coding"));
        assert!(coding.is_kimi_k27("kimi-for-coding"));
    }

    #[test]
    fn is_kimi_coding_and_balance_supported() {
        let coding = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(coding.is_kimi_coding("kimi-for-coding"));
        assert!(!coding.is_kimi_balance_supported());
        assert!(coding.is_kimi_usage_supported());

        let cn = provider_info(
            ProviderKind::Kimi,
            "",
            "https://api.moonshot.cn/v1",
            "kimi-k2.5",
        );
        assert!(!cn.is_kimi_coding("kimi-k2.5"));
        assert!(cn.is_kimi_balance_supported());
        assert!(!cn.is_kimi_usage_supported());

        let proxy_balance = provider_info(
            ProviderKind::Kimi,
            "",
            "https://proxy.example.com/v1",
            "kimi-k2.5",
        );
        assert!(!proxy_balance.is_kimi_balance_supported());
        assert!(!proxy_balance.is_account_query_supported());

        // kimi-for-coding behind a custom proxy is still Kimi Code for wire
        // shape, but has no usage quota API: no account queries at all.
        let proxy = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://proxy.example.com/v1",
            "kimi-for-coding",
        );
        assert!(proxy.is_kimi_coding("kimi-for-coding"));
        assert!(!proxy.is_kimi_balance_supported());
        assert!(!proxy.is_kimi_usage_supported());
        assert!(!proxy.is_account_query_supported());

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
    fn is_deepseek_balance_supported_only_for_official_endpoint() {
        let official = provider_info(
            ProviderKind::DeepSeek,
            "",
            "https://api.deepseek.com",
            "deepseek-chat",
        );
        assert!(official.is_deepseek_balance_supported());
        assert!(official.is_account_query_supported());

        // `/v1` suffix resolves to the same official host.
        let official_v1 = provider_info(
            ProviderKind::DeepSeek,
            "",
            "https://api.deepseek.com/v1",
            "deepseek-chat",
        );
        assert!(official_v1.is_deepseek_balance_supported());

        // Empty base URL defaults to the official endpoint (config resolution).
        let empty_base = provider_info(ProviderKind::DeepSeek, "", "", "deepseek-chat");
        assert!(empty_base.is_deepseek_balance_supported());

        // A DeepSeek model served through a custom proxy has no balance API.
        let proxy = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://proxy.example.com/v1",
            "deepseek-chat",
        );
        assert!(!proxy.is_deepseek_balance_supported());
        assert!(!proxy.is_account_query_supported());

        // Host-name spoofing and plain HTTP are rejected.
        let spoofed = provider_info(
            ProviderKind::DeepSeek,
            "",
            "https://api.deepseek.com.evil.example/v1",
            "deepseek-chat",
        );
        assert!(!spoofed.is_deepseek_balance_supported());

        let http = provider_info(
            ProviderKind::DeepSeek,
            "",
            "http://api.deepseek.com/v1",
            "deepseek-chat",
        );
        assert!(!http.is_deepseek_balance_supported());

        let anthropic = provider_info(
            ProviderKind::Anthropic,
            "",
            "https://api.anthropic.com",
            "claude-sonnet-4",
        );
        assert!(!anthropic.is_deepseek_balance_supported());
    }

    #[test]
    fn is_kimi_usage_supported_only_for_official_endpoint() {
        let official = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(official.is_kimi_usage_supported());
        assert!(official.is_account_query_supported());

        // Custom proxy serving kimi-for-coding: wire shape is still coding,
        // but there is no usage quota API behind a proxy.
        let proxy = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://proxy.example.com/v1",
            "kimi-for-coding",
        );
        assert!(proxy.is_kimi_coding("kimi-for-coding"));
        assert!(!proxy.is_kimi_usage_supported());
        assert!(!proxy.is_account_query_supported());

        // Official host without the /coding path is not the Kimi Code API.
        let bare_host = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com",
            "kimi-for-coding",
        );
        assert!(!bare_host.is_kimi_usage_supported());

        // Host-name spoofing and plain HTTP are rejected.
        let spoofed = provider_info(
            ProviderKind::OpenAi,
            "",
            "https://api.kimi.com.evil.example/coding/v1",
            "kimi-for-coding",
        );
        assert!(!spoofed.is_kimi_usage_supported());

        let http = provider_info(
            ProviderKind::OpenAi,
            "",
            "http://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(!http.is_kimi_usage_supported());

        // Moonshot (non-coding) endpoint: no usage quota API.
        let cn = provider_info(
            ProviderKind::Kimi,
            "",
            "https://api.moonshot.cn/v1",
            "kimi-k2.5",
        );
        assert!(!cn.is_kimi_usage_supported());
    }

    #[test]
    fn anthropic_build_client_requires_base_url() {
        let p = provider_info(ProviderKind::Anthropic, "sk-test", "", "claude-sonnet-4");
        assert!(p.build_client().is_err());
    }

    #[tokio::test]
    async fn mock_stream_emits_token_usage_when_configured() {
        use tokio::sync::mpsc::unbounded_channel;

        use crate::ContentBlock;

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
        let response = client
            .stream_message(
                &CreateMessageParams::new(RequiredMessageParams {
                    model: "mock".to_string(),
                    messages: vec![],
                    max_tokens: 100,
                }),
                None,
                Some(tx),
            )
            .await
            .expect("stream");
        let blocks = response.blocks;
        let returned = response.usage;

        assert_eq!(blocks.len(), 1);
        assert_eq!(returned.as_ref().map(|u| u.total), Some(15));

        let update = rx.try_recv().expect("TokenUsage event");
        assert!(matches!(
            update,
            AgentUpdate::TokenUsage(u) if u.total == usage.total
        ));
    }

    #[test]
    fn provider_is_immutable_after_install() {
        let _guard = super::lock_provider_for_tests();
        init_provider(provider_info(
            ProviderKind::Kimi,
            "sk-test",
            "https://api.moonshot.cn/v1",
            "kimi-k2.5",
        ));
        // The global provider is a static snapshot; per-agent model changes
        // live in AgentSettings / CreateMessageParams, never here.
        assert_eq!(get_provider().model, "kimi-k2.5");
    }

    #[tokio::test]
    async fn client_new_builds_unified_chat_completions_adapter() {
        let profile = ProviderProfile {
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5".to_string(),
        };
        let provider = Client::new(profile, Arc::new(ApiKeyProvider::new("sk-test")))
            .await
            .expect("client construction must succeed");
        let LlmProvider::ChatCompletions(adapter) = provider else {
            panic!("expected unified Chat Completions adapter");
        };
        assert_eq!(adapter.base_url(), "https://api.openai.com/v1");
    }

    #[tokio::test]
    async fn client_new_rejects_empty_api_key() {
        let profile = ProviderProfile {
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5".to_string(),
        };
        let error = match Client::new(profile, Arc::new(ApiKeyProvider::new(""))).await {
            Err(error) => error,
            Ok(_) => panic!("empty api key must fail fast"),
        };
        assert!(error.to_string().contains("api_key not configured"));
    }

    #[tokio::test]
    async fn client_new_preserves_credential_error_source() {
        let profile = ProviderProfile {
            provider: ProviderKind::OpenAi,
            protocol: OpenAiProtocol::default(),
            responses_compact_threshold: None,
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-5".to_string(),
        };
        let error = match Client::new(profile, Arc::new(FailingCredential)).await {
            Ok(_) => panic!("client construction must fail"),
            Err(error) => error,
        };

        assert!(error.to_string().contains("test credential unavailable"));
        assert!(
            error.downcast_ref::<LlmError>().is_some(),
            "credential error must be preserved in the chain"
        );
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // test mutex serializes global provider only
    async fn get_llm_client_uses_installed_credential_provider() {
        let _guard = lock_provider_for_tests();
        let credentials = CountingCredential::new("sk-oauth");
        init_provider_with_credentials(
            provider_info(
                ProviderKind::OpenAi,
                "",
                "https://api.openai.com/v1",
                "gpt-5",
            ),
            Arc::new(credentials.clone()),
        );

        let client = get_llm_client()
            .await
            .expect("client construction must succeed");
        assert!(matches!(client, LlmProvider::ChatCompletions(_)));
        assert_eq!(credentials.calls(), 1);
    }
}
