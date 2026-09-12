//! Error recovery and retry logic.
//!
//! The agent loop uses this module to decide whether a failure is transient
//! (network timeout, rate limit) or permanent (prompt too long).  Transient
//! errors are retried with exponential back-off (see [`backoff_delay`]).
//!
//! - [`CONTINUATION_MESSAGE`]: appended when the LLM hits its output limit
//!   on the first attempt, asking it to pick up mid-response.
//! - [`CONVERGENCE_CONTINUATION_MESSAGE`]: used on repeated truncations to
//!   stop further expansion and request only a concise final result.
//! - [`continuation_message`]: selector that chooses the direct-resume prompt
//!   on attempt 1 and the convergence prompt on later attempts.
//! - [`MAX_COMPACT_ATTEMPTS`]: prompt-too-long compaction retries.
//! - [`MAX_TRANSPORT_ATTEMPTS`]: transient network error retries (higher for
//!   long-running tasks that may encounter multiple intermittent failures).
//! - [`MAX_CONTINUATION_ATTEMPTS`]: max-tokens continuation retries.
//! - [`MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS`]: transient retries during the
//!   compaction summary call itself.
//! - [`RecoveryState`]: tracks attempts across compaction, continuation, and
//!   transport categories.
//! - [`is_prompt_too_long_error`] / [`is_transient_transport_error`]:
//!   classify error strings to route recovery decisions.
//! - [`error_summary`]: collapses an error into a single readable line so
//!   recovery status messages say *why* a retry is happening.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_COMPACT_ATTEMPTS: u32 = 3;
pub const MAX_TRANSPORT_ATTEMPTS: u32 = 10;
pub const MAX_CONTINUATION_ATTEMPTS: u32 = 3;
/// Retries for transient errors during the compaction summary LLM call.
/// Kept smaller than [`MAX_TRANSPORT_ATTEMPTS`] because the summary call is
/// a short one-shot operation — failing after a few retries means the
/// compaction cannot proceed, and the main loop will surface the error.
pub const MAX_COMPACT_SUMMARY_RETRY_ATTEMPTS: u32 = 3;
const BACKOFF_BASE_DELAY_SECS: f64 = 1.0;
const BACKOFF_MAX_DELAY_SECS: f64 = 30.0;

pub const CONTINUATION_MESSAGE: &str = "Output limit hit. Continue directly from where you stopped. \
No recap, no repetition. Pick up mid-sentence if needed.";

pub const CONVERGENCE_CONTINUATION_MESSAGE: &str = "Your response has been truncated repeatedly. Stop expanding the analysis and do not revisit the same scenarios. Return only the final actionable result in a concise structured format: conclusion, verified issues, and minimal fixes. Do not recap, repeat, or speculate.";

/// Returns the continuation prompt appropriate for the current attempt index.
///
/// During continuation recovery, attempt 1 uses the direct-resume prompt. If the model
/// is truncated again, attempts 2 and 3 switch to a convergence prompt that requests
/// only concise actionable conclusions without further expansion.
pub fn continuation_message(attempt: u32) -> &'static str {
    if attempt <= 1 {
        CONTINUATION_MESSAGE
    } else {
        CONVERGENCE_CONTINUATION_MESSAGE
    }
}

/// Current state of retry counters.
///
/// Each counter is scoped to a recovery strategy:
/// - `continuation_attempts`: "output limit" continuations.
/// - `compact_attempts`: context-compaction attempts.
/// - `transport_attempts`: network-level retries.
#[derive(Debug, Default)]
pub struct RecoveryState {
    pub continuation_attempts: u32,
    pub compact_attempts: u32,
    pub transport_attempts: u32,
}

/// Returns `true` if the error string indicates the prompt exceeded the
/// model's context window.
pub fn is_prompt_too_long_error(error_text: &str) -> bool {
    (error_text.contains("prompt") && error_text.contains("long"))
        || error_text.contains("overlong_prompt")
        || error_text.contains("too many tokens")
        || error_text.contains("context length")
}

