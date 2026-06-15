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
use std::sync::LazyLock;
use std::{fmt, time::Duration};
use tokio::sync::mpsc::UnboundedSender;

use tact_core::AgentUpdate;

/// Holds private LLM configuration information.
#[derive(Default)]
pub struct ProviderInfo {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub provider: String,
}

impl ProviderInfo {
    pub fn get_provider_from_env() -> anyhow::Result<Self> {
        let provider = std::env::var("TACT_PROVIDER")?;
        if provider.is_empty() {
            return Err(anyhow::anyhow!(
                "TACT_PROVIDER environment variable not set"
            ));
        }
        let (api_key, base_url, model) = if provider != "openai" && provider != "anthropic" {
            return Err(anyhow::anyhow!(
                "TACT_PROVIDER must be either 'openai' or 'anthropic'"
            ));
        } else if provider == "openai" {
            (
                std::env::var("OPENAI_API_KEY")?,
                std::env::var("OPENAI_BASE_URL").unwrap_or_default(),
                std::env::var("OPENAI_MODEL").unwrap_or_default(),
            )
        } else {
            (
                std::env::var("ANTHROPIC_API_KEY")?,
                std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default(),
                std::env::var("ANTHROPIC_MODEL").unwrap_or_default(),
            )
        };
        Ok(Self {
            api_key,
            base_url,
            model,
            provider,
        })
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

/// Abstract interface for streaming and non-streaming LLM calls.
#[async_trait::async_trait]
pub trait LlmClient: Send + Sync {
    /// Stream a message request, emitting real-time updates via `ui_tx`.
    ///
    /// Returns the final content blocks and stop reason.
    async fn stream_message(
        &self,
        request: &CreateMessageParams,
        ui_tx: Option<UnboundedSender<AgentUpdate>>,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError>;

    /// Non-streaming message request (used for context compaction).
    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError>;
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
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        match self {
            LlmProvider::Anthropic(a) => a.stream_message(request, ui_tx).await,
            LlmProvider::OpenAi(o) => o.stream_message(request, ui_tx).await,
        }
    }

    async fn create_message(
        &self,
        request: &CreateMessageParams,
    ) -> Result<(Vec<ContentBlock>, Option<StopReason>), LlmError> {
        match self {
            LlmProvider::Anthropic(a) => a.create_message(request).await,
            LlmProvider::OpenAi(o) => o.create_message(request).await,
        }
    }
}

/// Returns the active LLM client based on environment variables.
///
/// Environment variables:
/// - `TACT_PROVIDER` = `anthropic` | `openai` (optional; inferred from API keys if absent)
/// - `ANTHROPIC_API_KEY`, `ANTHROPIC_BASE_URL`, `ANTHROPIC_MODEL`
/// - `OPENAI_API_KEY`, `OPENAI_BASE_URL` (optional), `OPENAI_MODEL`
pub fn get_llm_client() -> anyhow::Result<LlmProvider> {
    dotenvy::dotenv().ok();

    let provider = std::env::var("TACT_PROVIDER")
        .ok()
        .or_else(|| {
            if std::env::var("ANTHROPIC_API_KEY").is_ok() {
                Some("anthropic".to_string())
            } else if std::env::var("OPENAI_API_KEY").is_ok() {
                Some("openai".to_string())
            } else {
                None
            }
        })
        .context("No LLM provider configured. Set TACT_PROVIDER=anthropic|openai or provide ANTHROPIC_API_KEY / OPENAI_API_KEY")?;

    match provider.as_str() {
        "anthropic" => {
            let key = std::env::var("ANTHROPIC_API_KEY").context("ANTHROPIC_API_KEY is not set")?;
            let base_url =
                std::env::var("ANTHROPIC_BASE_URL").context("ANTHROPIC_BASE_URL is not set")?;
            let client = anthropic_ai_sdk::client::AnthropicClientBuilder::new(key, "")
                .with_api_base_url(base_url)
                .build::<MessageError>()
                .context("can't create Anthropic client")?;
            Ok(LlmProvider::Anthropic(anthropic::AnthropicAdapter::new(
                client,
            )))
        }
        "openai" => {
            let api_key = std::env::var("OPENAI_API_KEY").context("OPENAI_API_KEY is not set")?;
            let base_url = std::env::var("OPENAI_BASE_URL")
                .unwrap_or_else(|_| "https://api.openai.com/v1".to_string());
            let config = openai::CompatibleConfig::new(api_key, base_url);
            Ok(LlmProvider::OpenAi(openai::OpenAiAdapter::new(config)))
        }
        other => anyhow::bail!("Unknown TACT_PROVIDER: {other}. Use 'anthropic' or 'openai'."),
    }
}

/// Returns `true` if the configured provider is DeepSeek.
///
/// Detects DeepSeek by checking if `OPENAI_BASE_URL` or `OPENAI_MODEL`
/// contains "deepseek".
pub fn is_deepseek() -> bool {
    let provider = std::env::var("TACT_PROVIDER").unwrap_or_default();
    if provider == "anthropic" {
        let base_url = std::env::var("ANTHROPIC_BASE_URL").unwrap_or_default();
        let model = std::env::var("ANTHROPIC_MODEL").unwrap_or_default();
        base_url.contains("deepseek") || model.contains("deepseek")
    } else {
        let base_url = std::env::var("OPENAI_BASE_URL").unwrap_or_default();
        let model = std::env::var("OPENAI_MODEL").unwrap_or_default();
        base_url.contains("deepseek") || model.contains("deepseek")
    }
}

/// Query DeepSeek account balance.
///
/// Calls `GET https://api.deepseek.com/user/balance` with the provided API key.
/// Returns `BalanceInfo` on success.
pub async fn query_deepseek_balance() -> anyhow::Result<tact_core::BalanceInfo> {
    let provider = ProviderInfo::get_provider_from_env()?;
    let api_key = provider.api_key;
    let base_url = provider.base_url;

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

    Ok(tact_core::BalanceInfo {
        is_available: raw.is_available,
        balance_infos: raw
            .balance_infos
            .into_iter()
            .map(|e| tact_core::BalanceEntry {
                currency: e.currency,
                total_balance: e.total_balance,
                granted_balance: e.granted_balance,
                topped_up_balance: e.topped_up_balance,
            })
            .collect(),
    })
}

/// Returns the provider information from the active provider's environment variables.
/// Parsed once on first call and cached for the lifetime of the process.
pub fn get_provider() -> &'static ProviderInfo {
    static PROVIDER: LazyLock<ProviderInfo> =
        LazyLock::new(|| ProviderInfo::get_provider_from_env().unwrap_or_default());
    &PROVIDER
}
