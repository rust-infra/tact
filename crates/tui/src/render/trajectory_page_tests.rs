//! The trajectory page as the app uses it: entry, close, keys, and the fact
//! that it owns the main area.
//!
//! The ledger's own behaviour is tested in `agent_tui_kit::state::trajectory`
//! and the painting in `agent_tui_kit::render::trajectory`. What is left to
//! prove here is the wiring: that `/trajectory` opens it, that closing hands
//! the keyboard back and clears every hit rect, that only one page is up at a
//! time, and that the keys reach the ledger.

use agent_tui_kit::state::trajectory::{TrajectoryKind, TrajectoryStatus};
use tact_protocol::{AgentUpdate, StepResult, StepStatus, ToolPresentationInfo};
use tokio::sync::mpsc::unbounded_channel;

use super::test_harness::{make_app, render_app_text};
use crate::widgets::state::InputMode as TuiInputMode;

fn queued(tool_id: &str, name: &str, args: &str) -> AgentUpdate {
    AgentUpdate::StepQueued {
        idx: 0,
        tool_id: tool_id.into(),
        tool_name: name.into(),
        arg_summary: args.into(),
        arg_full: String::new(),
        presentation: ToolPresentationInfo::generic(name),
    }
}

fn finished(tool_id: &str, name: &str, duration_us: Option<u64>) -> AgentUpdate {
    AgentUpdate::StepFinished {
        idx: 0,
        run_id: None,
        tool_id: tool_id.into(),
        result: StepResult {
            tool: name.into(),
            arg_summary: String::new(),
            arg_full: None,
            status: StepStatus::Success,
            message: "body".into(),
            detail: Some("the detail".into()),
            duration_us,
            permission_label: None,
            presentation: ToolPresentationInfo::generic(name),
        },
    }
}

fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
}

fn press(app: &mut crate::widgets::state::App, code: crossterm::event::KeyCode) {
    // The real handler, not a stand-in: the point is that the page's keys are
    // reachable from Normal mode.
    let (tx, _rx) = unbounded_channel();
    crate::handlers::handle_normal_mode_for_page_tests(app, key(code), &tx);
}

/// One wheel notch, through the real mouse dispatcher.
fn wheel(app: &mut crate::widgets::state::App, kind: crossterm::event::MouseEventKind) {
    // The middle of an 80x30 frame: inside the page body, so a test that only
    // cares about the page does not depend on where its border happens to be.
    wheel_at(app, kind, 40, 15);
}

fn wheel_at(
    app: &mut crate::widgets::state::App,
    kind: crossterm::event::MouseEventKind,
    column: u16,
    row: u16,
) {
    crate::handle_mouse_event(
        app,
        crossterm::event::MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        },
    );
}

/// A run with two turns and a few records, which is the smallest shape that
/// exercises turn numbering and the ledger.
fn app_with_a_run() -> crate::widgets::state::App {
    let mut app = make_app();
    app.begin_trajectory_turn("why does the retry test fail");
    app.handle_agent_update(queued("call-1", "read_file", "src/lib.rs"));
    app.handle_agent_update(finished("call-1", "read_file", Some(1_200_000)));
    app.begin_trajectory_turn("fix it");
    app.handle_agent_update(queued("call-2", "bash", "cargo test"));
    app.handle_agent_update(finished("call-2", "bash", None));
    app
}

#[test]
fn the_slash_command_opens_the_page_and_takes_the_keyboard() {
    let mut app = make_app();
    app.begin_trajectory_turn("go");
    assert!(!app.show_trajectory);

    app.open_trajectory_page();

    assert!(app.show_trajectory, "the page is up");
    assert_eq!(
        app.input_mode,
        TuiInputMode::Normal,
        "the page's keys are dispatched from Normal mode, so opening must take it"
    );
}

#[test]
fn opening_one_page_closes_the_others() {
    let mut app = make_app();
    app.show_help = true;
    app.show_history = true;
    app.show_subagent_view = true;

    app.open_trajectory_page();

    assert!(app.show_trajectory);
    assert!(!app.show_help, "two pages cannot both claim the main area");
    assert!(!app.show_history);
    assert!(!app.show_subagent_view);
}

