//! LLM provider abstraction.
//!
//! Supports Anthropic (Messages API) and OpenAI-compatible providers
//! (Chat Completions API) via the `async-openai` crate.

pub mod anthropic;
pub mod convert;
pub mod openai;

#[cfg(test)]
mod test_openai;

use anthropic_ai_sdk::types::message::{
    ContentBlock, CreateMessageParams, MessageError, StopReason,
};
use anyhow::Context;
use std::sync::OnceLock;
use std::{fmt, time::Duration};
use tokio::sync::mpsc::UnboundedSender;

use tact_protocol::AgentUpdate;
use tact_protocol::TokenUsageInfo;

/// Holds private LLM configuration information.
#[derive(Debug, Default)]
pub struct ProviderInfo {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub provider: String,
}

impl ProviderInfo {
    /// Build an LLM client for this provider configuration.
    pub fn build_client(&self) -> anyhow::Result<LlmProvider> {
        match self.provider.as_str() {
            "anthropic" => {
                if self.api_key.is_empty() {
                    anyhow::bail!("api_key not configured for provider 'anthropic'");
                }
                if self.base_url.is_empty() {
                    anyhow::bail!("base_url not configured for provider 'anthropic'");
                }
                Ok(LlmProvider::Anthropic(anthropic::AnthropicAdapter::new(
                    self.api_key.clone(),
                    self.base_url.clone(),
                )))
            }
            "openai" => {
                if self.api_key.is_empty() {
                    anyhow::bail!("api_key not configured for provider 'openai'");
                }
                let base_url = if self.base_url.is_empty() {
                    "https://api.openai.com/v1".to_string()
                } else {
                    self.base_url.clone()
                };
                let config = openai::CompatibleConfig::new(self.api_key.clone(), base_url);
                Ok(LlmProvider::OpenAi(openai::OpenAiAdapter::new(config)))
            }
            other => {
                anyhow::bail!("Unknown provider: {other}. Use 'anthropic' or 'openai'.")
            }
        }
    }

    /// Returns true if the active target is a Kimi/Moonshot endpoint.
    pub fn is_kimi(&self) -> bool {
        self.base_url.contains("moonshot")
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
}

/// Unified error type for LLM operations.
#[derive(Debug)]
pub enum LlmError {
    Anthropic(MessageError),
    OpenAi(async_openai::error::OpenAIError),
    Other(String),
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LlmError::Anthropic(e) => write!(f, "Anthropic error: {e}"),
            LlmError::OpenAi(e) => write!(f, "OpenAI error: {e}"),
            LlmError::Other(s) => write!(f, "LLM error: {s}"),
        }
    }
}

impl std::error::Error for LlmError {}

impl From<MessageError> for LlmError {
    fn from(e: MessageError) -> Self {
        LlmError::Anthropic(e)
    }
}

impl From<async_openai::error::OpenAIError> for LlmError {
    fn from(e: async_openai::error::OpenAIError) -> Self {
        LlmError::OpenAi(e)
    }
}

/// Serialized JSON request body actually sent to the LLM API (for session debugging).
pub type LlmRequestBody = Vec<u8>;

/// Abstract interface for streaming and non-streaming LLM calls.
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    /// Stream a message request, emitting real-time updates via `ui_tx`.
    ///
    /// Returns content blocks, stop reason, token usage, and the serialized request body.
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
    >;

    /// Non-streaming message request (used for context compaction).
    ///
    /// Returns content blocks, stop reason, token usage, and the serialized request body.
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
    >;
}

/// Supported LLM providers.
#[derive(Clone)]
pub enum LlmProvider {
    Anthropic(anthropic::AnthropicAdapter),
    OpenAi(openai::OpenAiAdapter),
}

#[async_trait::async_trait]
impl LlmClient for LlmProvider {
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
        match self {
            LlmProvider::Anthropic(a) => a.stream_message(request, ui_tx).await,
            LlmProvider::OpenAi(o) => o.stream_message(request, ui_tx).await,
        }
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
        match self {
            LlmProvider::Anthropic(a) => a.create_message(request).await,
            LlmProvider::OpenAi(o) => o.create_message(request).await,
        }
    }
}

