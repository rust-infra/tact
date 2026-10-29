//! Shared HTTP transport for provider and account adapters.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, SystemTime};

use crate::error::HttpErrorClass;

/// Upper bound for a provider error body retained for classification.
pub(crate) const MAX_HTTP_ERROR_BODY_BYTES: usize = 4 * 1024;
/// Upper bound for a successful account/discovery JSON response.
pub(crate) const MAX_HTTP_SUCCESS_BODY_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedReadError {
    Transport,
    TooLarge,
}

fn parse_retry_after(value: &str) -> Option<Duration> {
    if let Ok(seconds) = value.trim().parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let when = httpdate::parse_http_date(value).ok()?;
    when.duration_since(SystemTime::now()).ok()
}

pub(crate) fn retry_after_13(headers: &reqwest13::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest13::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
}

pub(crate) fn classify_http_error(status: u16, body: &str) -> HttpErrorClass {
    if status == 401 || status == 403 {
        return HttpErrorClass::Authentication;
    }
    let lowered = body.to_ascii_lowercase();
    if lowered.contains("context length")
        || lowered.contains("context window")
        || lowered.contains("too many tokens")
        || lowered.contains("overlong_prompt")
        || lowered.contains("prompt") && lowered.contains("long")
    {
        return HttpErrorClass::ContextOverflow;
    }
    if status == 408 || status == 429 || (500..600).contains(&status) {
        return HttpErrorClass::Transient;
    }
    HttpErrorClass::Permanent
}

#[cfg(feature = "anthropic")]
pub(crate) fn retry_after_12(headers: &reqwest::header::HeaderMap) -> Option<Duration> {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(parse_retry_after)
}

pub(crate) async fn read_bounded_error_body_13(mut response: reqwest13::Response) -> String {
    let mut body = Vec::with_capacity(MAX_HTTP_ERROR_BODY_BYTES);
    while body.len() < MAX_HTTP_ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_HTTP_ERROR_BODY_BYTES - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

pub(crate) async fn read_bounded_success_body_13(
    mut response: reqwest13::Response,
) -> Result<String, BoundedReadError> {
    let mut body = Vec::with_capacity(MAX_HTTP_SUCCESS_BODY_BYTES);
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| BoundedReadError::Transport)?
    {
        if body.len().saturating_add(chunk.len()) > MAX_HTTP_SUCCESS_BODY_BYTES {
            return Err(BoundedReadError::TooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    String::from_utf8(body).map_err(|_| BoundedReadError::Transport)
}

#[cfg(feature = "anthropic")]
pub(crate) async fn read_bounded_error_body_12(mut response: reqwest::Response) -> String {
    let mut body = Vec::with_capacity(MAX_HTTP_ERROR_BODY_BYTES);
    while body.len() < MAX_HTTP_ERROR_BODY_BYTES {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let remaining = MAX_HTTP_ERROR_BODY_BYTES - body.len();
                body.extend_from_slice(&chunk[..chunk.len().min(remaining)]);
            }
            Ok(None) | Err(_) => break,
        }
    }
    String::from_utf8_lossy(&body).into_owned()
}

/// Shared [`reqwest`] client with the project's standard read timeout.
///
/// Wrapping the client in a newtype keeps connection-pool reuse explicit at
/// every call site without forcing each adapter to build its own client.
#[derive(Debug, Clone)]
pub struct SharedHttpClient(Arc<reqwest13::Client>);

impl SharedHttpClient {
    /// Build a client with a 120-second read timeout (matching the previous
    /// per-adapter defaults).
    pub fn try_new() -> Result<Self, reqwest13::Error> {
        let client = reqwest13::Client::builder()
            .read_timeout(Duration::from_secs(120))
            .build()?;
        Ok(Self(Arc::new(client)))
    }

    /// Borrow the underlying [`reqwest::Client`].
    pub fn inner(&self) -> &reqwest13::Client {
        &self.0
    }
}

impl Default for SharedHttpClient {
    fn default() -> Self {
        static CLIENT: OnceLock<SharedHttpClient> = OnceLock::new();
        CLIENT
            .get_or_init(|| {
                SharedHttpClient::try_new().expect("failed to build shared reqwest client")
            })
            .clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(parse_retry_after("12"), Some(Duration::from_secs(12)));
        let when = SystemTime::now() + Duration::from_secs(30);
        let parsed = parse_retry_after(&httpdate::fmt_http_date(when)).unwrap();
        assert!(parsed <= Duration::from_secs(30));
        assert!(parsed >= Duration::from_secs(25));
        assert_eq!(parse_retry_after("not-a-date"), None);
    }
}
