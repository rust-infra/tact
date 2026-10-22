//! Turning agent events into a headless run's stderr log.
//!
//! A headless run has no frame to draw on and no one to prompt, so its only
//! evidence of what happened is what it prints. Before this module existed it
//! printed nothing between "session started" and the final stats: a run that
//! spent twenty minutes retrying a gateway looked exactly like a run that was
//! thinking hard, and a killed run left an empty log behind.
//!
//! The split is deliberate: [`format_event`] is a pure function of one
//! [`AgentUpdate`] and returns `Option<String>`, so the format is testable
//! without a process, and the caller owns the writing (and therefore flushing).
//!
//! What is deliberately *not* logged:
//! - [`AgentUpdate::StreamChunk`]: the final answer is already written to
//!   stdout, and echoing every delta would double it.
//! - [`AgentUpdate::ThinkingChunk`]: reasoning text is not a progress signal.
//! - [`AgentUpdate::RequestSelect`] and friends: headless cannot answer them
//!   (see [`crate::headless`]), so they must not be logged as if it could.

use tact_protocol::{AgentUpdate, StepResult, StepStatus, ThinkingChunk};

/// Longest single-line field this module will print before truncating.
///
/// A tool argument summary can be a whole file; a log line has to stay a line.
const MAX_FIELD_CHARS: usize = 240;

/// Render one event as a single log line, or `None` when it is not worth one.
pub fn format_event(update: &AgentUpdate) -> Option<String> {
    match update {
        AgentUpdate::StepStarted {
            tool_name,
            arg_summary,
            ..
        } => Some(format!(
            "[tool] {tool_name} {}",
            truncate(arg_summary, MAX_FIELD_CHARS)
        )),

        AgentUpdate::StepFinished { result, .. } => Some(format_step_result(result)),

        AgentUpdate::StepFailed { error, .. } => Some(format!(
            "[tool failed] {}",
            truncate(error, MAX_FIELD_CHARS)
        )),

        AgentUpdate::Info(message) => {
            Some(format!("[info] {}", truncate(message, MAX_FIELD_CHARS)))
        }

        AgentUpdate::Error(kind) => Some(format!("[error] {kind}")),

        AgentUpdate::TaskComplete(_) => Some("[done] final answer written to stdout".to_string()),

        AgentUpdate::TaskCancelled => Some("[cancelled] the run was cancelled".to_string()),

        AgentUpdate::TurnStats {
            turns_taken,
            max_turns,
        } => Some(match max_turns {
            Some(max) => format!("[turn] {turns_taken}/{max}"),
            None => format!("[turn] {turns_taken}"),
        }),

        AgentUpdate::TokenUsage(usage) => Some(format!(
            "[tokens] prompt={} completion={} total={}",
            usage.prompt, usage.completion, usage.total
        )),

        AgentUpdate::ModelInfo(params) => Some(format!(
            "[model] {} max_tokens={}",
            params.model, params.max_tokens
        )),

        // The remaining variants are UI surface: cards, popups, task/goal
        // snapshots, subagent transcripts. They describe how a TUI would draw
        // the run, not what the run did.
        _ => None,
    }
}

/// One line for a finished tool call, keeping the failure visible.
fn format_step_result(result: &StepResult) -> String {
    let status = match result.status {
        StepStatus::Success => "ok",
        StepStatus::Failed => "failed",
    };
    let mut line = format!("[tool {status}] {} {}", result.tool, result.message);
    if let Some(duration_us) = result.duration_us {
        line.push_str(&format!(" ({}ms)", duration_us / 1000));
    }
    truncate(&line, MAX_FIELD_CHARS)
}

/// Collapse to one line and cap the length.
///
/// Newlines matter here beyond tidiness: a multi-line field would break the
/// one-event-per-line shape that makes this log greppable, and a model-written
/// string can contain both newlines and terminal control characters.
fn truncate(text: &str, max_chars: usize) -> String {
    let collapsed: String = text
        .chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .filter(|c| !c.is_control() || *c == '\t')
        .collect();
    let trimmed = collapsed.trim();
    if trimmed.chars().count() <= max_chars {
        return trimmed.to_string();
    }
    let cut: String = trimmed.chars().take(max_chars).collect();
    format!("{cut}…")
}

