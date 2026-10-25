//! Shared protocol types between the agent runtime and the TUI.
//!
//! [`agent`] defines the runtime messages ([`AgentUpdate`] / [`UserCommand`])
//! exchanged over channels; [`biz`] defines account / quota structures carried
//! inside those messages.
//!
//! State machine transitions: [book/25_chapter_protocol_zh.html](../book/25_chapter_protocol_zh.html).

pub mod agent;
pub mod biz;
/// A command the view sends to the voice worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VoiceCommand {
    Start,
    Stop,
    Cancel,
    Shutdown,
}

/// What the voice worker reports back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEvent {
    RecordingStarted,
    RecordingStopped,
    Transcribing,
    Transcript(String),
    Error(String),
    Cancelled,
}

pub mod context_markers;
pub mod events;
pub mod marketplace;
pub mod reasoning;
pub mod subagent_transcript;
pub mod tool_output;

pub use agent::{
    AgentErrorKind, AgentUpdate, GoalAcceptanceSnapshot, GoalCriterionSnapshot,
    GoalCriterionStatusSnapshot, GoalPhaseSnapshot, GoalRunBudgetSnapshot, GoalSnapshot,
    GoalStatusSnapshot, ModelCallParams, PlanStep, StepResult, StepStatus, SubagentRunSnapshot,
    SubagentStatusSnapshot, TaskSnapshot, TaskStatusSnapshot, TasksChangeReason, ThinkingChunk,
    TokenUsageInfo, ToolDetailKind, ToolPopupKind, ToolPresentationInfo, ToolVisualKind,
    UiResponse, UserCommand,
};
pub use biz::{
    AccountError, AccountUpdate, BalanceEntry, BalanceInfo, UsageQuotaInfo, UsageQuotaWindow,
};
pub use context_markers::{
    GOAL_CONTEXT_CLOSE_TAG, GOAL_CONTEXT_OPEN_TAG, HOOK_CONTEXT_CLOSE_TAG, HOOK_CONTEXT_OPEN_TAG,
    frame_hook_context, hook_context_body, hook_context_source, is_goal_context_text,
    is_hook_context_text,
};
pub use events::UpdateSink;
pub use marketplace::{
    InstalledPlugin, MarketplaceRecord, MarketplaceSource, PluginEvent, PluginOperation,
    PluginRequest, PluginResult, marketplace_catalog_path,
};
pub use reasoning::ReasoningEffort;
pub use subagent_transcript::{SubagentStep, SubagentStepStatus, SubagentTranscriptEntry};
pub use tool_output::{
    ToolOutputBuffer, ToolOutputChunk, ToolOutputLine, ToolOutputSpan, ToolOutputStream,
};

/// The most characters one user input may carry.
///
/// Both sides need this number: the runtime enforces it and the view reports it
/// in the "input too long" message. It lives here so neither has to depend on
/// the other for a constant.
pub const MAX_INPUT_CHARS: usize = 500_000;

/// Whether `char_count` exceeds [`MAX_INPUT_CHARS`].
///
/// A function rather than an inline comparison so the boundary is stated once:
/// the limit is inclusive, and an off-by-one here silently rejects a valid
/// paste or accepts one the runtime will truncate.
#[inline]
#[must_use]
pub fn exceeds_input_char_limit(char_count: usize) -> bool {
    char_count > MAX_INPUT_CHARS
}

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
    use super::{MAX_INPUT_CHARS, UsageQuotaWindow, exceeds_input_char_limit, format_bytes};

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

    /// The limit is inclusive: exactly `MAX_INPUT_CHARS` is allowed.
    ///
    /// The view rejects a paste on this predicate and reports the number, so an
    /// off-by-one here would either reject a valid input or accept one the
    /// runtime truncates.
    #[test]
    fn the_input_limit_is_inclusive() {
        assert!(!exceeds_input_char_limit(0));
        assert!(!exceeds_input_char_limit(MAX_INPUT_CHARS));
        assert!(exceeds_input_char_limit(MAX_INPUT_CHARS + 1));
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