#[test]
fn closing_returns_insert_mode_and_clears_the_hit_area() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    render_app_text(&mut app, 80, 24);
    let painted = app
        .mouse
        .area(crate::widgets::state::SurfaceId::TrajectoryPage);
    assert!(painted.height > 0, "the page records where it painted");

    app.close_trajectory_page();

    assert!(!app.show_trajectory);
    assert_eq!(
        app.input_mode,
        TuiInputMode::Insert,
        "the reader must be able to type again"
    );
    assert!(!app.trajectory_page.filter_editing);
    // A closed page that kept its rect would both size the ledger from a stale
    // frame and swallow wheel events meant for the log underneath.
    assert_eq!(
        app.mouse
            .area(crate::widgets::state::SurfaceId::TrajectoryPage),
        ratatui::layout::Rect::default(),
        "the closed page must not keep a hit rect"
    );
    assert!(
        !app.mouse.hits(
            crate::widgets::state::SurfaceId::TrajectoryPage,
            painted.x,
            painted.y
        ),
        "nothing answers a point inside a closed page"
    );
}

#[test]
fn opening_lands_on_the_newest_record() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();

    let last = app.trajectory.records().last().expect("a record").id;
    assert_eq!(
        app.trajectory_page.selected,
        Some(last),
        "a reader opening mid run wants the newest work, not the top"
    );
    assert!(app.trajectory_page.follow);
}

#[test]
fn the_page_paints_its_summary_ledger_and_turn_rules() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    let text = render_app_text(&mut app, 80, 30);

    assert!(
        text.contains("Trajectory"),
        "the page replaced the log:\n{text}"
    );
    assert!(text.contains("2 turns"), "summary:\n{text}");
    assert!(text.contains("TURN 1"), "first turn:\n{text}");
    assert!(text.contains("TURN 2"), "second turn:\n{text}");
    assert!(text.contains("read_file"), "a tool record:\n{text}");
    // The page replaced the log rather than covering it: the log's own box is
    // gone, so nothing of it is painted underneath. Counting the ledger's own
    // turn rules is the observable form of that.
    assert_eq!(
        text.matches("TURN ").count(),
        2,
        "the ledger drew both turns and nothing twice:\n{text}"
    );
}

#[test]
fn j_and_k_move_the_selection_and_g_and_shift_g_jump() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    let count = app.trajectory.len();
    assert!(count >= 3);

    press(&mut app, crossterm::event::KeyCode::Char('g'));
    let first = app.trajectory_page.selected.expect("selection");
    assert_eq!(
        app.trajectory.position_of(first),
        Some(0),
        "g goes to the first record"
    );

    press(&mut app, crossterm::event::KeyCode::Char('j'));
    let second = app.trajectory_page.selected.expect("selection");
    assert_ne!(second, first, "j moves one record");

    press(&mut app, crossterm::event::KeyCode::Char('k'));
    assert_eq!(app.trajectory_page.selected, Some(first), "k moves back");

    press(&mut app, crossterm::event::KeyCode::Char('G'));
    let last = app.trajectory_page.selected.expect("selection");
    assert_eq!(
        app.trajectory.position_of(last),
        Some(count - 1),
        "G goes to the newest record"
    );
}

#[test]
fn the_selection_is_an_identity_so_an_append_does_not_move_it() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    press(&mut app, crossterm::event::KeyCode::Char('g'));
    press(&mut app, crossterm::event::KeyCode::Char('j'));
    let selected = app.trajectory_page.selected.expect("selection");

    app.handle_agent_update(queued("call-3", "grep", "needle"));

    assert_eq!(
        app.trajectory_page.selected,
        Some(selected),
        "a live run must not move the reader's place"
    );
    assert!(
        app.trajectory.position_of(selected).is_some(),
        "and the record it points at still exists"
    );
}

#[test]
fn escape_closes_the_page_when_there_is_no_filter() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();

    press(&mut app, crossterm::event::KeyCode::Esc);

    assert!(!app.show_trajectory, "esc is the way back");
}

#[test]
fn q_closes_the_page() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();

    press(&mut app, crossterm::event::KeyCode::Char('q'));

    assert!(!app.show_trajectory);
}

