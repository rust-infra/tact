//! OpenCode Go (`opencode.ai/zen`) endpoint identification and session header.
//!
//! OpenCode's hosted coding endpoint asks clients to send an
//! `x-opencode-session` header on every request so it can correlate and
//! optimize the session (routing + prompt caching). Requests without it (and
//! without a recognizable `User-Agent`) are reported as "Unknown client" and
//! may error. The published client requirements re-introduced the header for
//! OpenCode Go (`https://opencode.ai/zen/go/v1`) on 2026-09-07, after a
//! window where it was optional.
//!
//! This module:
//! - detects whether a `base_url` is an OpenCode Go endpoint;
//! - fills `x-opencode-session` from the caller-supplied session id (the Tact
//!   session id), which OpenCode uses as the key that distinguishes its
//!   per-conversation caches — no synthetic token, no env override;
//! - sends `tact/<version>` as the `User-Agent` so OpenCode can identify the
//!   tool instead of "Unknown client".

use reqwest13::header::HeaderMap;

/// Header OpenCode Go uses to correlate a client session.
pub const X_OPENCODE_SESSION: &str = "x-opencode-session";

/// Tool identifier sent as `User-Agent` on OpenCode endpoints.
pub const USER_AGENT: &str = concat!("tact/", env!("CARGO_PKG_VERSION"));

/// Returns `true` when `base_url` points at OpenCode Go (`opencode.ai` or a
/// subdomain such as `app.opencode.ai`). The canonical OpenCode Go base URL
/// is `https://opencode.ai/zen/go/v1`.
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
///
/// `session_id` is the Tact session id: OpenCode treats `x-opencode-session`
/// as the session key that distinguishes its per-conversation caches, so one
/// conversation must reuse the same value and different conversations must
/// differ. The header is filled from `session_id` only — requests without a
/// session (e.g. the `/v1/models` picker fetch) omit it and send just the
/// `User-Agent`.
pub fn endpoint_headers(base_url: &str, session_id: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if !is_opencode_base_url(base_url) {
        return headers;
    }
    if let Ok(ua) = USER_AGENT.parse() {
        headers.insert(reqwest13::header::USER_AGENT, ua);
    }
    if let Some(session) = session_id.filter(|s| !s.is_empty())
        && let Ok(session) = session.parse()
    {
        headers.insert(X_OPENCODE_SESSION, session);
    }
    headers
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
        let headers = endpoint_headers("https://opencode.ai/zen/go/v1", Some("sess-1"));
        assert!(
            headers.contains_key(X_OPENCODE_SESSION),
            "must carry x-opencode-session"
        );
        assert_eq!(
            headers
                .get(X_OPENCODE_SESSION)
                .and_then(|v| v.to_str().ok()),
            Some("sess-1"),
            "the session id must be used verbatim when supplied"
        );
        let ua = headers
            .get(reqwest13::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(ua.starts_with("tact/"), "user agent identifies tact: {ua}");

        let non_opencode = endpoint_headers("https://api.openai.com/v1", Some("sess-1"));
        assert!(non_opencode.is_empty(), "must not add headers elsewhere");
    }

    #[test]
    fn session_header_is_filled_from_session_id_only() {
        // With a session id the header equals it.
        let with_session = endpoint_headers("https://opencode.ai/zen/go/v1", Some("sess-2"));
        assert_eq!(
            with_session
                .get(X_OPENCODE_SESSION)
                .and_then(|v| v.to_str().ok()),
            Some("sess-2")
        );

        // Without a session id no synthetic token is invented; only the
        // identifying User-Agent is sent.
        let without_session = endpoint_headers("https://opencode.ai/zen/go/v1", None);
        assert!(
            without_session.get(X_OPENCODE_SESSION).is_none(),
            "no session header without a session id"
        );
        let ua = without_session
            .get(reqwest13::header::USER_AGENT)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            ua.starts_with("tact/"),
            "user agent still identifies tact: {ua}"
        );

        // An empty session id is treated the same as no session.
        let empty_session = endpoint_headers("https://opencode.ai/zen/go/v1", Some(""));
        assert!(empty_session.get(X_OPENCODE_SESSION).is_none());
    }
}