/// Whether an event is reasoning noise that must never reach the log.
///
/// Exposed so the writer can skip the whole variant without matching on it
/// again; the answer lives next to the format decision it belongs to.
pub fn is_noise(update: &AgentUpdate) -> bool {
    matches!(
        update,
        AgentUpdate::StreamChunk(_) | AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(_))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{AgentErrorKind, ModelCallParams, TokenUsageInfo, ToolPresentationInfo};

    fn step_result(tool: &str, status: StepStatus, message: &str) -> StepResult {
        StepResult {
            tool: tool.to_string(),
            arg_summary: String::new(),
            arg_full: None,
            status,
            message: message.to_string(),
            detail: None,
            duration_us: Some(1_500_000),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("t"),
        }
    }

    #[test]
    fn a_started_step_names_the_tool_and_its_arguments() {
        let update = AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "t1".into(),
            tool_name: "bash".into(),
            arg_summary: "git log --oneline -5".into(),
            arg_full: String::new(),
            presentation: ToolPresentationInfo::generic("t"),
        };
        assert_eq!(
            format_event(&update).as_deref(),
            Some("[tool] bash git log --oneline -5")
        );
    }

    #[test]
    fn a_finished_step_reports_status_duration_and_message() {
        let ok = AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "t1".into(),
            result: step_result("read_file", StepStatus::Success, "12 lines"),
        };
        assert_eq!(
            format_event(&ok).as_deref(),
            Some("[tool ok] read_file 12 lines (1500ms)")
        );

        let failed = AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "t1".into(),
            result: step_result("bash", StepStatus::Failed, "exit 1"),
        };
        assert_eq!(
            format_event(&failed).as_deref(),
            Some("[tool failed] bash exit 1 (1500ms)")
        );
    }

    #[test]
    fn a_failed_step_without_a_duration_omits_the_suffix() {
        let mut result = step_result("bash", StepStatus::Failed, "denied");
        result.duration_us = None;
        let update = AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "t".into(),
            result,
        };
        assert_eq!(
            format_event(&update).as_deref(),
            Some("[tool failed] bash denied")
        );
    }

    #[test]
    fn multi_line_fields_are_collapsed_so_one_event_stays_one_line() {
        let update = AgentUpdate::Info("first\nsecond\r\nthird".into());
        assert_eq!(
            format_event(&update).as_deref(),
            Some("[info] first second  third")
        );
    }

    #[test]
    fn a_long_field_is_truncated_rather_than_breaking_the_log() {
        let long = "x".repeat(MAX_FIELD_CHARS + 50);
        let update = AgentUpdate::Info(long);
        let line = format_event(&update).expect("info is logged");
        assert!(line.ends_with('…'), "expected ellipsis: {line}");
        // The `[info] ` prefix is outside the cap, the field is inside it.
        assert_eq!(line.chars().count(), "[info] ".len() + MAX_FIELD_CHARS + 1);
    }

    #[test]
    fn streaming_text_and_reasoning_are_not_logged() {
        // The final answer goes to stdout; echoing deltas would duplicate it.
        assert!(format_event(&AgentUpdate::StreamChunk("partial".into())).is_none());
        assert!(
            format_event(&AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
                "hmm".into()
            )))
            .is_none()
        );
    }

    #[test]
    fn permission_requests_are_not_logged_as_progress() {
        // Headless cannot answer these; logging them would suggest it did.
        let update = AgentUpdate::RequestSelect {
            request_id: 1,
            prompt: "Allow bash?".into(),
            options: vec!["Allow".into()],
            log_confirm: false,
            from_subagent: false,
        };
        assert!(format_event(&update).is_none());
    }

    #[test]
    fn turn_and_token_counters_are_reported() {
        let turn = AgentUpdate::TurnStats {
            turns_taken: 3,
            max_turns: Some(40),
        };
        assert_eq!(format_event(&turn).as_deref(), Some("[turn] 3/40"));

        let unbounded = AgentUpdate::TurnStats {
            turns_taken: 3,
            max_turns: None,
        };
        assert_eq!(format_event(&unbounded).as_deref(), Some("[turn] 3"));

        let tokens = AgentUpdate::TokenUsage(TokenUsageInfo {
            prompt: 10,
            completion: 5,
            total: 15,
            ..Default::default()
        });
        assert_eq!(
            format_event(&tokens).as_deref(),
            Some("[tokens] prompt=10 completion=5 total=15")
        );
    }

    #[test]
    fn model_and_error_lines_carry_the_failure_reason() {
        let model = AgentUpdate::ModelInfo(ModelCallParams {
            model: "deepseek-v4".into(),
            max_tokens: 8000,
            thinking_budget: None,
            reasoning_effort: None,
            extra_body: None,
        });
        assert_eq!(
            format_event(&model).as_deref(),
            Some("[model] deepseek-v4 max_tokens=8000")
        );

        let error = AgentUpdate::Error(AgentErrorKind::Other("gateway 502".into()));
        assert_eq!(format_event(&error).as_deref(), Some("[error] gateway 502"));
    }

    #[test]
    fn noise_is_named_separately_from_being_formatted() {
        assert!(is_noise(&AgentUpdate::StreamChunk("x".into())));
        assert!(is_noise(&AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "x".into()
        ))));
        assert!(!is_noise(&AgentUpdate::Info("x".into())));
    }
}