impl LlmProvider {
    /// Set a `user_id` on the underlying client adapter.
    ///
    /// For OpenAI-compatible adapters (DeepSeek, Kimi, etc.) this is
    /// injected into the request body as `"user_id"`.  For Anthropic
    /// adapters it is injected as `metadata.user_id`.  Both mechanisms
    /// enable KV cache isolation per session on DeepSeek's endpoints.
    pub fn set_user_id(&mut self, user_id: &str) {
        match self {
            LlmProvider::OpenAi(o) => o.set_user_id(user_id.to_string()),
            LlmProvider::Anthropic(a) => a.set_user_id(user_id.to_string()),
        }
    }
}

/// The active LLM provider configuration.
static PROVIDER: OnceLock<ProviderInfo> = OnceLock::new();

/// Install the active LLM provider configuration. Must be called once at startup.
pub fn init_provider(info: ProviderInfo) {
    PROVIDER
        .set(info)
        .expect("LLM provider must be initialized exactly once");
}

/// Returns the active LLM client from the installed provider configuration.
pub fn get_llm_client() -> anyhow::Result<LlmProvider> {
    get_provider().build_client()
}

/// Returns `true` if the configured provider is DeepSeek.
pub fn is_deepseek() -> bool {
    let provider = get_provider();
    if provider.provider == "anthropic" {
        provider.base_url.contains("deepseek") || provider.model.contains("deepseek")
    } else {
        provider.base_url.contains("deepseek") || provider.model.contains("deepseek")
    }
}

/// Query DeepSeek account balance.
///
/// Calls `GET https://api.deepseek.com/user/balance` with the provided API key.
/// Returns `BalanceInfo` on success.
pub async fn query_deepseek_balance() -> anyhow::Result<tact_protocol::BalanceInfo> {
    let provider = get_provider();
    let api_key = provider.api_key.clone();
    let base_url = provider.base_url.clone();

    // Construct the balance endpoint URL from the base URL
    let balance_url = if base_url.contains("api.deepseek.com") {
        "https://api.deepseek.com/user/balance".to_string()
    } else {
        // Extract origin from base_url and append /user/balance
        let origin = base_url
            .trim_end_matches('/')
            .trim_end_matches("/v1")
            .trim_end_matches("/v1/");
        format!("{origin}/user/balance")
    };

    let client = reqwest::Client::new();
    let resp = client
        .get(&balance_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(Duration::from_millis(5000))
        .send()
        .await
        .with_context(|| format!("Failed to query DeepSeek balance at {balance_url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "DeepSeek balance query returned HTTP {}: {}",
            status,
            error_body.trim()
        );
    }

    let body = resp
        .text()
        .await
        .context("Failed to read DeepSeek balance response")?;

    #[derive(serde::Deserialize)]
    struct RawBalanceEntry {
        currency: String,
        total_balance: String,
        granted_balance: String,
        topped_up_balance: String,
    }

    #[derive(serde::Deserialize)]
    struct RawBalanceResponse {
        is_available: bool,
        balance_infos: Vec<RawBalanceEntry>,
    }

    let raw: RawBalanceResponse =
        serde_json::from_str(&body).context("Failed to parse DeepSeek balance response")?;

    Ok(tact_protocol::BalanceInfo {
        is_available: raw.is_available,
        balance_infos: raw
            .balance_infos
            .into_iter()
            .map(|e| tact_protocol::BalanceEntry {
                currency: e.currency,
                total_balance: e.total_balance,
                granted_balance: e.granted_balance,
                topped_up_balance: e.topped_up_balance,
            })
            .collect(),
    })
}

/// Returns the provider information installed at startup.
pub fn get_provider() -> &'static ProviderInfo {
    PROVIDER
        .get()
        .expect("LLM provider not initialized; call tact_llm::init_provider first")
}

/// Returns true if the active provider/target is Kimi/Moonshot.
pub fn is_kimi() -> bool {
    get_provider().is_kimi()
}

/// Returns true for the Kimi K2.x family.
pub fn is_kimi_k2x() -> bool {
    get_provider().is_kimi_k2x()
}

/// Returns true specifically for kimi-k2.7-code.
pub fn is_kimi_k27() -> bool {
    get_provider().is_kimi_k27()
}
