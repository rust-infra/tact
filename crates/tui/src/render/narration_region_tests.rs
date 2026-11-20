//! A region that *starts* with narration, not with a card.
//!
//! `activity_region::is_folded` folds any region that holds narration beside a
//! call or a card, but three call sites still ask `mixes_thinking_and_tools()`
//! — the rule that predates narration. A region headed by a narration run is
//! therefore folded by the renderer and by `z`, yet:
//!
//! - `expanded_region_head_at` answers `None`, so an opened region draws no fold
//!   control and can never be closed again by mouse;
//! - `first_fold_in_view` skips it, so `z` toggles the tool run *underneath*
//!   instead of the region that stands for the whole stretch.
//!
//! These are the tests that pin the two halves.

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

fn z(app: &mut App) {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    app.input_mode = crate::widgets::state::InputMode::Normal;
    crate::handlers::handle_normal_mode(
        app,
        crossterm::event::KeyEvent::new(
            crossterm::event::KeyCode::Char('z'),
            crossterm::event::KeyModifiers::NONE,
        ),
        &tx,
    );
}

/// `narrate → 2 tools → answer`: the shape that produces a region headed by a
/// narration run and holding no thinking card.
fn narration_headed_region(app: &mut App) {
    app.add_user_message("commit".into());
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m1".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("Branch looks right.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m1".into(),
        phase: Some(tact_protocol::MessagePhase::Commentary),
    });
    seed(app, 0, "b1", "bash", "git commit");
    seed(app, 1, "b2", "bash", "git log");
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m2".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("Done.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m2".into(),
        phase: Some(tact_protocol::MessagePhase::FinalAnswer),
    });
}

/// The region folds and counts its narration: the baseline this file builds on.
#[test]
fn a_narration_headed_region_folds_by_default() {
    let mut app = make_app();
    narration_headed_region(&mut app);

    let text = render_log_panel_text(&mut app, 100, 32);
    assert!(
        text.contains("1 note") && text.contains("2 tools"),
        "one row must count the narration and both calls: {text}"
    );
    assert!(
        !text.contains("Branch looks right."),
        "the narration is behind the summary: {text}"
    );
}

/// `z` must flip the region the summary row belongs to, not the tool run that
/// sits under it. Before, the region was never offered to `z` — `first_fold_in_view`
/// gated on `mixes_thinking_and_tools()`, which a narration-headed region fails —
/// so the press expanded the inner run and left the region summary in place with
/// the members painted under it.
#[test]
fn z_opens_the_narration_headed_region_it_stands_for() {
    let mut app = make_app();
    narration_headed_region(&mut app);
    let folded = render_log_panel_text(&mut app, 100, 32);
    assert!(folded.contains("1 note"), "starts folded: {folded}");

    z(&mut app);
    let open = render_log_panel_text(&mut app, 100, 32);
    assert!(
        open.contains("Branch looks right."),
        "the press must reveal the narration and its calls: {open}"
    );
    assert!(
        !open.contains("1 note"),
        "the region summary must be gone, not left above its own members: {open}"
    );
}

/// The same defect on the mouse path: an opened region's fold control is its
/// first member's row, found through `expanded_region_head_at`. The helper
/// derived the regions *without* narration, so a narration-headed region
/// answered `None`, and its rows came back with no way to close them again.
#[test]
fn clicking_an_open_narration_headed_region_head_folds_it_back() {
    use crate::render::test_harness::render_app_text;

    let mut app = make_app();
    narration_headed_region(&mut app);
    let _ = render_app_text(&mut app, 100, 32);
    let summary = app
        .mouse
        .open_actions
        .iter()
        .find(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ActivityRegion { .. }
            )
        })
        .cloned()
        .expect("the folded summary is a control");
    click(&mut app, summary.rect.x + 2, summary.rect.y);
    let open = render_app_text(&mut app, 100, 32);
    assert!(
        open.contains("Branch looks right."),
        "the click opens the region: {open}"
    );

    let head = app
        .mouse
        .open_actions
        .iter()
        .find(|action| {
            matches!(
                action.target,
                agent_tui_kit::render::log::OpenTarget::ActivityRegion { .. }
            )
        })
        .cloned()
        .expect("an open region's head is the control that closes it");
    click(&mut app, head.rect.x, head.rect.y);

    let again = render_app_text(&mut app, 100, 32);
    assert!(
        again.contains("1 note") && !again.contains("Branch looks right."),
        "the head must fold the region back: {again}"
    );
}

/// A left-button press at `(x, y)`, routed through the real mouse handler.
fn click(app: &mut App, x: u16, y: u16) {
    crate::handle_mouse_event(
        app,
        crossterm::event::MouseEvent {
            kind: crossterm::event::MouseEventKind::Down(crossterm::event::MouseButton::Left),
            column: x,
            row: y,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
    );
}

/// Two narration-headed regions must have *different* fold keys.
///
/// A run's identity is its first row's text — the doc says so, and it is what
/// survives an insert above the run. But a narration run commonly opens with a
/// blank separator (the gap before the paragraph, or the one
/// `ensure_gap_after_tools` leaves), and that row's text is `""`. Keyed on it,
/// every such region shares one key: folding one folds them all, and a reader's
/// choice lands on a region they never touched.
#[test]
fn two_narration_headed_regions_do_not_share_a_fold_key() {
    let mut app = make_app();
    // Turn one.
    app.add_user_message("first".into());
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m1".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("First.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m1".into(),
        phase: Some(tact_protocol::MessagePhase::Commentary),
    });
    seed(&mut app, 0, "b1", "bash", "one");
    app.handle_agent_update(AgentUpdate::StreamChunk("Answer one.\n\n".into()));
    // Turn two, same shape.
    app.add_user_message("second".into());
    app.handle_agent_update(AgentUpdate::MessageStarted {
        message_id: "m2".into(),
    });
    app.handle_agent_update(AgentUpdate::StreamChunk("Second.\n\n".into()));
    app.handle_agent_update(AgentUpdate::MessageFinished {
        message_id: "m2".into(),
        phase: Some(tact_protocol::MessagePhase::Commentary),
    });
    seed(&mut app, 1, "b2", "bash", "two");
    app.handle_agent_update(AgentUpdate::StreamChunk("Answer two.\n\n".into()));

    let regions: Vec<_> = app
        .activity_regions()
        .into_iter()
        .filter(|region| region.commentary() > 0 && region.tools() > 0)
        .collect();
    assert!(
        regions.len() >= 2,
        "the fixture must produce two narration regions: {}",
        regions.len()
    );
    let keys: std::collections::BTreeSet<&str> =
        regions.iter().map(|region| region.key.as_str()).collect();
    assert_eq!(
        keys.len(),
        regions.len(),
        "each region needs its own key, got {keys:?}"
    );
}
