//! OpenCode Go (`opencode.ai/zen`) endpoint identification and session header.
//!
//! OpenCode's hosted coding endpoint asks clients to send an
//! `x-opencode-session` header on every request so it can correlate and
//! optimize the session. Requests without it (and without a recognizable
//! `User-Agent`) are reported as "Unknown client" and will start erroring.
//!
//! This module:
//! - detects whether a `base_url` is an OpenCode Go endpoint;
//! - supplies an `x-opencode-session` token that is **stable per process and
//!   per base URL**, so all requests from one run share the same session
//!   (`TACT_OPENCODE_SESSION` overrides it when set);
//! - sends `tact/<version>` as the `User-Agent` so OpenCode can identify the
//!   tool instead of "Unknown client".

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use reqwest13::header::HeaderMap;

/// Header OpenCode Go uses to correlate a client session.
pub const X_OPENCODE_SESSION: &str = "x-opencode-session";

/// Environment override: pin the session token (e.g. to resume a prior
/// session) instead of the auto-generated per-process value.
pub const SESSION_ENV: &str = "TACT_OPENCODE_SESSION";

/// Tool identifier sent as `User-Agent` on OpenCode endpoints.
pub const USER_AGENT: &str = concat!("tact/", env!("CARGO_PKG_VERSION"));

/// Returns `true` when `base_url` points at OpenCode Go (`opencode.ai` or a
/// subdomain such as `app.opencode.ai`).
pub fn is_opencode_base_url(base_url: &str) -> bool {
    let Some(host) = reqwest13::Url::parse(base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
    else {
        return false;
    };
    host == "opencode.ai" || host.ends_with(".opencode.ai")
}

/// Headers to attach to every request for `base_url`. Empty unless the
/// endpoint is OpenCode Go.
pub fn endpoint_headers(base_url: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if !is_opencode_base_url(base_url) {
        return headers;
    }
    if let Ok(ua) = USER_AGENT.parse() {
        headers.insert(reqwest13::header::USER_AGENT, ua);
    }
    if let Ok(session) = session_token(base_url).parse() {
        headers.insert(X_OPENCODE_SESSION, session);
    }
    headers
}

static SESSION_TOKENS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

/// Stable per-process session token for an OpenCode base URL. One token per
/// endpoint so the service can correlate the whole run; different endpoints
/// get different tokens.
pub fn session_token(base_url: &str) -> String {
    if let Ok(override_value) = std::env::var(SESSION_ENV)
        && !override_value.is_empty()
    {
        return override_value;
    }
    let mut tokens = SESSION_TOKENS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    tokens
        .entry(base_url.to_string())
        .or_insert_with(new_session_token)
        .clone()
}

fn new_session_token() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    format!(
        "tact-{:x}-{:x}-{:x}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_opencode_base_urls() {
        for url in [
            "https://opencode.ai/zen/go/v1",
            "https://opencode.ai",
            "https://opencode.ai/v1/",
            "https://app.opencode.ai/zen/go/v1",
            "http://opencode.ai/zen",
        ] {
            assert!(is_opencode_base_url(url), "expected opencode: {url}");
        }
        for url in [
            "https://api.openai.com/v1",
            "https://api.deepseek.com/v1",
            "https://api.kimi.com/coding",
            "",
            "not-a-url",
            "https://opencode.ai.evil.example/v1",
            "https://notopencode.ai/v1",
        ] {
            assert!(!is_opencode_base_url(url), "expected non-opencode: {url}");
        }
    }

    #[test]
    fn endpoint_headers_only_attach_to_opencode() {
        let headers = endpoint_headers("https://opencode.ai/zen/go/v1");
        assert!(
            headers.contains_key(X_OPENCODE_SESSION),
            "must carry x-opencode-session"
        );
        let ua = headers
            .get(reqwest13::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(ua.starts_with("tact/"), "user agent identifies tact: {ua}");

        let non_opencode = endpoint_headers("https://api.openai.com/v1");
        assert!(non_opencode.is_empty(), "must not add headers elsewhere");
    }

    #[test]
    fn session_token_is_stable_per_base_url() {
        let a1 = session_token("https://opencode.ai/zen/go/v1");
        let a2 = session_token("https://opencode.ai/zen/go/v1");
        let b = session_token("https://opencode.ai/zen/go/v2");
        assert_eq!(a1, a2, "same base url must reuse the token");
        assert_ne!(a1, b, "different base urls get different tokens");
        assert!(!a1.is_empty());
    }
}
