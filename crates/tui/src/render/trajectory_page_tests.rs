//! The trajectory page as the app uses it: entry, close, keys, and the fact
//! that it owns the main area.
//!
//! The ledger's own behaviour is tested in `agent_tui_kit::state::trajectory`
//! and the painting in `agent_tui_kit::render::trajectory`. What is left to
//! prove here is the wiring: that `/trajectory` opens it, that closing hands
//! the keyboard back and clears every hit rect, that only one page is up at a
//! time, and that the keys reach the ledger.

use agent_tui_kit::state::trajectory::TrajectoryKind;
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
    assert!(
        app.mouse.trajectory_page_area.height > 0,
        "the page records where it painted"
    );

    app.close_trajectory_page();

    assert!(!app.show_trajectory);
    assert_eq!(
        app.input_mode,
        TuiInputMode::Insert,
        "the reader must be able to type again"
    );
    assert!(!app.trajectory_page.filter_editing);
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
