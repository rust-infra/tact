//! Shared HTTP utilities for web tools.

use std::net::IpAddr;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::{Context, Result};
use reqwest::Url;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub fn http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .expect("failed to build shared HTTP client")
    })
}

/// Validates that a URL is a safe public http(s) target for outbound fetches.
pub fn validate_public_http_url(raw: &str) -> Result<Url> {
    let url = Url::parse(raw.trim()).context("Invalid URL")?;

    match url.scheme() {
        "http" | "https" => {}
        scheme => anyhow::bail!("Only http and https URLs are allowed, got {scheme}"),
    }

    if url.username() != "" || url.password().is_some() {
        anyhow::bail!("URLs with embedded credentials are not allowed");
    }

    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL must have a host"))?;

    if is_blocked_host(host) {
        anyhow::bail!("Blocked host: {host}");
    }

    Ok(url)
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
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
    }
}

pub fn encode_query(value: &str) -> String {
    let mut encoded = String::new();
    for ch in value.chars() {
        match ch {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => encoded.push(ch),
            ' ' => encoded.push('+'),
            _ => {
                for byte in ch.to_string().as_bytes() {
                    encoded.push_str(&format!("%{:02X}", byte));
                }
            }
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
}