#[test]
fn slash_starts_a_filter_and_typing_narrows_the_ledger() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();

    press(&mut app, crossterm::event::KeyCode::Char('/'));
    assert!(app.trajectory_page.filter_editing);
    for c in "bash".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(c));
    }
    assert_eq!(app.trajectory_page.filter, "bash");

    press(&mut app, crossterm::event::KeyCode::Enter);
    assert!(!app.trajectory_page.filter_editing, "enter applies");

    let text = render_app_text(&mut app, 100, 30);
    assert!(text.contains("bash"), "the match is shown:\n{text}");
    assert!(
        !text.contains("read_file"),
        "the non-match is hidden:\n{text}"
    );
}

#[test]
fn escape_clears_the_filter_before_leaving_the_page() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    press(&mut app, crossterm::event::KeyCode::Char('/'));
    for c in "bash".chars() {
        press(&mut app, crossterm::event::KeyCode::Char(c));
    }
    press(&mut app, crossterm::event::KeyCode::Enter);

    press(&mut app, crossterm::event::KeyCode::Esc);

    assert!(app.show_trajectory, "the page stays up");
    assert!(app.trajectory_page.filter.is_empty(), "the filter cleared");

    press(&mut app, crossterm::event::KeyCode::Esc);
    assert!(!app.show_trajectory, "a second esc leaves");
}

#[test]
fn o_toggles_the_overview_against_the_height_default() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();

    // At 30 rows the page shows the plot by default.
    assert!(app.trajectory_page.overview.is_none());
    let before = render_app_text(&mut app, 80, 30);
    assert!(
        before.contains("INPUT"),
        "default shows the plot:\n{before}"
    );

    press(&mut app, crossterm::event::KeyCode::Char('o'));
    let after = render_app_text(&mut app, 80, 30);
    assert!(!after.contains("INPUT"), "o hides the plot:\n{after}");

    press(&mut app, crossterm::event::KeyCode::Char('o'));
    let again = render_app_text(&mut app, 80, 30);
    assert!(again.contains("INPUT"), "and shows it again:\n{again}");
}

#[test]
fn enter_opens_the_detail_panel_and_tab_cycles_its_tabs() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();

    press(&mut app, crossterm::event::KeyCode::Enter);
    assert!(app.trajectory_page.detail);
    let opened = render_app_text(&mut app, 100, 34);
    assert!(opened.contains("Summary"), "detail title:\n{opened}");

    press(&mut app, crossterm::event::KeyCode::Tab);
    let payload = render_app_text(&mut app, 100, 34);
    assert!(
        payload.contains("Payload"),
        "tab moved the panel:\n{payload}"
    );

    press(&mut app, crossterm::event::KeyCode::Enter);
    assert!(!app.trajectory_page.detail, "enter closes it again");
}

#[test]
fn the_ledger_records_a_tool_with_its_name_and_measured_duration() {
    let app = app_with_a_run();
    let record = app
        .trajectory
        .records()
        .iter()
        .find(|record| record.tool_name.as_deref() == Some("read_file"))
        .expect("the tool record");

    assert_eq!(record.kind, TrajectoryKind::Tool);
    assert_eq!(record.text, "read_file src/lib.rs");
    assert_eq!(record.duration_ms, Some(1_200));
    assert_eq!(record.turn, 1, "it belongs to the prompt that caused it");

    let unmeasured = app
        .trajectory
        .records()
        .iter()
        .find(|record| record.tool_name.as_deref() == Some("bash"))
        .expect("the second tool");
    assert_eq!(unmeasured.duration_ms, None);
    assert_eq!(unmeasured.turn, 2);
}

#[test]
fn the_page_updates_while_it_is_open() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    let before = app.trajectory.len();

    app.handle_agent_update(queued("call-3", "grep", "needle"));
    app.handle_agent_update(finished("call-3", "grep", Some(900_000)));

    assert_eq!(app.trajectory.len(), before + 1, "the record landed");
    let text = render_app_text(&mut app, 100, 30);
    assert!(text.contains("grep"), "and it is on screen:\n{text}");
}

#[test]
fn a_closed_page_does_not_steal_keys_from_the_log() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    app.close_trajectory_page();

    // `j` with the page closed must not touch the page's selection.
    let before = app.trajectory_page.selected;
    press(&mut app, crossterm::event::KeyCode::Char('j'));

    assert_eq!(
        app.trajectory_page.selected, before,
        "a closed page's keymap is inert"
    );
}

