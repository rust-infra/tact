use std::time::Duration;

pub const MAX_TRANSPORT_ATTEMPTS: u32 = 10;
pub const MAX_CONTINUATION_ATTEMPTS: u32 = 3;

pub const CONTINUATION_MESSAGE: &str = "Output limit hit. Continue directly from where you stopped. No recap, no repetition. Pick up mid-sentence if needed.";
pub const CONVERGENCE_CONTINUATION_MESSAGE: &str = "Your response has been truncated repeatedly. Stop expanding the analysis and do not revisit the same scenarios. Return only the final actionable result in a concise structured format: conclusion, verified issues, and minimal fixes. Do not recap, repeat, or speculate.";

pub fn continuation_message(attempt: u32) -> &'static str {
    if attempt <= 1 {
        CONTINUATION_MESSAGE
    } else {
        CONVERGENCE_CONTINUATION_MESSAGE
    }
}

pub fn backoff_delay(attempt: u32) -> Duration {
    let seconds = (1u64 << attempt.min(5)).min(30);
    Duration::from_secs(seconds)
}