/// Returns `true` if the error string matches a known transient transport
/// failure pattern (timeout, rate limit, connection reset, etc.).
///
/// Prefer [`is_transient_llm_error`] when the typed error is still available:
/// string matching cannot tell a retryable 503 from a permanent 400, and it
/// depends on the provider's English prose.
pub fn is_transient_transport_error(error_text: &str) -> bool {
    [
        "timeout",
        "timed out",
        "rate limit",
        "too many requests",
        "unavailable",
        "connection",
        "overloaded",
        "temporarily",
        "econnreset",
        "broken pipe",
        "http request failed",
        "error sending request",
    ]
    .iter()
    .any(|needle| error_text.contains(needle))
}

/// Whether an HTTP status code describes a failure worth retrying.
///
/// `429` / `408` (rate limit, request timeout) and every `5xx` are transient;
/// other `4xx` (400 malformed request, 401/403 credential or authorization,
/// 404) are permanent — retrying them only burns the user's quota.
pub fn is_transient_http_status(status: u16) -> bool {
    status == 429 || status == 408 || (500..600).contains(&status)
}

/// Classification of a failed LLM call for the recovery loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureKind {
    /// Prompt exceeded the model's context window; compact and retry.
    PromptTooLong,
    /// Retryable transport/rate-limit failure; back off and retry.
    Transient,
    /// Everything else — surface to the caller.
    Permanent,
}

/// Message-based fallback classification, used for errors whose type carries
/// no status (transport failures arrive as `reqwest` prose).
fn classify_text(error_text: &str) -> FailureKind {
    let lowered = error_text.to_lowercase();
    if is_prompt_too_long_error(&lowered) {
        FailureKind::PromptTooLong
    } else if is_transient_transport_error(&lowered) {
        FailureKind::Transient
    } else {
        FailureKind::Permanent
    }
}

/// Classifies an [`LlmError`](tact_llm::LlmError), preferring its typed fields.
///
/// `HttpError` carries the real HTTP status, so a 429/5xx is retried while a
/// 400/401/403/404 fails fast instead of burning the user's quota. Other
/// variants carry no status and fall back to message matching.
pub fn classify_llm_error(error: &tact_llm::LlmError) -> FailureKind {
    if let tact_llm::LlmError::HttpError { status, .. } = error {
        if is_transient_http_status(*status) {
            return FailureKind::Transient;
        }
        // A non-transient status can still be recoverable: an over-long prompt
        // is usually reported as a 400.
        return match classify_text(&error.to_string()) {
            FailureKind::PromptTooLong => FailureKind::PromptTooLong,
            _ => FailureKind::Permanent,
        };
    }
    classify_text(&error.to_string())
}

/// Classifies a boxed error, downcasting to [`tact_llm::LlmError`] when the
/// typed cause survived (see `Agent::stream_message`).
pub fn classify_error(error: &anyhow::Error) -> FailureKind {
    match error.downcast_ref::<tact_llm::LlmError>() {
        Some(llm_error) => classify_llm_error(llm_error),
        None => classify_text(&error.to_string()),
    }
}

/// Exponential back-off delay with millisecond jitter.
///
/// Formula: `min(1s × 2^attempt, 30s) + random(0..1s)`.
pub fn backoff_delay(attempt: u32) -> Duration {
    let base = (BACKOFF_BASE_DELAY_SECS * 2f64.powi(attempt as i32)).min(BACKOFF_MAX_DELAY_SECS);
    let jitter = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.subsec_millis() % 1000) as f64 / 1000.0)
        .unwrap_or(0.0);
    Duration::from_secs_f64(base + jitter)
}

/// Maximum length of the single-line error summary embedded in recovery
/// status messages. Long chains are truncated so the TUI line stays readable.
const MAX_ERROR_SUMMARY_CHARS: usize = 200;