#[test]
fn every_page_flag_resolves_to_exactly_one_surface() {
    let mut app = make_app();
    app.open_trajectory_page();
    let text = render_app_text(&mut app, 80, 24);
    assert!(text.contains("Trajectory"), "trajectory wins:\n{text}");

    app.show_help = true;
    let text = render_app_text(&mut app, 80, 24);
    assert!(
        !text.contains("Trajectory"),
        "help resolves before the trajectory branch, so only one paints:\n{text}"
    );
}

#[test]
fn the_filter_rejects_nothing_and_accepts_unicode() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    app.begin_trajectory_filter();
    app.push_trajectory_filter('读');
    app.finish_trajectory_filter();

    let text = render_app_text(&mut app, 100, 30);
    assert!(
        text.contains("读"),
        "the query is echoed in the summary:\n{text}"
    );
    assert!(
        !text.contains("read_file"),
        "and nothing matches it:\n{text}"
    );
}

#[test]
fn a_live_append_keeps_the_follower_on_the_newest_record() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    assert!(app.trajectory_page.follow);
    let before = app.trajectory_page.selected.expect("selection");

    app.handle_agent_update(queued("call-3", "grep", "needle"));
    app.handle_agent_update(finished("call-3", "grep", Some(900_000)));

    let after = app.trajectory_page.selected.expect("selection");
    assert_ne!(after, before, "following moves the selection with the tail");
    assert_eq!(
        after,
        app.trajectory.records().last().expect("last").id,
        "the newest record is the selection"
    );
    assert!(
        app.trajectory_page.follow,
        "and the reader is still following"
    );
}

/// The bug this catches: while following, `scroll` is meaningless and the
/// renderer derives it. Leaving the tail without pinning the offset made the
/// first `k` jump to the top of the ledger.
#[test]
fn leaving_the_tail_pins_the_offset_instead_of_jumping_to_the_top() {
    let mut app = app_with_a_run();
    // Enough records that the ledger must scroll.
    for index in 0..40 {
        app.handle_agent_update(queued(&format!("bulk-{index}"), "bash", "true"));
    }
    app.open_trajectory_page();
    assert!(app.trajectory_page.follow);

    press(&mut app, crossterm::event::KeyCode::Char('k'));

    assert!(
        !app.trajectory_page.follow,
        "k takes the reader off the tail"
    );
    assert!(
        app.trajectory_page.scroll > 0,
        "and pins the offset it was at, not zero: {}",
        app.trajectory_page.scroll
    );
}

#[test]
fn a_reader_off_the_tail_keeps_their_place_when_records_arrive() {
    let mut app = app_with_a_run();
    for index in 0..40 {
        app.handle_agent_update(queued(&format!("bulk-{index}"), "bash", "true"));
    }
    app.open_trajectory_page();
    press(&mut app, crossterm::event::KeyCode::Char('k'));
    let scroll = app.trajectory_page.scroll;
    let selected = app.trajectory_page.selected;

    app.handle_agent_update(queued("late", "bash", "true"));

    assert!(
        !app.trajectory_page.follow,
        "an append does not re-arm following"
    );
    assert_eq!(
        app.trajectory_page.selected, selected,
        "and it does not move the reader's selection"
    );
    assert!(
        app.trajectory_page.scroll <= scroll,
        "nor scroll them forward: {} then {}",
        scroll,
        app.trajectory_page.scroll
    );
}

#[test]
fn a_live_tail_update_repaints_the_page_without_appending() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    app.handle_agent_update(queued("call-9", "bash", "cargo test"));
    let count = app.trajectory.len();

    app.handle_agent_update(tact_protocol::AgentUpdate::ToolProgress {
        run_id: None,
        tool_id: "call-9".into(),
        chunks: vec![tact_protocol::ToolOutputChunk::stdout(
            "compiling tact_host\n",
        )],
    });

    assert_eq!(app.trajectory.len(), count, "a tail update appends no row");
    let text = render_app_text(&mut app, 100, 30);
    assert!(
        text.contains("compiling tact_host"),
        "but it is on screen:\n{text}"
    );
}

#[test]
fn the_log_still_gets_every_update_the_page_consumed() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    let log_len = app.log.items.len();

    app.handle_agent_update(tact_protocol::AgentUpdate::Info("still logged".into()));

    assert!(
        app.log.items.len() > log_len,
        "the page observes; it must not intercept the log's own stream"
    );
}

