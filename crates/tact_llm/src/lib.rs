//! LLM provider abstraction.
//!
//! Supports Anthropic (Messages API), OpenAI-compatible providers
//! (Chat Completions API) via the `async-openai` crate, DeepSeek
//! (which uses the OpenAI-compatible API), and Kimi/Moonshot
//! (also OpenAI-compatible).

pub mod anthropic;
pub mod convert;
pub mod openai;

#[cfg(test)]
mod test_openai;

use anthropic_ai_sdk::types::message::{
    ContentBlock, CreateMessageParams, MessageError, StopReason,
};
use anyhow::Context;
use std::sync::Arc;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
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
            "deepseek" => {
                if self.api_key.is_empty() {
                    anyhow::bail!("api_key not configured for provider 'deepseek'");
                }
                let base_url = if self.base_url.is_empty() {
                    "https://api.deepseek.com".to_string()
                } else {
                    self.base_url.clone()
                };
                let config = openai::CompatibleConfig::new(self.api_key.clone(), base_url);
                Ok(LlmProvider::OpenAi(openai::OpenAiAdapter::new(config)))
            }
            "kimi" => {
                if self.api_key.is_empty() {
                    anyhow::bail!("api_key not configured for provider 'kimi'");
                }
                let base_url = if self.base_url.is_empty() {
                    "https://api.moonshot.cn/v1".to_string()
                } else {
                    self.base_url.clone()
                };
                let config = openai::CompatibleConfig::new(self.api_key.clone(), base_url);
                Ok(LlmProvider::OpenAi(openai::OpenAiAdapter::new(config)))
            }
            other => {
                anyhow::bail!(
                    "Unknown provider: {other}. Use 'anthropic', 'openai', 'deepseek', or 'kimi'."
                )
            }
        }
    }

    /// Returns true if the active target is a Kimi/Moonshot endpoint.
    pub fn is_kimi(&self) -> bool {
        self.provider == "kimi"
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
        self.provider == "deepseek"
            || self.base_url.contains("deepseek")
            || self.model.contains("deepseek")
            || self.is_kimi_balance_supported()
            || self.is_kimi_usage_supported()
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
    /// Mock provider for integration tests. Returns predetermined responses.
    Mock(MockClient),
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
            LlmProvider::Mock(m) => m.stream_message(request, ui_tx).await,
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
            LlmProvider::Mock(m) => m.create_message(request).await,
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
            LlmProvider::Mock(_) => {} // no-op for mock
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
///
/// DeepSeek can be configured either as the dedicated `"deepseek"`
/// provider or as an OpenAI-compatible endpoint that targets DeepSeek
/// (e.g. `provider = "openai"` with a `deepseek.com` base URL).
pub fn is_deepseek() -> bool {
    let provider = get_provider();
    provider.provider == "deepseek"
        || provider.base_url.contains("deepseek")
        || provider.model.contains("deepseek")
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

    fn parse_amount(field: &str, value: &str) -> anyhow::Result<f64> {
        value
            .trim()
            .parse::<f64>()
            .with_context(|| format!("DeepSeek balance field {field} is not numeric: {value:?}"))
    }

    Ok(tact_protocol::BalanceInfo {
        is_available: raw.is_available,
        balance_infos: raw
            .balance_infos
            .into_iter()
            .map(|e| {
                Ok(tact_protocol::BalanceEntry {
                    currency: e.currency,
                    total_balance: parse_amount("total_balance", &e.total_balance)?,
                    granted_balance: parse_amount("granted_balance", &e.granted_balance)?,
                    topped_up_balance: parse_amount("topped_up_balance", &e.topped_up_balance)?,
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
    })
}

/// Derive Kimi balance API URL from the configured OpenAI-compatible base URL.
///
/// Returns `None` for Kimi Code (`api.kimi.com/coding`), which has no balance REST endpoint.
fn kimi_balance_url_from_base_url(base_url: &str) -> Option<String> {
    if base_url.contains("kimi.com/coding") {
        return None;
    }

    let trimmed = base_url.trim_end_matches('/');

    if base_url.contains("api.moonshot.cn") || base_url.contains("api.moonshot.ai") {
        let api_base = if trimmed.ends_with("/v1") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/v1")
        };
        return Some(format!("{api_base}/users/me/balance"));
    }

    Some("https://api.moonshot.cn/v1/users/me/balance".to_string())
}

fn kimi_balance_currency(base_url: &str) -> &'static str {
    if base_url.contains("api.moonshot.ai") {
        "USD"
    } else {
        "CNY"
    }
}

fn parse_kimi_balance_response(
    body: &str,
    currency: &str,
) -> anyhow::Result<tact_protocol::BalanceInfo> {
    #[derive(serde::Deserialize)]
    struct RawKimiBalanceData {
        available_balance: f64,
        voucher_balance: f64,
        cash_balance: f64,
    }

    #[derive(serde::Deserialize)]
    struct RawKimiBalanceResponse {
        code: i32,
        status: bool,
        data: RawKimiBalanceData,
    }

    let raw: RawKimiBalanceResponse =
        serde_json::from_str(body).context("Failed to parse Kimi balance response")?;

    Ok(tact_protocol::BalanceInfo {
        is_available: raw.status && raw.code == 0,
        balance_infos: vec![tact_protocol::BalanceEntry {
            currency: currency.to_string(),
            total_balance: raw.data.available_balance,
            granted_balance: raw.data.voucher_balance,
            topped_up_balance: raw.data.cash_balance,
        }],
    })
}

/// Query Kimi/Moonshot account balance.
///
/// Calls `GET .../v1/users/me/balance` on `api.moonshot.cn` or `api.moonshot.ai`.
/// Returns `BalanceInfo` on success.
pub async fn query_kimi_balance() -> anyhow::Result<tact_protocol::BalanceInfo> {
    let provider = get_provider();
    let api_key = provider.api_key.clone();
    let base_url = provider.base_url.clone();

    let balance_url = kimi_balance_url_from_base_url(&base_url).ok_or_else(|| {
        anyhow::anyhow!("Kimi Code endpoint (api.kimi.com/coding) does not expose a balance API")
    })?;
    let currency = kimi_balance_currency(&base_url);

    let client = reqwest::Client::new();
    let resp = client
        .get(&balance_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .timeout(Duration::from_millis(5000))
        .send()
        .await
        .with_context(|| format!("Failed to query Kimi balance at {balance_url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Kimi balance query returned HTTP {}: {}",
            status,
            error_body.trim()
        );
    }

    let body = resp
        .text()
        .await
        .context("Failed to read Kimi balance response")?;

    parse_kimi_balance_response(&body, currency)
}

/// Derive the Kimi Code usage API URL from the configured base URL.
///
/// Works for the official endpoint and for custom proxies serving the
/// `kimi-for-coding` model. Falls back to the official endpoint when the
/// base URL is empty.
fn kimi_usage_url_from_base_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.is_empty() {
        return "https://api.kimi.com/coding/v1/usages".to_string();
    }
    let api_base = if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    };
    format!("{api_base}/usages")
}

/// Parse a quota number reported as a JSON string.
///
/// Kimi reports quota values as strings; non-numeric values (e.g. unlimited
/// markers) map to `None`.
fn parse_quota_value(value: &str) -> Option<f64> {
    value.trim().parse::<f64>().ok()
}

fn parse_kimi_usage_response(body: &str) -> anyhow::Result<tact_protocol::UsageQuotaInfo> {
    #[derive(serde::Deserialize)]
    struct RawUsageDetail {
        limit: String,
        remaining: String,
        #[serde(rename = "resetTime")]
        reset_time: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct RawWindow {
        duration: u64,
        #[serde(rename = "timeUnit")]
        time_unit: String,
    }

    #[derive(serde::Deserialize)]
    struct RawLimitEntry {
        window: RawWindow,
        detail: RawUsageDetail,
    }

    #[derive(serde::Deserialize)]
    struct RawMembership {
        level: String,
    }

    #[derive(serde::Deserialize)]
    struct RawUser {
        membership: Option<RawMembership>,
    }

    #[derive(serde::Deserialize)]
    struct RawKimiUsageResponse {
        usage: RawUsageDetail,
        #[serde(default)]
        limits: Vec<RawLimitEntry>,
        user: Option<RawUser>,
    }

    let raw: RawKimiUsageResponse =
        serde_json::from_str(body).context("Failed to parse Kimi usage response")?;

    let mut windows = vec![tact_protocol::UsageQuotaWindow {
        label: "week".to_string(),
        limit: parse_quota_value(&raw.usage.limit),
        remaining: parse_quota_value(&raw.usage.remaining),
        reset_time: raw.usage.reset_time.clone(),
    }];

    for entry in &raw.limits {
        let label = if entry.window.time_unit.contains("MINUTE") && entry.window.duration == 300 {
            "5h".to_string()
        } else {
            format!("{}m", entry.window.duration)
        };
        windows.push(tact_protocol::UsageQuotaWindow {
            label,
            limit: parse_quota_value(&entry.detail.limit),
            remaining: parse_quota_value(&entry.detail.remaining),
            reset_time: entry.detail.reset_time.clone(),
        });
    }

    let is_available = windows.iter().all(|w| w.has_remaining());

    Ok(tact_protocol::UsageQuotaInfo {
        is_available,
        windows,
        membership_level: raw.user.and_then(|u| u.membership).map(|m| m.level),
    })
}

/// Query Kimi Code subscription quota (`GET .../v1/usages`).
pub async fn query_kimi_code_usage() -> anyhow::Result<tact_protocol::UsageQuotaInfo> {
    let provider = get_provider();
    let api_key = provider.api_key.clone();
    let base_url = provider.base_url.clone();

    if !provider.is_kimi_coding() {
        anyhow::bail!("usage quota API is only available on Kimi Code (api.kimi.com/coding)");
    }
    let usage_url = kimi_usage_url_from_base_url(&base_url);

    let client = reqwest::Client::new();
    let resp = client
        .get(&usage_url)
        .header("Authorization", format!("Bearer {api_key}"))
        .header("Accept", "application/json")
        .header("User-Agent", "Claude Code")
        .timeout(Duration::from_millis(5000))
        .send()
        .await
        .with_context(|| format!("Failed to query Kimi usage at {usage_url}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let error_body = resp.text().await.unwrap_or_default();
        anyhow::bail!(
            "Kimi usage query returned HTTP {}: {}",
            status,
            error_body.trim()
        );
    }

    let body = resp
        .text()
        .await
        .context("Failed to read Kimi usage response")?;

    parse_kimi_usage_response(&body)
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

/// Returns true for the Kimi Code platform (`api.kimi.com/coding`).
pub fn is_kimi_coding() -> bool {
    get_provider().is_kimi_coding()
}

/// Returns true when Kimi balance queries are supported for the configured endpoint.
pub fn is_kimi_balance_supported() -> bool {
    get_provider().is_kimi_balance_supported()
}

/// Returns true when Kimi Code usage quota queries are supported.
pub fn is_kimi_usage_supported() -> bool {
    get_provider().is_kimi_usage_supported()
}

/// Returns true when account balance or usage quota queries are supported.
pub fn is_account_query_supported() -> bool {
    get_provider().is_account_query_supported()
}

// ── Mock client for integration testing ───────────────────────────

/// A single canned LLM turn for [`MockClient`].
struct MockTurn {
    blocks: Vec<ContentBlock>,
    stop_reason: Option<StopReason>,
    usage: Option<TokenUsageInfo>,
}

type MockTurnResult = Result<
    (
        Vec<ContentBlock>,
        Option<StopReason>,
        Option<TokenUsageInfo>,
    ),
    LlmError,
>;

/// Backing behavior for [`MockClient`].
trait MockClientInner: Send + Sync {
    /// Produce the next turn. `idx` is the 0-based call counter.
    fn next_turn(&self, request: &CreateMessageParams, idx: usize) -> MockTurnResult;
}

struct CannedMockInner {
    responses: Vec<MockTurn>,
}

impl MockClientInner for CannedMockInner {
    fn next_turn(
        &self,
        _request: &CreateMessageParams,
        idx: usize,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
        ),
        LlmError,
    > {
        let turn = &self.responses[idx % self.responses.len()];
        Ok((
            turn.blocks.clone(),
            clone_stop_reason(&turn.stop_reason),
            turn.usage.clone(),
        ))
    }
}

struct DynamicMockInner<F> {
    responder: F,
}

impl<F> MockClientInner for DynamicMockInner<F>
where
    F: Fn(
            &CreateMessageParams,
            usize,
        ) -> Result<
            (
                Vec<ContentBlock>,
                Option<StopReason>,
                Option<TokenUsageInfo>,
            ),
            LlmError,
        > + Send
        + Sync,
{
    fn next_turn(
        &self,
        request: &CreateMessageParams,
        idx: usize,
    ) -> Result<
        (
            Vec<ContentBlock>,
            Option<StopReason>,
            Option<TokenUsageInfo>,
        ),
        LlmError,
    > {
        (self.responder)(request, idx)
    }
}

fn clone_stop_reason(stop_reason: &Option<StopReason>) -> Option<StopReason> {
    stop_reason.as_ref().map(|s| {
        serde_json::from_value(serde_json::to_value(s).expect("serialize StopReason"))
            .expect("deserialize StopReason")
    })
}

fn clone_llm_error(e: &LlmError) -> LlmError {
    LlmError::Other(e.to_string())
}

/// Deterministic mock LLM client that returns scripted or dynamic responses.
///
/// Supports:
/// - Fixed sequences of turns (`new`, `with_usage`)
/// - Dynamic request-aware responses (`with_responder`)
/// - Turn-by-turn error injection (`with_error`)
/// - Optional streaming `StreamChunk` emission (`with_streaming_chunks`)
#[derive(Clone)]
pub struct MockClient {
    inner: Arc<dyn MockClientInner + Send + Sync>,
    counter: Arc<AtomicUsize>,
    emit_stream_chunks: bool,
}

impl MockClient {
    /// Create a mock client that cycles through the given responses.
    ///
    /// Each tuple provides content blocks and a stop reason. Token usage and
    /// the serialised request body are always `None`.
    pub fn new(responses: Vec<(Vec<ContentBlock>, Option<StopReason>)>) -> Self {
        Self::with_inner(
            Arc::new(CannedMockInner {
                responses: responses
                    .into_iter()
                    .map(|(blocks, stop_reason)| MockTurn {
                        blocks,
                        stop_reason,
                        usage: None,
                    })
                    .collect(),
            }),
            false,
        )
    }

    /// Like [`Self::new`], but attaches token usage to each turn (and emits
    /// [`AgentUpdate::TokenUsage`] on `stream_message` when `ui_tx` is set).
    pub fn with_usage(
        responses: Vec<(Vec<ContentBlock>, Option<StopReason>, TokenUsageInfo)>,
    ) -> Self {
        Self::with_inner(
            Arc::new(CannedMockInner {
                responses: responses
                    .into_iter()
                    .map(|(blocks, stop_reason, usage)| MockTurn {
                        blocks,
                        stop_reason,
                        usage: Some(usage),
                    })
                    .collect(),
            }),
            false,
        )
    }

    /// Create a mock client driven by a closure.
    ///
    /// The closure receives the full LLM request and the 0-based call counter,
    /// and returns either a successful turn `(blocks, stop_reason, usage)` or
    /// an [`LlmError`]. This makes it possible to assert on the request body,
    /// branch on previous tool results, and inject failures.
    pub fn with_responder<F>(responder: F) -> Self
    where
        F: Fn(
                &CreateMessageParams,
                usize,
            ) -> Result<
                (
                    Vec<ContentBlock>,
                    Option<StopReason>,
                    Option<TokenUsageInfo>,
                ),
                LlmError,
            > + Send
            + Sync
            + 'static,
    {
        Self::with_inner(Arc::new(DynamicMockInner { responder }), false)
    }

    /// Create a mock client where the given errors are returned in order.
    ///
    /// If a call exceeds the error list, the client falls back to an empty
    /// successful turn.
    pub fn with_error(errors: Vec<LlmError>) -> Self {
        Self::with_responder(move |_request, idx| {
            errors
                .get(idx)
                .map(|e| Err(clone_llm_error(e)))
                .unwrap_or_else(|| Ok((vec![], None, None)))
        })
    }

    /// Enable emission of [`AgentUpdate::StreamChunk`] events during
    /// `stream_message` by splitting text blocks into word-sized chunks.
    pub fn with_streaming_chunks(self) -> Self {
        Self {
            emit_stream_chunks: true,
            ..self
        }
    }

    fn with_inner(inner: Arc<dyn MockClientInner + Send + Sync>, emit_stream_chunks: bool) -> Self {
        Self {
            inner,
            counter: Arc::new(AtomicUsize::new(0)),
            emit_stream_chunks,
        }
    }

    fn next_turn(&self, request: &CreateMessageParams) -> MockTurnResult {
        let idx = self.counter.fetch_add(1, Ordering::Relaxed);
        self.inner.next_turn(request, idx)
    }

    fn emit_token_usage(ui_tx: &Option<UnboundedSender<AgentUpdate>>, usage: &TokenUsageInfo) {
        if let Some(tx) = ui_tx {
            let _ = tx.send(AgentUpdate::TokenUsage(usage.clone()));
        }
    }

    fn emit_stream_chunks(ui_tx: &Option<UnboundedSender<AgentUpdate>>, blocks: &[ContentBlock]) {
        let Some(tx) = ui_tx else { return };
        for block in blocks {
            if let ContentBlock::Text { text } = block {
                // Emit word-by-word to simulate streaming without overloading the channel.
                let words: Vec<&str> = text.split_whitespace().collect();
                let n = words.len();
                for (i, word) in words.into_iter().enumerate() {
                    let chunk = if i + 1 == n {
                        word.to_string()
                    } else {
                        format!("{word} ")
                    };
                    let _ = tx.send(AgentUpdate::StreamChunk(chunk));
                }
            }
        }
    }
}

#[async_trait::async_trait]
impl LlmClient for MockClient {
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
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        if let Some(ref u) = usage {
            Self::emit_token_usage(&ui_tx, u);
        }
        if self.emit_stream_chunks {
            Self::emit_stream_chunks(&ui_tx, &blocks);
        }
        Ok((blocks, stop_reason, usage, None))
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
        let (blocks, stop_reason, usage) = self.next_turn(request)?;
        Ok((blocks, stop_reason, usage, None))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_info(provider: &str, api_key: &str, base_url: &str, model: &str) -> ProviderInfo {
        ProviderInfo {
            provider: provider.to_string(),
            api_key: api_key.to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
        }
    }

    #[test]
    fn build_client_requires_api_key() {
        let p = provider_info("deepseek", "", "", "deepseek-chat");
        assert!(p.build_client().is_err());
    }

    #[test]
    fn deepseek_builds_openai_adapter_with_default_base_url() {
        let p = provider_info("deepseek", "sk-test", "", "deepseek-chat");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAi(adapter) = result.unwrap() else {
            panic!("expected OpenAi adapter for deepseek");
        };
        assert_eq!(adapter.base_url(), "https://api.deepseek.com");
    }

    #[test]
    fn kimi_builds_openai_adapter_with_default_base_url() {
        let p = provider_info("kimi", "sk-test", "", "kimi-k2.5");
        let result = p.build_client();
        assert!(result.is_ok());
        let LlmProvider::OpenAi(adapter) = result.unwrap() else {
            panic!("expected OpenAi adapter for kimi");
        };
        assert_eq!(adapter.base_url(), "https://api.moonshot.cn/v1");
    }

    #[test]
    fn custom_base_url_is_preserved() {
        let p = provider_info(
            "kimi",
            "sk-test",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        let result = p.build_client().unwrap();
        let LlmProvider::OpenAi(adapter) = result else {
            panic!("expected OpenAi adapter");
        };
        assert_eq!(adapter.base_url(), "https://api.kimi.com/coding/v1");
    }

    #[test]
    fn is_kimi_detection() {
        assert!(provider_info("kimi", "", "", "kimi-k2.5").is_kimi());
        assert!(provider_info("openai", "", "https://api.moonshot.cn/v1", "").is_kimi());
        assert!(provider_info("openai", "", "https://api.kimi.com/coding/v1", "").is_kimi());
        assert!(provider_info("openai", "", "", "kimi-k2.5").is_kimi());
        assert!(!provider_info("anthropic", "", "", "claude-sonnet-4").is_kimi());
    }

    #[test]
    fn is_kimi_k2x_and_k27() {
        let k25 = provider_info("kimi", "", "", "kimi-k2.5");
        assert!(k25.is_kimi_k2x());
        assert!(!k25.is_kimi_k27());

        let k27 = provider_info("kimi", "", "", "kimi-k2.7");
        assert!(k27.is_kimi_k2x());
        assert!(k27.is_kimi_k27());

        let coding = provider_info(
            "openai",
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
            "openai",
            "",
            "https://api.kimi.com/coding/v1",
            "kimi-for-coding",
        );
        assert!(coding.is_kimi_coding());
        assert!(!coding.is_kimi_balance_supported());
        assert!(coding.is_kimi_usage_supported());

        let cn = provider_info("kimi", "", "https://api.moonshot.cn/v1", "kimi-k2.5");
        assert!(!cn.is_kimi_coding());
        assert!(cn.is_kimi_balance_supported());
        assert!(!cn.is_kimi_usage_supported());

        // kimi-for-coding behind a custom proxy is still Kimi Code:
        // no balance API, usage quota supported.
        let proxy = provider_info(
            "openai",
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
            "anthropic",
            "",
            "https://api.anthropic.com",
            "claude-sonnet-4",
        );
        assert!(!anthropic.is_account_query_supported());
    }

    #[test]
    fn kimi_usage_url_derivation() {
        assert_eq!(
            kimi_usage_url_from_base_url("https://api.kimi.com/coding/v1"),
            "https://api.kimi.com/coding/v1/usages"
        );
        assert_eq!(
            kimi_usage_url_from_base_url("https://api.kimi.com/coding/v1/"),
            "https://api.kimi.com/coding/v1/usages"
        );
        // Custom proxy serving kimi-for-coding: derive from the proxy base.
        assert_eq!(
            kimi_usage_url_from_base_url("https://proxy.example.com"),
            "https://proxy.example.com/v1/usages"
        );
        // Empty base URL falls back to the official endpoint.
        assert_eq!(
            kimi_usage_url_from_base_url(""),
            "https://api.kimi.com/coding/v1/usages"
        );
    }

    #[test]
    fn parse_kimi_usage_response_maps_official_schema() {
        let body = r#"{
            "usage": {"limit": "100", "remaining": "74", "resetTime": "2026-02-11T17:32:50Z"},
            "limits": [{
                "window": {"duration": 300, "timeUnit": "TIME_UNIT_MINUTE"},
                "detail": {"limit": "100", "remaining": "85", "resetTime": "2026-02-07T12:32:50Z"}
            }],
            "user": {"membership": {"level": "LEVEL_INTERMEDIATE"}}
        }"#;
        let info = parse_kimi_usage_response(body).unwrap();
        assert!(info.is_available);
        assert_eq!(info.windows.len(), 2);
        assert_eq!(info.windows[0].label, "week");
        assert_eq!(info.windows[0].remaining, Some(74.0));
        assert_eq!(info.windows[1].label, "5h");
        assert_eq!(info.windows[1].remaining, Some(85.0));
        assert_eq!(info.membership_level.as_deref(), Some("LEVEL_INTERMEDIATE"));
    }

    #[test]
    fn parse_kimi_usage_response_unavailable_when_remaining_zero() {
        let body = r#"{
            "usage": {"limit": "100", "remaining": "0", "resetTime": "2026-02-11T17:32:50Z"},
            "limits": []
        }"#;
        let info = parse_kimi_usage_response(body).unwrap();
        assert!(!info.is_available);
    }

    #[test]
    fn kimi_balance_url_derivation() {
        assert_eq!(
            kimi_balance_url_from_base_url("https://api.moonshot.cn/v1"),
            Some("https://api.moonshot.cn/v1/users/me/balance".to_string())
        );
        assert_eq!(
            kimi_balance_url_from_base_url("https://api.moonshot.cn/v1/"),
            Some("https://api.moonshot.cn/v1/users/me/balance".to_string())
        );
        assert_eq!(
            kimi_balance_url_from_base_url("https://api.moonshot.ai/v1"),
            Some("https://api.moonshot.ai/v1/users/me/balance".to_string())
        );
        assert_eq!(
            kimi_balance_url_from_base_url("https://api.moonshot.cn"),
            Some("https://api.moonshot.cn/v1/users/me/balance".to_string())
        );
        assert_eq!(
            kimi_balance_url_from_base_url("https://api.kimi.com/coding/v1"),
            None
        );
        assert_eq!(
            kimi_balance_url_from_base_url(""),
            Some("https://api.moonshot.cn/v1/users/me/balance".to_string())
        );
    }

    #[test]
    fn parse_kimi_balance_response_maps_official_schema() {
        let body = r#"{"code":0,"status":true,"scode":"0x0","data":{"available_balance":49.58,"voucher_balance":46.58,"cash_balance":3.0}}"#;
        let info = parse_kimi_balance_response(body, "CNY").unwrap();
        assert!(info.is_available);
        assert_eq!(info.balance_infos.len(), 1);
        let entry = &info.balance_infos[0];
        assert_eq!(entry.currency, "CNY");
        assert_eq!(entry.total_balance, 49.58);
        assert_eq!(entry.granted_balance, 46.58);
        assert_eq!(entry.topped_up_balance, 3.0);
    }

    #[test]
    fn parse_kimi_balance_response_unavailable_when_code_nonzero() {
        let body = r#"{"code":1,"status":false,"data":{"available_balance":0.0,"voucher_balance":0.0,"cash_balance":0.0}}"#;
        let info = parse_kimi_balance_response(body, "USD").unwrap();
        assert!(!info.is_available);
        assert_eq!(info.balance_infos[0].currency, "USD");
    }

    #[test]
    fn unknown_provider_errors() {
        let p = provider_info("google", "sk-test", "", "gemini");
        match p.build_client() {
            Ok(_) => panic!("expected error for unknown provider"),
            Err(e) => {
                let err = e.to_string();
                assert!(err.contains("Unknown provider"));
                assert!(err.contains("google"));
            }
        }
    }

    #[tokio::test]
    async fn mock_stream_emits_token_usage_when_configured() {
        use anthropic_ai_sdk::types::message::ContentBlock;
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
                &CreateMessageParams::new(
                    anthropic_ai_sdk::types::message::RequiredMessageParams {
                        model: "mock".to_string(),
                        messages: vec![],
                        max_tokens: 100,
                    },
                ),
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
}
