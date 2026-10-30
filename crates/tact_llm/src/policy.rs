//! The single owner of provider, model, and endpoint interpretation.

use crate::types::OpenAiProtocol;
use crate::{ChatCompletionsDialect, ProviderKind, WireProtocol};

#[derive(Debug, Clone, Copy)]
pub(crate) struct ProviderPolicy {
    kind: Kind,
    protocol: WireProtocol,
}

#[derive(Debug, Clone, Copy)]
enum Kind {
    Anthropic,
    OpenAi,
    DeepSeek,
    Kimi,
    Custom,
}

impl ProviderPolicy {
    pub(crate) fn for_target(kind: &ProviderKind, protocol: WireProtocol) -> Self {
        let kind = match kind {
            ProviderKind::Anthropic => Kind::Anthropic,
            ProviderKind::OpenAi => Kind::OpenAi,
            ProviderKind::DeepSeek => Kind::DeepSeek,
            ProviderKind::Kimi => Kind::Kimi,
            ProviderKind::Custom(_) => Kind::Custom,
        };
        Self { kind, protocol }
    }

    pub(crate) fn default_endpoint(self) -> Option<&'static str> {
        default_endpoint_for_kind(self.kind)
    }

    pub(crate) fn native_compaction(self) -> bool {
        self.protocol == WireProtocol::OpenAiResponses && !matches!(self.kind, Kind::DeepSeek)
    }

    pub(crate) fn supports_vision(self) -> bool {
        !matches!(self.kind, Kind::DeepSeek)
    }
}

fn default_endpoint_for_kind(kind: Kind) -> Option<&'static str> {
    match kind {
        Kind::Anthropic => Some("https://api.anthropic.com"),
        Kind::OpenAi => Some("https://api.openai.com/v1"),
        Kind::DeepSeek => Some("https://api.deepseek.com"),
        Kind::Kimi => Some("https://api.moonshot.cn/v1"),
        Kind::Custom => None,
    }
}

fn kind_class(kind: &ProviderKind) -> Kind {
    match kind {
        ProviderKind::Anthropic => Kind::Anthropic,
        ProviderKind::OpenAi => Kind::OpenAi,
        ProviderKind::DeepSeek => Kind::DeepSeek,
        ProviderKind::Kimi => Kind::Kimi,
        ProviderKind::Custom(_) => Kind::Custom,
    }
}

/// Legacy-compatible endpoint defaults. Anthropic keeps requiring an explicit
/// URL on the compatibility construction path.
pub(crate) fn legacy_default_base_url(kind: &ProviderKind) -> Option<&'static str> {
    match kind {
        ProviderKind::Anthropic => None,
        _ => default_endpoint_for_kind(kind_class(kind)),
    }
}

pub(crate) fn is_kimi(provider: &ProviderKind, base_url: &str, model: &str) -> bool {
    *provider == ProviderKind::Kimi
        || base_url.contains("moonshot")
        || base_url.contains("kimi")
        || model.contains("kimi")
}

pub(crate) fn is_kimi_k2x(provider: &ProviderKind, base_url: &str, model: &str) -> bool {
    if !is_kimi(provider, base_url, model) {
        return false;
    }
    if model == "kimi-for-coding" || base_url.contains("kimi.com/coding") {
        return true;
    }
    model.contains("kimi-k2") || model.contains("k2.") || model.contains("k2-")
}

pub(crate) fn is_kimi_k27(provider: &ProviderKind, base_url: &str, model: &str) -> bool {
    if !is_kimi(provider, base_url, model) {
        return false;
    }
    if model == "kimi-for-coding" || base_url.contains("kimi.com/coding") {
        return true;
    }
    model.contains("k2.7") || model.contains("k2-7")
}

pub(crate) fn is_kimi_coding(base_url: &str, model: &str) -> bool {
    base_url.contains("kimi.com/coding") || model == "kimi-for-coding"
}

pub(crate) fn is_kimi_balance_supported(
    provider: &ProviderKind,
    base_url: &str,
    model: &str,
) -> bool {
    if !is_kimi(provider, base_url, model) || is_kimi_coding(base_url, model) {
        return false;
    }
    if base_url.is_empty() {
        return *provider == ProviderKind::Kimi;
    }
    url::Url::parse(base_url).is_ok_and(|url| {
        url.scheme() == "https"
            && matches!(url.host_str(), Some("api.moonshot.cn" | "api.moonshot.ai"))
    })
}

pub(crate) fn is_kimi_usage_supported(base_url: &str, model: &str) -> bool {
    if !is_kimi_coding(base_url, model) {
        return false;
    }
    url::Url::parse(base_url).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == Some("api.kimi.com")
            && url.path().contains("coding")
    })
}

pub(crate) fn is_deepseekish(provider: &ProviderKind, base_url: &str, model: &str) -> bool {
    *provider == ProviderKind::DeepSeek
        || base_url.contains("deepseek")
        || model.contains("deepseek")
}

pub(crate) fn is_deepseek_balance_supported(
    provider: &ProviderKind,
    base_url: &str,
    model: &str,
) -> bool {
    if !is_deepseekish(provider, base_url, model) {
        return false;
    }
    if base_url.is_empty() {
        return *provider == ProviderKind::DeepSeek;
    }
    url::Url::parse(base_url)
        .is_ok_and(|url| url.scheme() == "https" && url.host_str() == Some("api.deepseek.com"))
}