/// The wheel is the pointer's `j`/`k`: one record per notch, in both
/// directions, without leaving the page or touching the log.
#[test]
fn the_wheel_moves_the_selection_one_record_per_notch() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    render_app_text(&mut app, 80, 30);
    let count = app.trajectory.len();
    assert!(count >= 3, "the fixture needs a ledger to scroll");

    press(&mut app, crossterm::event::KeyCode::Char('g'));
    let first = app.trajectory_page.selected.expect("selection");

    wheel(&mut app, crossterm::event::MouseEventKind::ScrollDown);
    let second = app.trajectory_page.selected.expect("selection");
    assert_ne!(second, first, "a notch down moves one record");
    assert!(
        !app.trajectory_page.follow,
        "leaving the tail stops following"
    );

    wheel(&mut app, crossterm::event::MouseEventKind::ScrollUp);
    assert_eq!(
        app.trajectory_page.selected,
        Some(first),
        "a notch up moves back"
    );
}

/// Arriving at the tail with the wheel re-arms following, so the next append
/// keeps the newest record in view instead of freezing a row short.
#[test]
fn the_wheel_re_arms_following_at_the_tail() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    render_app_text(&mut app, 80, 30);

    press(&mut app, crossterm::event::KeyCode::Char('g'));
    assert!(!app.trajectory_page.follow);

    // One notch per record, so walk the whole ledger rather than guessing.
    for _ in 0..app.trajectory.len() {
        wheel(&mut app, crossterm::event::MouseEventKind::ScrollDown);
    }

    assert!(
        app.trajectory_page.follow,
        "the wheel reached the tail, which is what following means"
    );
    let last = app.trajectory.records().last().expect("a record").id;
    assert_eq!(app.trajectory_page.selected, Some(last));
}

/// The page owns the main area, so the wheel belongs to it wherever the pointer
/// is — including over the border, which the ledger does not paint.
///
/// The log here is deliberately long enough to scroll: with a short log the
/// assertion would hold even if the page never got the event, because a log
/// that cannot scroll cannot move.
#[test]
fn the_wheel_over_the_page_never_scrolls_the_log() {
    let mut app = make_app();
    for index in 0..120 {
        app.add_system_message(format!("log line {index}"));
    }
    app.begin_trajectory_turn("go");
    app.open_trajectory_page();
    render_app_text(&mut app, 80, 30);
    // Sanity: this log really can scroll, so the assertion below has teeth.
    app.scroll_log_up(1);
    let scrolled = app.log_scroll.visual_top;
    app.scroll_log_down(1);
    let before = app.log_scroll.visual_top;

    // (0, 0) is the page's outer corner, outside the ledger's own body.
    wheel_at(&mut app, crossterm::event::MouseEventKind::ScrollDown, 0, 0);

    assert_eq!(
        app.log_scroll.visual_top, before,
        "the log underneath must not move (it had {scrolled} rows above it)"
    );
    assert!(app.show_trajectory, "the wheel does not close the page");
}

/// The page outranks a stale Log rect.
///
/// The Log's hit area is only refreshed on frames that paint the log; while the
/// page is up it holds the last rect from before it opened. Nothing clears it,
/// so the dispatch order is what keeps the wheel on the page — a `show_trajectory`
/// arm placed after `hit.in_log` would hand every notch to the hidden log.
#[test]
fn the_page_outranks_a_stale_log_rect() {
    let mut app = make_app();
    for index in 0..120 {
        app.add_system_message(format!("log line {index}"));
    }
    app.begin_trajectory_turn("go");
    app.handle_agent_update(queued("call-1", "read_file", "src/lib.rs"));
    app.handle_agent_update(finished("call-1", "read_file", Some(1_200_000)));
    app.open_trajectory_page();
    render_app_text(&mut app, 80, 30);

    // Reproduce the real world: the Log rect still describes this frame's
    // main area even though the page paints over it.
    let page = app
        .mouse
        .area(crate::widgets::state::SurfaceId::TrajectoryPage);
    app.mouse
        .set_area(crate::widgets::state::SurfaceId::Log, page);
    press(&mut app, crossterm::event::KeyCode::Char('g'));
    let first = app.trajectory_page.selected;
    let log_before = app.log_scroll.visual_top;

    wheel_at(
        &mut app,
        crossterm::event::MouseEventKind::ScrollDown,
        page.x + 1,
        page.y + 1,
    );

    assert_ne!(
        app.trajectory_page.selected, first,
        "down: the page must consume the notch, not the log rect under it"
    );
    assert_eq!(
        app.log_scroll.visual_top, log_before,
        "down: the hidden log must not scroll"
    );

    wheel_at(
        &mut app,
        crossterm::event::MouseEventKind::ScrollUp,
        page.x + 1,
        page.y + 1,
    );
    assert_eq!(
        app.trajectory_page.selected, first,
        "up: the page must consume the notch, not the log rect under it"
    );
    assert_eq!(
        app.log_scroll.visual_top, log_before,
        "up: the hidden log must not scroll"
    );
}

