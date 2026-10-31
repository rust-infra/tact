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
//! - `TaskComplete` / `TaskCancelled`: the driver emits those, and a headless
//!   run has no driver. Logging them here would describe a state this process
//!   never reaches.
//!
//! [`AgentUpdate::ToolProgress`] *is* logged: a command that prints for a
//! minute is otherwise indistinguishable from one that is wedged.
//!
//! Every field that can carry model- or tool-authored text goes through
//! [`truncate`], including [`AgentUpdate::Error`]: an error chain built with
//! `{error:#}` contains newlines, and one event must stay one line.

use tact_protocol::{AgentUpdate, StepResult, StepStatus};

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

        // A command that prints for a minute looks identical to one that is
        // wedged, unless its output reaches the log. One line per chunk keeps
        // the one-event-per-line shape; the final `StepFinished` line still
        // carries the verdict and duration.
        AgentUpdate::ToolProgress {
            run_id: None,
            tool_id,
            chunks,
            ..
        } => {
            let text: String = chunks.iter().map(|chunk| chunk.text.as_str()).collect();
            let text = text.trim_end();
            if text.is_empty() {
                return None;
            }
            Some(format!(
                "[out {tool_id}] {}",
                truncate(text, MAX_FIELD_CHARS)
            ))
        }

        AgentUpdate::StepFailed { error, .. } => Some(format!(
            "[tool failed] {}",
            truncate(error, MAX_FIELD_CHARS)
        )),

        AgentUpdate::Info(message) => {
            Some(format!("[info] {}", truncate(message, MAX_FIELD_CHARS)))
        }

        AgentUpdate::Error(kind) => Some(format!(
            "[error] {}",
            truncate(&kind.to_string(), MAX_FIELD_CHARS)
        )),

        // A plugin declares `statusMessage` because its hook can take seconds;
        // headless has no row to rewrite, so it prints the open and the close.
        AgentUpdate::HookStatus {
            message,
            elapsed_ms,
            ..
        } => Some(match elapsed_ms {
            Some(ms) => format!("[hook done {ms}ms] {}", truncate(message, MAX_FIELD_CHARS)),
            None => format!("[hook] {}", truncate(message, MAX_FIELD_CHARS)),
        }),

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
            truncate(&params.model, MAX_FIELD_CHARS),
            params.max_tokens
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

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{
        AgentErrorKind, ModelCallParams, ThinkingChunk, TokenUsageInfo, ToolOutputChunk,
        ToolPresentationInfo,
    };

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
            run_id: None,
            tool_id: "t1".into(),
            result: step_result("read_file", StepStatus::Success, "12 lines"),
        };
        assert_eq!(
            format_event(&ok).as_deref(),
            Some("[tool ok] read_file 12 lines (1500ms)")
        );

        let failed = AgentUpdate::StepFinished {
            idx: 0,
            run_id: None,
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
            run_id: None,
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
            purpose: tact_protocol::UiRequestPurpose::Permission,
            origin: tact_protocol::UiRequestOrigin::main(),
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
    fn an_error_chain_stays_on_one_line_and_is_capped() {
        let chained = AgentErrorKind::Other("gateway 502\nretrying\nafter backoff".into());
        assert_eq!(
            format_event(&AgentUpdate::Error(chained)).as_deref(),
            Some("[error] gateway 502 retrying after backoff")
        );

        let long = AgentErrorKind::Other("e".repeat(MAX_FIELD_CHARS + 20));
        let line = format_event(&AgentUpdate::Error(long)).expect("errors are logged");
        assert_eq!(line.chars().count(), "[error] ".len() + MAX_FIELD_CHARS + 1);
    }

    #[test]
    fn a_hook_status_prints_open_and_closed() {
        let running = AgentUpdate::HookStatus {
            id: 1,
            source: Some("plugin codex".into()),
            message: "warming up".into(),
            elapsed_ms: None,
        };
        assert_eq!(format_event(&running).as_deref(), Some("[hook] warming up"));

        let done = AgentUpdate::HookStatus {
            id: 1,
            source: Some("plugin codex".into()),
            message: "warming up".into(),
            elapsed_ms: Some(2_500),
        };
        assert_eq!(
            format_event(&done).as_deref(),
            Some("[hook done 2500ms] warming up")
        );
    }

    #[test]
    fn live_tool_output_is_logged_so_a_slow_command_stays_visible() {
        let update = AgentUpdate::ToolProgress {
            run_id: None,
            tool_id: "bash-7".into(),
            chunks: vec![
                ToolOutputChunk::stdout("alpha\n"),
                ToolOutputChunk::stdout("beta\n"),
            ],
        };
        assert_eq!(
            format_event(&update).as_deref(),
            Some("[out bash-7] alpha beta")
        );

        let empty = AgentUpdate::ToolProgress {
            run_id: None,
            tool_id: "bash-7".into(),
            chunks: vec![ToolOutputChunk::stdout("\n")],
        };
        assert!(format_event(&empty).is_none(), "a blank chunk is not news");
    }

    #[test]
    fn driver_only_lifecycle_events_are_not_logged() {
        // A headless run has no driver, so these describe a state it never
        // reaches; printing them would claim otherwise.
        assert!(format_event(&AgentUpdate::TaskComplete("done".into())).is_none());
        assert!(format_event(&AgentUpdate::TaskCancelled).is_none());
    }
}
