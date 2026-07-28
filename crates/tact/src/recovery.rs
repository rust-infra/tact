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

#[cfg(test)]
mod tests {
    use super::*;

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
}
