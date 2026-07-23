//! Tool/exec output truncation by [`TruncationPolicy`] (ported from Codex
//! `codex-rs/utils/output-truncation` + `TruncationPolicy` in protocol).

use std::ops::Mul;

use serde::{Deserialize, Serialize};

use super::truncate::{
    approx_bytes_for_tokens, approx_token_count, approx_tokens_from_byte_count,
    truncate_middle_chars, truncate_middle_with_token_budget,
};

/// Budget mode for truncating tool / exec output kept in model-visible history.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "mode", content = "limit", rename_all = "snake_case")]
pub enum TruncationPolicy {
    Bytes(usize),
    Tokens(usize),
}

impl TruncationPolicy {
    /// Token budget derived from this policy.
    ///
    /// - [`Tokens`]: the explicit limit.
    /// - [`Bytes`]: approximate tokens via `ceil(bytes / 4)`.
    pub fn token_budget(&self) -> usize {
        match self {
            Self::Bytes(bytes) => {
                usize::try_from(approx_tokens_from_byte_count(*bytes)).unwrap_or(usize::MAX)
            }
            Self::Tokens(tokens) => *tokens,
        }
    }

    /// Byte budget derived from this policy.
    ///
    /// - [`Bytes`]: the explicit limit.
    /// - [`Tokens`]: approximate bytes via `tokens * 4`.
    pub fn byte_budget(&self) -> usize {
        match self {
            Self::Bytes(bytes) => *bytes,
            Self::Tokens(tokens) => approx_bytes_for_tokens(*tokens),
        }
    }
}

impl Mul<f64> for TruncationPolicy {
    type Output = Self;

    fn mul(self, multiplier: f64) -> Self::Output {
        match self {
            Self::Bytes(bytes) => Self::Bytes((bytes as f64 * multiplier).ceil() as usize),
            Self::Tokens(tokens) => Self::Tokens((tokens as f64 * multiplier).ceil() as usize),
        }
    }
}

/// Truncate `content` according to `policy` (head + middle marker + tail).
pub fn truncate_text(content: &str, policy: TruncationPolicy) -> String {
    match policy {
        TruncationPolicy::Bytes(bytes) => truncate_middle_chars(content, bytes),
        TruncationPolicy::Tokens(tokens) => truncate_middle_with_token_budget(content, tokens).0,
    }
}

/// Like [`truncate_text`], but prefixes a warning with original token/line counts
/// when truncation occurs.
pub fn formatted_truncate_text(content: &str, policy: TruncationPolicy) -> String {
    if content.len() <= policy.byte_budget() {
        return content.to_string();
    }

    let original_token_count = approx_token_count(content);
    let total_lines = content.lines().count();
    let result = truncate_text(content, policy);
    format!(
        "Warning: truncated output (original token count: {original_token_count})\nTotal output lines: {total_lines}\n\n{result}"
    )
}

/// Convert a byte count into an approximate token count as `i64`.
pub fn approx_tokens_from_byte_count_i64(bytes: i64) -> i64 {
    if bytes <= 0 {
        return 0;
    }

    let bytes = usize::try_from(bytes).unwrap_or(usize::MAX);
    i64::try_from(approx_tokens_from_byte_count(bytes)).unwrap_or(i64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_budget_conversions() {
        let bytes = TruncationPolicy::Bytes(10_000);
        assert_eq!(bytes.byte_budget(), 10_000);
        assert_eq!(bytes.token_budget(), 2_500);

        let tokens = TruncationPolicy::Tokens(10_000);
        assert_eq!(tokens.token_budget(), 10_000);
        assert_eq!(tokens.byte_budget(), 40_000);
    }

    #[test]
    fn policy_mul_scales_ceil() {
        let scaled = TruncationPolicy::Tokens(10) * 1.2;
        assert_eq!(scaled, TruncationPolicy::Tokens(12));

        let scaled_bytes = TruncationPolicy::Bytes(10) * 1.2;
        assert_eq!(scaled_bytes, TruncationPolicy::Bytes(12));
    }

    #[test]
    fn truncate_text_bytes_mode() {
        let out = truncate_text("abcdefghij", TruncationPolicy::Bytes(6));
        assert_eq!(out, "abc…4 chars truncated…hij");
    }

    #[test]
    fn formatted_truncate_text_under_budget_unchanged() {
        let text = "short";
        let out = formatted_truncate_text(text, TruncationPolicy::Bytes(100));
        assert_eq!(out, text);
    }

    #[test]
    fn formatted_truncate_text_over_budget_adds_warning() {
        let text = "a".repeat(200);
        let out = formatted_truncate_text(&text, TruncationPolicy::Bytes(40));
        assert!(out.starts_with("Warning: truncated output"));
        assert!(out.contains("Total output lines:"));
        assert!(out.contains("chars truncated"));
    }

    #[test]
    fn truncate_text_chinese_bytes_mode() {
        let out = truncate_text("一二三四五六七八九十", TruncationPolicy::Bytes(12));
        assert_eq!(out, "一二…6 chars truncated…九十");
    }

    #[test]
    fn truncate_text_chinese_tokens_mode() {
        let out = truncate_text("一二三四五六七八九十", TruncationPolicy::Tokens(3));
        assert_eq!(out, "一二…5 tokens truncated…九十");
    }

    #[test]
    fn formatted_truncate_text_chinese_warning() {
        let text = "中文输出内容偏长需要截断处理一二三四五六七八九十";
        let out = formatted_truncate_text(text, TruncationPolicy::Bytes(24));
        assert!(out.starts_with("Warning: truncated output"), "{out}");
        assert!(out.contains("Total output lines: 1"), "{out}");
        assert!(out.contains("chars truncated"), "{out}");
        assert!(out.contains("中文") || out.contains("九十") || out.contains("处理"), "{out}");
    }
}
