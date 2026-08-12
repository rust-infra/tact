//! Provider account balance / usage queries.

use std::time::Duration;

use anyhow::Context;
use secrecy::ExposeSecret;

use crate::{
    CredentialProvider, ProviderProfile, SharedHttpClient,
    provider::{active_credential_provider, get_provider},
};

/// Derive the DeepSeek balance API URL from the configured base URL.
///
/// Returns `None` unless the URL targets the official DeepSeek API host.
///
/// DeepSeek only serves `GET /user/balance` on the official endpoint; custom
/// OpenAI-compatible proxies / gateways do not implement it. The API key is
/// therefore never sent anywhere but `https://api.deepseek.com`.
fn deepseek_balance_url_from_base_url(base_url: &str) -> Option<String> {
    if base_url.is_empty() {
        return Some("https://api.deepseek.com/user/balance".to_string());
    }

    let parsed = reqwest::Url::parse(base_url).ok()?;
    if parsed.scheme() != "https" || parsed.host_str() != Some("api.deepseek.com") {
        return None;
    }
    Some("https://api.deepseek.com/user/balance".to_string())
}

/// Query DeepSeek account balance.
///
/// Calls `GET https://api.deepseek.com/user/balance` with the provided API key.
/// Returns `BalanceInfo` on success.
///
/// Only supported when the configured base URL targets the official DeepSeek
/// API host (`https://api.deepseek.com`, with or without a `/v1` suffix).
pub async fn query_deepseek_balance() -> anyhow::Result<tact_protocol::BalanceInfo> {
    let provider = get_provider();
    let profile = provider.to_profile();
    let credentials = active_credential_provider();
    query_deepseek_balance_for(&profile, credentials.as_ref(), &SharedHttpClient::default()).await
}

/// Query DeepSeek account balance with explicit credentials and transport.
///
/// Calls `GET https://api.deepseek.com/user/balance` with the resolved
/// credential. Only supported when the configured base URL targets the
/// official DeepSeek API host.
pub async fn query_deepseek_balance_for(
    profile: &ProviderProfile,
    credentials: &dyn CredentialProvider,
    http: &SharedHttpClient,
) -> anyhow::Result<tact_protocol::BalanceInfo> {
    let balance_url = deepseek_balance_url_from_base_url(&profile.base_url).ok_or_else(|| {
        anyhow::anyhow!(
            "DeepSeek balance API is only available for the official endpoint https://api.deepseek.com"
        )
    })?;
    let secret = credentials.resolve().await?;

    let resp = http
        .inner()
        .get(&balance_url)
        .header(
            "Authorization",
            format!("Bearer {}", secret.expose_secret()),
        )
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
/// Returns `None` unless the URL targets an official Moonshot API host.
fn kimi_balance_url_from_base_url(base_url: &str) -> Option<String> {
    if base_url.is_empty() {
        return Some("https://api.moonshot.cn/v1/users/me/balance".to_string());
    }

    let parsed = reqwest::Url::parse(base_url).ok()?;
    if parsed.scheme() != "https" {
        return None;
    }
    match parsed.host_str()? {
        "api.moonshot.cn" => Some("https://api.moonshot.cn/v1/users/me/balance".to_string()),
        "api.moonshot.ai" => Some("https://api.moonshot.ai/v1/users/me/balance".to_string()),
        _ => None,
    }
}

fn kimi_balance_currency(base_url: &str) -> &'static str {
    if reqwest::Url::parse(base_url)
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
    let profile = provider.to_profile();
    let credentials = active_credential_provider();
    query_kimi_balance_for(&profile, credentials.as_ref(), &SharedHttpClient::default()).await
}

