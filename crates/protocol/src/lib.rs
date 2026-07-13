//! Shared protocol types between the agent runtime and the TUI.
//!
//! [`agent`] defines the runtime messages ([`AgentUpdate`] / [`UserCommand`])
//! exchanged over channels; [`biz`] defines account / quota structures carried
//! inside those messages.
//!
//! State machine transitions: [book/25_chapter_protocol.md](../book/25_chapter_protocol.md).

pub mod agent;
pub mod biz;

pub use agent::{
    AgentErrorKind, AgentUpdate, ModelCallParams, PlanStep, StepResult, StepStatus, ThinkingChunk,
    TokenUsageInfo, UserCommand,
};
pub use biz::{
    AccountError, AccountUpdate, BalanceEntry, BalanceInfo, UsageQuotaInfo, UsageQuotaWindow,
};

/// Format a byte count using human-readable units: B, KB, MB, GB.
///
/// Uses 1024 as the unit base. Values below 1024 are shown as
/// `"<n> B"`; larger values are scaled to the largest fitting unit
/// with one decimal place and trailing ".0" removed.
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];

    if bytes < 1024 {
        return format!("{} B", bytes);
    }

    let mut size = bytes as f64;
    let mut unit_index = 0;
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    let formatted = format!("{:.1}", size);
    if formatted.ends_with(".0") {
        format!(
            "{} {}",
            &formatted[..formatted.len() - 2],
            UNITS[unit_index]
        )
    } else {
        format!("{} {}", formatted, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::{UsageQuotaWindow, format_bytes};

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0 B");
        assert_eq!(format_bytes(1023), "1023 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(1024 * 1024), "1 MB");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 GB");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 3), "3 GB");
    }

    #[test]
    fn usage_pct_basic() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: Some(100.0),
            remaining: Some(42.0),
            reset_time: None,
        };
        assert!((w.usage_pct().unwrap() - 58.0).abs() < 1e-9);
    }

    #[test]
    fn usage_pct_zero_limit() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: Some(0.0),
            remaining: Some(0.0),
            reset_time: None,
        };
        assert_eq!(w.usage_pct(), None);
    }

    #[test]
    fn usage_pct_unlimited_window() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: None,
            remaining: Some(42.0),
            reset_time: None,
        };
        assert_eq!(w.usage_pct(), None);
        assert!(w.has_remaining());
    }

    #[test]
    fn usage_pct_caps_at_100() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: Some(100.0),
            remaining: Some(-10.0),
            reset_time: None,
        };
        assert!((w.usage_pct().unwrap() - 100.0).abs() < f64::EPSILON);
        assert!(!w.has_remaining());
    }
}