/// Closing the page hands the wheel back: with no trajectory rect the same
/// notch must reach the log again.
#[test]
fn closing_the_page_returns_the_wheel_to_the_log() {
    let mut app = app_with_a_run();
    app.open_trajectory_page();
    render_app_text(&mut app, 80, 30);
    let selected = app.trajectory_page.selected;

    app.close_trajectory_page();
    render_app_text(&mut app, 80, 30);
    wheel_at(&mut app, crossterm::event::MouseEventKind::ScrollDown, 0, 0);

    assert_eq!(
        app.trajectory_page.selected, selected,
        "a closed page ignores the wheel"
    );
}

/// A resumed session rebuilds the ledger instead of opening it empty.
///
/// The stored transcript is the only durable record of what a run did, and the
/// log deliberately drops the blocks the ledger exists to hold, so restoring
/// from the messages is the only way `/trajectory` can survive a restart.
#[test]
fn a_resumed_session_restores_the_ledger() {
    use tact::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin, ToolResultStatus};

    let mut app = make_app();

    let prompt = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::User,
        vec![AgentContentBlock::Text {
            text: "why does the retry test fail".into(),
        }],
    );
    let call = AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        vec![
            AgentContentBlock::Thinking {
                thinking: "look at the fixture".into(),
                signature: String::new(),
            },
            AgentContentBlock::ToolUse {
                id: "call-1".into(),
                name: "read_file".into(),
                input: serde_json::json!({ "path": "src/lib.rs" }),
            },
        ],
    );
    let result = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::ToolResult,
        vec![AgentContentBlock::ToolResult {
            tool_use_id: "call-1".into(),
            content: "fn main() {}".into(),
            status: ToolResultStatus::Success,
        }],
    );

    app.restore_trajectory(&[prompt, call, result]);

    let kinds: Vec<_> = app.trajectory.records().iter().map(|r| r.kind).collect();
    assert_eq!(
        kinds,
        vec![
            TrajectoryKind::User,
            TrajectoryKind::Thinking,
            TrajectoryKind::Tool,
        ],
        "each stored block becomes exactly one record"
    );
    let tool = &app.trajectory.records()[2];
    assert_eq!(tool.tool_name.as_deref(), Some("read_file"));
    assert_eq!(tool.status, Some(TrajectoryStatus::Success));
    assert_eq!(
        tool.output.as_deref(),
        Some("fn main() {}"),
        "the result is attached by tool id"
    );
    assert_eq!(
        tool.text, "read_file src/lib.rs",
        "the row names the file the call was about"
    );
}

/// A result whose status was never recorded must not claim success.
///
/// Rows written before the status column existed decode as `Unknown`; showing
/// `Success` for them would tell the reader a call worked when nothing recorded
/// that it did.
#[test]
fn a_restored_result_with_unknown_status_shows_no_status() {
    use tact::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin, ToolResultStatus};

    let mut app = make_app();
    let call = AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        vec![AgentContentBlock::ToolUse {
            id: "call-1".into(),
            name: "bash".into(),
            input: serde_json::json!({ "command": "cargo test" }),
        }],
    );
    let result = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::ToolResult,
        vec![AgentContentBlock::ToolResult {
            tool_use_id: "call-1".into(),
            content: "output".into(),
            status: ToolResultStatus::Unknown,
        }],
    );

    app.restore_trajectory(&[call, result]);

    let tool = app
        .trajectory
        .records()
        .iter()
        .find(|record| record.kind == TrajectoryKind::Tool)
        .expect("the call is in the ledger");
    assert_eq!(tool.status, None, "an unrecorded status stays unrecorded");
    assert_eq!(tool.output.as_deref(), Some("output"));
}

