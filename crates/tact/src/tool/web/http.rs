//! Shared HTTP utilities for web tools.

use std::{net::IpAddr, sync::OnceLock, time::Duration};

use anyhow::{Context, Result};
use reqwest::{Url, redirect::Policy};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for trusted, hard-coded API endpoints (e.g. web search providers).
pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::custom(|attempt| {
                if attempt.previous().len() >= 10 {
                    return attempt.error("too many redirects");
                }
                match validate_public_http_url(attempt.url().as_str()) {
                    Ok(_) => attempt.follow(),
                    Err(e) => attempt.error(e),
                }
            }))
            .build()
            .expect("failed to build shared HTTP client")
    })
}

/// HTTP client for user-supplied URLs. Redirects are disabled and callers must
/// run [`validate_public_http_url_resolved`] before fetching.
pub fn public_fetch_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(Policy::none())
            .build()
            .expect("failed to build public fetch HTTP client")
    })
}

/// Validates that a URL is a safe public http(s) target for outbound fetches.
pub fn validate_public_http_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("Invalid URL")?;

    match url.scheme() {
        "http" | "https" => {},
        scheme => anyhow::bail!("Only http and https URLs are allowed, got {scheme}"),
    }

    if url.username() != "" || url.password().is_some() {
        anyhow::bail!("URLs with embedded credentials are not allowed");
    }

    let host = url.host_str().ok_or_else(|| anyhow::anyhow!("URL must have a host"))?;

    if is_blocked_host(host) {
        anyhow::bail!("Blocked host: {host}");
    }

    Ok(url)
}

/// Validates a URL and resolves hostnames to ensure they do not point at
/// private or internal addresses (DNS rebinding mitigation at lookup time).
pub async fn validate_public_http_url_resolved(raw: &str) -> Result<Url> {
    let url = validate_public_http_url(raw)?;

    if let Some(host) = url.host_str()
        && host.parse::<IpAddr>().is_err()
    {
        let port = url.port_or_known_default().unwrap_or(443);
        validate_resolved_host(host, port).await?;
    }

    Ok(url)
}

async fn validate_resolved_host(host: &str, port: u16) -> Result<()> {
    let mut resolved_any = false;
    let mut addrs =
        tokio::net::lookup_host((host, port)).await.with_context(|| format!("failed to resolve host {host}"))?;

    for addr in addrs.by_ref() {
        resolved_any = true;
        if is_blocked_ip(addr.ip()) {
            anyhow::bail!("Blocked host: {host} resolves to private/internal address");
        }
    }

    if !resolved_any {
        anyhow::bail!("host {host} did not resolve to any addresses");
    }

    Ok(())
}

fn is_blocked_host(host: &str) -> bool {
    let host_lower = host.to_lowercase();
    if host_lower == "localhost" || host_lower.ends_with(".localhost") {
        return true;
    }

    if let Ok(ip) = host.parse::<IpAddr>() {
        return is_blocked_ip(ip);
    }

    false
}

fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private() || v4.is_loopback() || v4.is_link_local() || v4.is_unspecified() || v4.is_broadcast()
        },
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified() || v6.is_unicast_link_local() || v6.is_unique_local(),
    }
}

pub fn encode_query(value: &str) -> String {
    use std::fmt::Write as _;

    let mut encoded = String::with_capacity(value.len());
    let mut buf = [0u8; 4];
    for ch in value.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => encoded.push(ch),
            ' ' => encoded.push('+'),
            _ => {
                for &byte in ch.encode_utf8(&mut buf).as_bytes() {
                    // Writing to a String is infallible.
                    let _ = write!(encoded, "%{byte:02X}");
                }
            },
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_public_http_url_allows_https() {
        validate_public_http_url("https://example.com/docs").unwrap();
    }

    #[test]
    fn validate_public_http_url_blocks_localhost() {
        assert!(validate_public_http_url("http://localhost/admin").is_err());
        assert!(validate_public_http_url("http://127.0.0.1/").is_err());
    }

    #[test]
    fn validate_public_http_url_blocks_private_networks() {
        assert!(validate_public_http_url("http://10.0.0.5/").is_err());
        assert!(validate_public_http_url("http://192.168.1.1/").is_err());
    }

    #[test]
    fn validate_public_http_url_blocks_non_http_schemes() {
        assert!(validate_public_http_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn validate_public_http_url_blocks_credentials() {
        assert!(validate_public_http_url("https://user:pass@example.com/").is_err());
    }

    #[tokio::test]
    async fn validate_public_http_url_resolved_blocks_localhost_dns() {
        let err = validate_public_http_url_resolved("http://localhost/admin").await.unwrap_err().to_string();
        assert!(err.contains("Blocked host"));
    }

    #[test]
    fn encode_query_percent_encodes_reserved_and_unicode() {
        assert_eq!(encode_query("a b"), "a+b");
        assert_eq!(encode_query("a&b=c"), "a%26b%3Dc");
        assert_eq!(encode_query("café"), "caf%C3%A9");
        assert_eq!(encode_query("safe-._~"), "safe-._~");
    }
}
