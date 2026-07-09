// Agent core module
// Receives user tasks, calls the OpenAI API to generate execution plans,
// and executes them step by step inside a sandbox.
// Communicates with the TUI module over channels, reporting execution status in real time.

pub mod agent;
pub mod biz;

pub use agent::{
    AgentErrorKind, AgentUpdate, ModelCallParams, PlanStep, StepResult, StepStatus, TokenUsageInfo,
    UserCommand,
};
pub use biz::{BalanceEntry, BalanceInfo, UsageQuotaInfo, UsageQuotaWindow};

// Format a byte count using human-readable units: Byte, K, M, G.
//
// Uses 1024 as the unit base. Values below 1024 are shown as
// `"<n> Byte"`; larger values are scaled to the largest fitting unit
// with one decimal place and trailing ".0" removed.
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["Byte", "K", "M", "G"];

    if bytes < 1024 {
        return format!("{} Byte", bytes);
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
        assert_eq!(format_bytes(0), "0 Byte");
        assert_eq!(format_bytes(1023), "1023 Byte");
        assert_eq!(format_bytes(1024), "1 K");
        assert_eq!(format_bytes(1536), "1.5 K");
        assert_eq!(format_bytes(1024 * 1024), "1 M");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 G");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 3), "3 G");
    }

    #[test]
    fn usage_pct_basic() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: "100".to_string(),
            remaining: "42".to_string(),
            reset_time: None,
        };
        assert!((w.usage_pct().unwrap() - 58.0).abs() < 1e-9);
    }

    #[test]
    fn usage_pct_zero_limit() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: "0".to_string(),
            remaining: "0".to_string(),
            reset_time: None,
        };
        assert_eq!(w.usage_pct(), None);
    }

    #[test]
    fn usage_pct_unparseable() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: "unlimited".to_string(),
            remaining: "42".to_string(),
            reset_time: None,
        };
        assert_eq!(w.usage_pct(), None);
    }

    #[test]
    fn usage_pct_caps_at_100() {
        let w = UsageQuotaWindow {
            label: "week".to_string(),
            limit: "100".to_string(),
            remaining: "-10".to_string(),
            reset_time: None,
        };
        assert!((w.usage_pct().unwrap() - 100.0).abs() < f64::EPSILON);
    }
}