/// Query Kimi/Moonshot account balance with explicit credentials and transport.
///
/// Calls `GET .../v1/users/me/balance` on `api.moonshot.cn` or
/// `api.moonshot.ai` with the resolved credential.
pub async fn query_kimi_balance_for(
    profile: &ProviderProfile,
    credentials: &dyn CredentialProvider,
    http: &SharedHttpClient,
) -> anyhow::Result<tact_protocol::BalanceInfo> {
    let balance_url = kimi_balance_url_from_base_url(&profile.base_url).ok_or_else(|| {
        anyhow::anyhow!("Kimi balance API is only available for official Moonshot API endpoints")
    })?;
    let currency = kimi_balance_currency(&profile.base_url);
    let secret = credentials.resolve().await?;

    let resp = http
        .inner()
        .get(&balance_url)
        .header(
            "Authorization",
            format!("Bearer {}", secret.expose_secret()),
        )
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
/// Returns `None` unless the URL targets the official Kimi Code API host
/// (`https://api.kimi.com/coding`). Custom OpenAI-compatible proxies do not
/// serve the usage quota API; the API key is never sent to `api.kimi.com`
/// from a proxy configuration.
fn kimi_usage_url_from_base_url(base_url: &str) -> Option<String> {
    let parsed = reqwest::Url::parse(base_url).ok()?;
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
///
/// Only supported when the configured base URL targets the official Kimi Code
/// endpoint (`https://api.kimi.com/coding`).
pub async fn query_kimi_code_usage() -> anyhow::Result<tact_protocol::UsageQuotaInfo> {
    let provider = get_provider();
    let profile = provider.to_profile();
    let credentials = active_credential_provider();
    query_kimi_code_usage_for(&profile, credentials.as_ref(), &SharedHttpClient::default()).await
}

/// Query Kimi Code subscription quota with explicit credentials and transport.
///
/// Only supported when the configured base URL targets the official Kimi Code
/// endpoint (`https://api.kimi.com/coding`).
pub async fn query_kimi_code_usage_for(
    profile: &ProviderProfile,
    credentials: &dyn CredentialProvider,
    http: &SharedHttpClient,
) -> anyhow::Result<tact_protocol::UsageQuotaInfo> {
    let usage_url = kimi_usage_url_from_base_url(&profile.base_url).ok_or_else(|| {
        anyhow::anyhow!(
            "Kimi Code usage API is only available for the official endpoint https://api.kimi.com/coding"
        )
    })?;
    let secret = credentials.resolve().await?;

    let resp = http
        .inner()
        .get(&usage_url)
        .header(
            "Authorization",
            format!("Bearer {}", secret.expose_secret()),
        )
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

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::future::BoxFuture;

    use crate::{LlmError, ProviderKind};

    #[derive(Debug)]
    struct FailingCredential;

    impl CredentialProvider for FailingCredential {
        fn resolve(&self) -> BoxFuture<'_, Result<secrecy::SecretString, LlmError>> {
            Box::pin(async { Err(LlmError::Auth("test credential unavailable".to_string())) })
        }
    }

    fn deepseek_profile() -> ProviderProfile {
        ProviderProfile {
            provider: ProviderKind::DeepSeek,
            base_url: "https://api.deepseek.com/v1".to_string(),
            model: "deepseek-chat".to_string(),
            protocol: crate::OpenAiProtocol::default(),
            responses_compact_threshold: None,
        }
    }

    fn proxy_profile(provider: ProviderKind) -> ProviderProfile {
        ProviderProfile {
            provider,
            base_url: "https://proxy.example.com/v1".to_string(),
            model: "test-model".to_string(),
            protocol: crate::OpenAiProtocol::default(),
            responses_compact_threshold: None,
        }
    }

    #[test]
    fn deepseek_balance_url_derivation() {
        assert_eq!(
            deepseek_balance_url_from_base_url("https://api.deepseek.com"),
            Some("https://api.deepseek.com/user/balance".to_string())
        );
        assert_eq!(
            deepseek_balance_url_from_base_url("https://api.deepseek.com/v1"),
            Some("https://api.deepseek.com/user/balance".to_string())
        );
        assert_eq!(
            deepseek_balance_url_from_base_url("https://api.deepseek.com/v1/"),
            Some("https://api.deepseek.com/user/balance".to_string())
        );
        // Empty base URL falls back to the official endpoint.
        assert_eq!(
            deepseek_balance_url_from_base_url(""),
            Some("https://api.deepseek.com/user/balance".to_string())
        );
        // Custom proxies / gateways do not serve the DeepSeek balance API.
        assert_eq!(
            deepseek_balance_url_from_base_url("https://proxy.example.com/v1"),
            None
        );
        // Host-name spoofing and plain HTTP are rejected too.
        assert_eq!(
            deepseek_balance_url_from_base_url("https://api.deepseek.com.evil.example/v1"),
            None
        );
        assert_eq!(
            deepseek_balance_url_from_base_url("http://api.deepseek.com/v1"),
            None
        );
    }

    #[test]
    fn kimi_usage_url_derivation() {
        assert_eq!(
            kimi_usage_url_from_base_url("https://api.kimi.com/coding/v1"),
            Some("https://api.kimi.com/coding/v1/usages".to_string())
        );
        assert_eq!(
            kimi_usage_url_from_base_url("https://api.kimi.com/coding/v1/"),
            Some("https://api.kimi.com/coding/v1/usages".to_string())
        );
        assert_eq!(
            kimi_usage_url_from_base_url("https://api.kimi.com/coding"),
            Some("https://api.kimi.com/coding/v1/usages".to_string())
        );
        // Custom proxy serving kimi-for-coding has no usage quota API.
        assert_eq!(
            kimi_usage_url_from_base_url("https://proxy.example.com/v1"),
            None
        );
        // Official host without the /coding path is not the Kimi Code API.
        assert_eq!(kimi_usage_url_from_base_url("https://api.kimi.com"), None);
        // Host-name spoofing and plain HTTP are rejected too.
        assert_eq!(
            kimi_usage_url_from_base_url("https://api.kimi.com.evil.example/coding/v1"),
            None
        );
        assert_eq!(
            kimi_usage_url_from_base_url("http://api.kimi.com/coding/v1"),
            None
        );
        // Empty base URL has no coding default (Kimi defaults to Moonshot).
        assert_eq!(kimi_usage_url_from_base_url(""), None);
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
            kimi_balance_url_from_base_url("https://proxy.example.com/v1"),
            None
        );
        assert_eq!(
            kimi_balance_url_from_base_url("https://api.moonshot.cn.attacker.example/v1"),
            None
        );
        assert_eq!(
            kimi_balance_url_from_base_url("http://api.moonshot.cn/v1"),
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

    #[tokio::test]
    async fn explicit_deepseek_balance_forwards_credential_errors() {
        let profile = deepseek_profile();
        let error =
            query_deepseek_balance_for(&profile, &FailingCredential, &SharedHttpClient::default())
                .await
                .unwrap_err();

        assert!(error.to_string().contains("test credential unavailable"));
    }

    #[tokio::test]
    async fn unsupported_endpoint_fails_without_resolving_credentials() {
        let profile = proxy_profile(ProviderKind::DeepSeek);
        let error =
            query_deepseek_balance_for(&profile, &FailingCredential, &SharedHttpClient::default())
                .await
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only available for the official endpoint")
        );

        let profile = proxy_profile(ProviderKind::Kimi);
        let error =
            query_kimi_balance_for(&profile, &FailingCredential, &SharedHttpClient::default())
                .await
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only available for official Moonshot API endpoints")
        );

        let profile = proxy_profile(ProviderKind::Kimi);
        let error =
            query_kimi_code_usage_for(&profile, &FailingCredential, &SharedHttpClient::default())
                .await
                .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("only available for the official endpoint https://api.kimi.com/coding")
        );
    }
}