/// Collapses an error message into a single readable line for recovery
/// status messages: runs of whitespace (including newlines) become single
/// spaces, and overly long messages are truncated with an ellipsis.
pub fn error_summary(error_text: &str) -> String {
    let collapsed: String = error_text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= MAX_ERROR_SUMMARY_CHARS {
        collapsed
    } else {
        let truncated: String = collapsed.chars().take(MAX_ERROR_SUMMARY_CHARS).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn http_error(status: u16, body: &str) -> tact_llm::LlmError {
        tact_llm::LlmError::HttpError {
            status,
            body: body.to_string(),
        }
    }

    #[test]
    fn http_status_classification_retries_only_transient_codes() {
        for status in [408, 429, 500, 502, 503, 504] {
            assert_eq!(
                classify_llm_error(&http_error(status, "busy")),
                FailureKind::Transient,
                "{status} should be retryable"
            );
        }
    }

    /// The whole point of the typed classification: a permanent client error
    /// must not be retried just because its body happens to read like a
    /// transient one.
    #[test]
    fn permanent_client_errors_are_not_retried() {
        for status in [400, 401, 403, 404, 422] {
            assert_eq!(
                classify_llm_error(&http_error(status, "rate limit exceeded")),
                FailureKind::Permanent,
                "{status} must not be retried"
            );
        }
    }

    /// An over-long prompt is usually a 400, and the recovery loop fixes it by
    /// compacting rather than retrying verbatim.
    #[test]
    fn overlong_prompt_on_a_400_is_recoverable() {
        assert_eq!(
            classify_llm_error(&http_error(400, "context length exceeded")),
            FailureKind::PromptTooLong
        );
    }

    #[test]
    fn transport_textual_errors_still_classify() {
        assert_eq!(
            classify_llm_error(&tact_llm::LlmError::Request("connection reset".into())),
            FailureKind::Transient
        );
        assert_eq!(
            classify_llm_error(&tact_llm::LlmError::Auth("bad key".into())),
            FailureKind::Permanent
        );
    }

    /// `Agent::stream_message` preserves the typed error, so the downcast path
    /// is what the recovery loop actually exercises.
    #[test]
    fn boxed_errors_are_classified_through_the_typed_cause() {
        let boxed = anyhow::Error::from(http_error(429, "slow down"));
        assert_eq!(classify_error(&boxed), FailureKind::Transient);

        let stringified = anyhow::anyhow!("api error (429): slow down");
        assert_eq!(
            classify_error(&stringified),
            FailureKind::Permanent,
            "without the type there is nothing to distinguish a 429 from prose"
        );
    }

    #[test]
    fn is_transient_matches_http_request_failed() {
        assert!(is_transient_transport_error(
            "unsupported response state: HTTP request failed: error sending request for url"
        ));
    }

    #[test]
    fn is_transient_matches_timeout() {
        assert!(is_transient_transport_error("request timed out"));
    }

    #[test]
    fn is_transient_matches_econnreset() {
        assert!(is_transient_transport_error("econnreset"));
    }

    #[test]
    fn is_transient_rejects_prompt_too_long() {
        assert!(!is_transient_transport_error("prompt too long"));
    }

    #[test]
    fn first_continuation_preserves_direct_resume_prompt() {
        assert_eq!(continuation_message(0), CONTINUATION_MESSAGE);
        assert_eq!(continuation_message(1), CONTINUATION_MESSAGE);
    }

    #[test]
    fn repeated_continuations_switch_to_convergence_prompt() {
        assert_eq!(continuation_message(2), CONVERGENCE_CONTINUATION_MESSAGE);
        assert_eq!(continuation_message(3), CONVERGENCE_CONTINUATION_MESSAGE);
        assert_eq!(continuation_message(99), CONVERGENCE_CONTINUATION_MESSAGE);
    }

    #[test]
    fn convergence_prompt_requires_concise_actionable_output() {
        let prompt = CONVERGENCE_CONTINUATION_MESSAGE.to_ascii_lowercase();
        assert!(prompt.contains("stop"));
        assert!(prompt.contains("concise"));
        assert!(prompt.contains("actionable"));
        assert!(prompt.contains("repeat"));
    }

    #[test]
    fn error_summary_preserves_short_errors() {
        assert_eq!(error_summary("request timed out"), "request timed out");
    }

    #[test]
    fn error_summary_collapses_whitespace_and_truncates() {
        let long = format!("line one\nline two   line three{}", " x".repeat(300));
        let summary = error_summary(&long);
        assert!(!summary.contains('\n'));
        assert!(!summary.contains("  "));
        assert_eq!(summary.chars().count(), MAX_ERROR_SUMMARY_CHARS + 1);
        assert!(summary.ends_with('…'));
    }

    #[test]
    fn error_summary_keeps_chain_separators() {
        let summary = error_summary("http request failed: error sending request for url");
        assert_eq!(
            summary,
            "http request failed: error sending request for url"
        );
    }
}