pub(crate) fn is_account_query_supported(
    provider: &ProviderKind,
    base_url: &str,
    model: &str,
) -> bool {
    is_deepseek_balance_supported(provider, base_url, model)
        || is_kimi_balance_supported(provider, base_url, model)
        || is_kimi_usage_supported(base_url, model)
}

#[cfg(feature = "account")]
pub(crate) fn deepseek_balance_url(base_url: &str) -> Option<String> {
    if base_url.is_empty() {
        return Some("https://api.deepseek.com/user/balance".to_string());
    }
    let parsed = url::Url::parse(base_url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("api.deepseek.com") {
        return None;
    }
    Some("https://api.deepseek.com/user/balance".to_string())
}

#[cfg(feature = "account")]
pub(crate) fn kimi_balance_url(base_url: &str) -> Option<String> {
    if base_url.is_empty() {
        return Some("https://api.moonshot.cn/v1/users/me/balance".to_string());
    }
    let parsed = url::Url::parse(base_url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    match parsed.host_str()? {
        "api.moonshot.cn" => Some("https://api.moonshot.cn/v1/users/me/balance".to_string()),
        "api.moonshot.ai" => Some("https://api.moonshot.ai/v1/users/me/balance".to_string()),
        _ => None,
    }
}

#[cfg(feature = "account")]
pub(crate) fn kimi_balance_currency(base_url: &str) -> &'static str {
    if url::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .as_deref()
        == Some("api.moonshot.ai")
    {
        "USD"
    } else {
        "CNY"
    }
}

#[cfg(feature = "account")]
pub(crate) fn kimi_usage_url(base_url: &str) -> Option<String> {
    let parsed = url::Url::parse(base_url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("api.kimi.com") {
        return None;
    }
    if !parsed.path().contains("coding") {
        return None;
    }
    let trimmed = parsed.path().trim_end_matches('/');
    let api_base = if trimmed.ends_with("/v1") {
        trimmed.to_string()
    } else {
        format!("{trimmed}/v1")
    };
    Some(format!("https://api.kimi.com{api_base}/usages"))
}

pub(crate) fn supports_vision(provider: &ProviderKind, base_url: &str, model: &str) -> bool {
    if is_deepseekish(provider, base_url, model) {
        return crate::types::is_deepseek_vision_model(model);
    }
    !matches!(provider, ProviderKind::DeepSeek)
}

pub(crate) fn is_kimi_k3(model: &str) -> bool {
    model == "k3" || model == "k3-256k"
}

pub(crate) fn model_uses_effort(provider: &ProviderKind, base_url: &str, model: &str) -> bool {
    match provider {
        ProviderKind::DeepSeek => true,
        ProviderKind::Anthropic => false,
        ProviderKind::Kimi => is_kimi_k3(model),
        ProviderKind::OpenAi | ProviderKind::Custom(_) => {
            if is_kimi(provider, base_url, model) {
                is_kimi_k3(model)
            } else {
                true
            }
        }
    }
}

pub(crate) fn dialect_for(
    provider: &ProviderKind,
    base_url: &str,
    model: &str,
) -> Option<ChatCompletionsDialect> {
    match provider {
        ProviderKind::DeepSeek => Some(ChatCompletionsDialect::DeepSeek),
        ProviderKind::Kimi => Some(ChatCompletionsDialect::Kimi),
        ProviderKind::OpenAi | ProviderKind::Custom(_) => {
            if is_kimi(provider, base_url, model) {
                Some(ChatCompletionsDialect::Kimi)
            } else if is_deepseekish(provider, base_url, model) {
                Some(ChatCompletionsDialect::DeepSeek)
            } else {
                Some(ChatCompletionsDialect::Standard)
            }
        }
        ProviderKind::Anthropic => None,
    }
}

pub(crate) fn routed_kind(provider: &ProviderKind, base_url: &str, model: &str) -> ProviderKind {
    if is_deepseekish(provider, base_url, model) {
        ProviderKind::DeepSeek
    } else {
        provider.clone()
    }
}

#[cfg(feature = "model-discovery")]
pub(crate) fn supports_models_query(provider: &ProviderKind) -> bool {
    !matches!(provider, ProviderKind::Anthropic)
}

pub(crate) fn wire_protocol(provider: &ProviderKind, protocol: OpenAiProtocol) -> WireProtocol {
    match provider {
        ProviderKind::Anthropic => WireProtocol::AnthropicMessages,
        _ => match protocol {
            OpenAiProtocol::ChatCompletions => WireProtocol::OpenAiChatCompletions,
            OpenAiProtocol::Responses => WireProtocol::OpenAiResponses,
        },
    }
}

pub(crate) fn is_official_openai_endpoint(base_url: &str) -> bool {
    let host = base_url.to_ascii_lowercase();
    host.contains("api.openai.com") || host.contains("openai.azure.com")
}

pub(crate) fn reasoning_replay_required(model: &str, thinking_or_effort: bool) -> bool {
    thinking_or_effort && model.to_ascii_lowercase().contains("deepseek")
}

pub(crate) fn web_search_uses_queries_shape(model: &str) -> bool {
    model.to_ascii_lowercase().contains("deepseek")
}
