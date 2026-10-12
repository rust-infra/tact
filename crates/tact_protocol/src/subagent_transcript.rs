//! Structured subagent transcript.
//!
//! A `spawn_subagent` child runs a nested agent loop and reports the same
//! updates the main agent does (`Text`, `Thinking`, `Step*`, …).
//! The parent's tagged forwarder turns those into two parallel streams:
//!
//! * the flattened text chunks that feed the parent tool card's live preview
//!   ([`crate::RuntimeEvent::ToolProgress`]), and
//! * the structured entries defined here
//!   ([`crate::RuntimeEvent::ToolTranscript`]), which the subagent view renders
//!   with the main log's own cell vocabulary.
//!
//! The structured stream exists because the flattened one cannot be reversed:
//! a `→ name args` line is indistinguishable from a paragraph, and the popup
//! needs to know which glyphs are a tool card's title, its meta row, and its
//! output. Nothing here is language-derived — every field is a fact about what
//! the child did, and the locale is applied where the block is drawn.

use serde::{Deserialize, Serialize};

use crate::agent::ToolPresentationInfo;

/// Status of one child tool invocation.
///
/// Deliberately separate from [`crate::StepStatus`], which has no `Running`:
/// the transcript records the `StepStarted` half of the lifecycle as its own
/// entry and rewrites it in place when the invocation finishes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStepStatus {
    Running,
    Success,
    Failed,
}

/// One tool invocation made by a subagent.
///
/// Emitted twice per invocation — once from `StepStarted` with
/// [`SubagentStepStatus::Running`], once from `StepFinished` / `StepFailed`
/// with the terminal status. Both halves carry the same `tool_id` (the child's
/// own), which is how the UI pairs them into one card, exactly like the main
/// agent's `ActiveToolBlock` → `ToolBlock` transition.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentStep {
    /// The child's local tool id — the key pairing the two halves.
    pub tool_id: String,
    pub tool_name: String,
    pub arg_summary: String,
    /// Full (untruncated) argument summary, used by the card's title.
    pub arg_full: String,
    pub status: SubagentStepStatus,
    /// Result summary, or the error text when the call failed.
    pub message: String,
    /// Full result detail (file content, command output) for the card's
    /// preview and the future `[Open]` affordance.
    pub detail: Option<String>,
    pub duration_us: Option<u64>,
    /// Presentation metadata: decides the card's visual kind (command /
    /// read / write / diff), so the popup draws the same card the main log
    /// would for the same call.
    pub presentation: ToolPresentationInfo,
}

/// One entry in a subagent's transcript.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SubagentTranscriptEntry {
    /// A run of assistant text. Consecutive `StreamChunk`s are coalesced into
    /// one entry so the popup renders one Markdown block per assistant turn
    /// rather than one per token batch.
    Assistant(String),
    /// A finished thinking block. Emitted on `ThinkingChunk::Finished`; the
    /// child's deltas are accumulated by the forwarder, which also measures
    /// `duration_us` — the transcript is the only place that knows when the
    /// block started, so the card's `⏱` readout has to come from here.
    Thinking {
        text: String,
        duration_us: Option<u64>,
    },
    /// A tool invocation (see [`SubagentStep`]).
    Step(SubagentStep),
    /// An informational line the child emitted (`Info` / `MdInfo`).
    Info(String),
    /// An error line (`Error`).
    Error(String),
}

impl SubagentTranscriptEntry {
    /// Plain text this entry contributes to the popup's selectable buffer.
    ///
    /// The buffer is what drag-selection and `y` copy read, so it must contain
    /// the text a reader would expect to copy — including a tool call's
    /// arguments and result, which are drawn inside a card and would otherwise
    /// be unreachable.
    pub fn selection_text(&self) -> String {
        match self {
            Self::Assistant(text) | Self::Info(text) | Self::Error(text) => text.clone(),
            Self::Thinking { text, .. } => text.clone(),
            Self::Step(step) => {
                let mut text = if step.arg_summary.is_empty() {
                    step.tool_name.clone()
                } else {
                    format!("{} {}", step.tool_name, step.arg_summary)
                };
                if !step.message.is_empty() {
                    text.push('\n');
                    text.push_str(&step.message);
                }
                text
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_text_covers_a_step_arguments_and_result() {
        let step = SubagentStep {
            tool_id: "t1".into(),
            tool_name: "read_file".into(),
            arg_summary: "src/main.rs".into(),
            arg_full: "src/main.rs".into(),
            status: SubagentStepStatus::Success,
            message: "fn main() {}".into(),
            detail: None,
            duration_us: Some(1),
            presentation: ToolPresentationInfo::generic("read_file"),
        };
        let text = SubagentTranscriptEntry::Step(step).selection_text();
        assert!(text.contains("read_file src/main.rs"), "{text}");
        assert!(text.contains("fn main()"), "{text}");
    }

    #[test]
    fn selection_text_of_a_running_step_has_no_result_line() {
        let step = SubagentStep {
            tool_id: "t1".into(),
            tool_name: "bash".into(),
            arg_summary: "ls".into(),
            arg_full: "ls".into(),
            status: SubagentStepStatus::Running,
            message: String::new(),
            detail: None,
            duration_us: None,
            presentation: ToolPresentationInfo::generic("bash"),
        };
        assert_eq!(
            SubagentTranscriptEntry::Step(step).selection_text(),
            "bash ls"
        );
    }
}
