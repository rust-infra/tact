//! Error recovery and retry logic.
//!
//! The agent loop uses this module to decide whether a failure is transient
//! (network timeout, rate limit) or permanent (prompt too long).  Transient
//! errors are retried with exponential back-off (see [`backoff_delay`]).
//!
//! - [`CONTINUATION_MESSAGE`]: appended when the LLM hits its output limit,
//!   asking it to pick up mid-response.
//! - [`MAX_RECOVERY_ATTEMPTS`]: total number of retries before giving up.
//! - [`RecoveryState`]: tracks attempts across compaction, continuation, and
//!   transport categories.
//! - [`is_prompt_too_long_error`] / [`is_transient_transport_error`]:
//!   classify error strings to route recovery decisions.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const MAX_RECOVERY_ATTEMPTS: u32 = 3;
const BACKOFF_BASE_DELAY_SECS: f64 = 1.0;
const BACKOFF_MAX_DELAY_SECS: f64 = 30.0;

pub const CONTINUATION_MESSAGE: &str = "Output limit hit. Continue directly from where you stopped. \
No recap, no repetition. Pick up mid-sentence if needed.";

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