/// Redacted reasoning keeps its row but not its body.
#[test]
fn restored_redacted_thinking_keeps_the_row_without_the_text() {
    use tact::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin};

    let mut app = make_app();
    let message = AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        vec![AgentContentBlock::RedactedThinking {
            data: "opaque".into(),
        }],
    );

    app.restore_trajectory(&[message]);

    let thinking = app
        .trajectory
        .records()
        .iter()
        .find(|record| record.kind == TrajectoryKind::Thinking)
        .expect("the block is recorded");
    assert_eq!(thinking.text, "(redacted)");
    assert!(
        !thinking
            .output
            .as_deref()
            .unwrap_or_default()
            .contains("opaque"),
        "the redacted payload must not be re-shown"
    );
}

/// Injected context is evidence, not a prompt: it must not open a second turn.
#[test]
fn restored_injected_context_does_not_open_a_turn() {
    use tact::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin};

    let mut app = make_app();
    let prompt = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::User,
        vec![AgentContentBlock::Text {
            text: "real turn".into(),
        }],
    );
    let injected = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::InjectedContext {
            source: "plugin codex".into(),
        },
        vec![AgentContentBlock::Text {
            text: "briefing body".into(),
        }],
    );

    app.restore_trajectory(&[prompt, injected]);

    let context = app
        .trajectory
        .records()
        .iter()
        .find(|record| record.kind == TrajectoryKind::Context)
        .expect("the briefing is recorded");
    assert!(context.text.contains("briefing"), "{}", context.text);
    let turns: Vec<u32> = app
        .trajectory
        .records()
        .iter()
        .map(|record| record.turn)
        .collect();
    assert_eq!(
        turns,
        vec![1, 1],
        "both records belong to the one real turn: {turns:?}"
    );
}

/// The startup wiring: a saved session is read back and the ledger rebuilt.
///
/// The tests above call `restore_trajectory` directly, which proves the mapping
/// but not that a restart reaches it. This one writes a transcript to a real
/// store and runs the same entry point `run_tui` does, so removing the call
/// from `restore_session_views` fails here.
#[tokio::test]
async fn restarting_a_saved_session_rebuilds_the_ledger() {
    use tact::{AgentContentBlock, AgentMessage, AgentRole, MessageOrigin, ToolResultStatus};

    let tmp = tempfile::tempdir().expect("tempdir");
    let db = tmp.path().join("tact.db");
    let store = tact::store::open_sqlite_session_store(&db)
        .await
        .expect("store opens");
    store
        .ensure_session_row("session-1", "/tmp", "")
        .await
        .expect("session row");

    let user = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::User,
        vec![AgentContentBlock::Text {
            text: "audit the retry path".into(),
        }],
    );
    let call = AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        vec![AgentContentBlock::ToolUse {
            id: "call-1".into(),
            name: "read_file".into(),
            input: serde_json::json!({ "path": "src/retry.rs" }),
        }],
    );
    let result = AgentMessage::new(
        AgentRole::User,
        MessageOrigin::ToolResult,
        vec![AgentContentBlock::ToolResult {
            tool_use_id: "call-1".into(),
            content: "fn retry() {}".into(),
            status: ToolResultStatus::Success,
        }],
    );
    for (ordinal, message) in [user, call, result].iter().enumerate() {
        store
            .append_agent_message("session-1", message, ordinal as i64)
            .await
            .expect("append");
    }

    let mut app = make_app();
    app.session_id = "session-1".into();
    app.session_store = Some(store);
    app.restore_session_views().await;

    let tool = app
        .trajectory
        .records()
        .iter()
        .find(|record| record.kind == TrajectoryKind::Tool)
        .expect("the saved tool call is in the ledger");
    assert_eq!(tool.tool_name.as_deref(), Some("read_file"));
    assert_eq!(tool.status, Some(TrajectoryStatus::Success));
    assert_eq!(tool.output.as_deref(), Some("fn retry() {}"));
    assert!(
        app.trajectory
            .records()
            .iter()
            .any(|record| record.kind == TrajectoryKind::User),
        "and so is the prompt that opened the turn"
    );
}
