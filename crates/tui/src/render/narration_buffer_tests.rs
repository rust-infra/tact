//! Narration that is still buffered when its message finishes.
//!
//! `MessageFinished` carries the phase, and the app relabels the rows the
//! message wrote. But the stream keeps an unterminated last paragraph in its
//! buffer until something forces a flush, so the rows the label lands on can be
//! *fewer* than the rows the message will eventually produce — and the tail
//! that lands later is appended as plain `AssistantMarkdown`, which never folds.
//!
//! The 2026-11-19 transcript had exactly this shape: a narration paragraph
//! above a `2 tools · $ Bash ×2` region that was not folded into it.

use std::collections::HashMap;

use tact_protocol::{AgentUpdate, PlanStep, StepStatus};

use crate::render::test_harness::{make_app, render_log_panel_text};
use crate::test_fixtures::StepCall;
use crate::widgets::state::App;

fn seed(app: &mut App, idx: usize, id: &str, tool: &str, arg: &str) {
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        format!("step {id}"),
        tool,
        id,
        HashMap::from([("arg".to_string(), arg.to_string())]),
    )));
    app.handle_agent_update(StepCall::new(idx, id, tool, arg).started());
    app.handle_agent_update(
        StepCall::new(idx, id, tool, arg)
            .detail("out-1\nout-2\nout-3")
            .status(StepStatus::Success)
            .duration_us(1_200)
            .finished(),
    );
}

/// The tail of a commentary message arrives *after* `MessageFinished`.
///
/// This is the screenshot's shape: the paragraph ends without a blank line, so
/// it is still in the stream buffer when the label is applied.
#[test]
fn narration_still_buffered_at_message_finished_still_folds() {
    let mut app = make_app();
    app.add_user_message("commit".into());
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m1".into(),
    });
    // No trailing blank line: the paragraph stays in `stream.buffer`.
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "分支正确。这批改动含两个关注点。".into(),
    ));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m1".into(),
        phase: Some(tact_protocol::MessagePhase::Commentary),
    });

    seed(&mut app, 0, "b1", "bash", "git commit");
    seed(&mut app, 1, "b2", "bash", "git log");

    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m2".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("提交完成。\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m2".into(),
        phase: Some(tact_protocol::MessagePhase::FinalAnswer),
    });

    let text = render_log_panel_text(&mut app, 100, 32);
    // A wide glyph occupies two cells, so the buffer pads between characters:
    // compare glyphs, not the contiguous string (as the `/lang` tests do).
    let glyphs: String = text.chars().filter(|c| !c.is_whitespace()).collect();
    assert!(
        glyphs.contains("提交完成。"),
        "the answer is never folded: {text}"
    );
    assert!(
        !glyphs.contains("分支正确"),
        "buffered narration joins the region: {text}"
    );
    assert!(
        glyphs.contains("1条") || glyphs.contains("1note"),
        "the region counts the narration it swallowed: {text}"
    );
}
