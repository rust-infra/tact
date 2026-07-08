//! Render gap tests: P0/P1 coverage for previously untested paths.

use super::plan::render_plan_panel;
use super::test_harness::{buffer_text, make_app, render_app_text, render_main_area_text};
use crate::widgets::state::{App, FocusedPanel, InputMode, Status};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::collections::HashMap;
use std::time::Duration;
use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus};

fn seed_write_file_finished(app: &mut App, path: &str, content: &str) {
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "write file",
        "write_file",
        "wf1",
        HashMap::from([("path".to_string(), path.to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted(
        0,
        "wf1".into(),
        "write_file".into(),
        path.into(),
    ));
    app.handle_agent_update(AgentUpdate::StepFinished(
        0,
        "wf1".into(),
        StepResult {
            tool: "write_file".into(),
            arg_summary: path.into(),
            arg_full: Some(path.into()),
            status: StepStatus::Success,
            message: "written".into(),
            detail: Some(content.into()),
            duration_us: Some(50),
            permission_label: None,
        },
    ));
}

fn seed_bash_finished(app: &mut App, command: &str, output: &str) {
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "run shell",
        "bash",
        "bash1",
        HashMap::from([("command".to_string(), command.to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted(
        0,
        "bash1".into(),
        "bash".into(),
        command.into(),
    ));
    app.handle_agent_update(AgentUpdate::StepFinished(
        0,
        "bash1".into(),
        StepResult {
            tool: "bash".into(),
            arg_summary: command.into(),
            arg_full: Some(command.into()),
            status: StepStatus::Success,
            message: "ok".into(),
            detail: Some(output.into()),
            duration_us: Some(100),
            permission_label: None,
        },
    ));
}

fn open_last_tool_popup(app: &mut App) {
    let phys_idx = app.tools.blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup(phys_idx);
}

// --- P0: diff gutter, bash popup, inline cards, WaitingForUser ---

#[test]
fn write_file_diff_popup_shows_gutter() {
    let mut app = make_app();
    let file = std::env::temp_dir().join(format!("tact-diff-gutter-{}.rs", std::process::id()));
    std::fs::write(&file, "fn gutter_test() {}").expect("write temp");
    let path = file.to_string_lossy().into_owned();

    seed_write_file_finished(&mut app, &path, "fn gutter_test() {}");
    open_last_tool_popup(&mut app);

    let text = render_main_area_text(&mut app, 100, 30);
    let _ = std::fs::remove_file(&file);

    assert!(
        app.tools.popup.as_ref().is_some_and(|p| p.use_diff_gutter),
        "write_file popup should enable diff gutter"
    );
    assert!(
        text.contains("gutter_test") || text.contains('+'),
        "write_file diff popup should render content or + gutter, got:\n{text}"
    );
}

#[test]
fn bash_tool_popup_shows_command_output() {
    let mut app = make_app();
    seed_bash_finished(&mut app, "echo hello", "hello\n");
    open_last_tool_popup(&mut app);

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(
        text.contains("echo hello") || text.contains("hello"),
        "bash popup should show command and output, got:\n{text}"
    );
}

#[test]
fn log_renders_collapsed_thinking_card() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::ThinkingChunk("Analyzing the problem…".into()));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(" considering options.".into()));
    app.handle_agent_update(AgentUpdate::StreamChunk("Final answer.".into()));

    let text = render_main_area_text(&mut app, 100, 28);

    assert!(
        !app.thinking.blocks.is_empty(),
        "thinking block should be closed after stream"
    );
    assert!(
        text.contains("Thinking") || text.contains("Analyzing") || text.contains("considering"),
        "collapsed thinking card should render in log, got:\n{text}"
    );
}

#[test]
fn log_renders_streamed_code_block_card() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```rust\nfn code_card_test() {}\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 28);

    assert!(
        !app.code_blocks.is_empty(),
        "stream should create a code block"
    );
    assert!(
        text.contains("rust") || text.contains("code_card_test"),
        "inline code card should render in log, got:\n{text}"
    );
}

#[test]
fn full_frame_waiting_for_user_shows_approval_banner() {
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.input_mode = InputMode::Normal;
    app.status = Status::WaitingForUser {
        prompt: "Allow rm -rf /tmp/test?".into(),
        step_index: 0,
        approval_tx: tx,
    };

    let text = render_app_text(&mut app, 100, 28);

    assert!(
        text.contains("Allow rm -rf"),
        "full frame should show approval banner in input area, got:\n{text}"
    );
}

// --- P1: Normal mode, plan states, popup scroll, file picker highlight, focus ---

#[test]
fn full_frame_normal_mode_status_bar() {
    let mut app = make_app();
    app.input_mode = InputMode::Normal;

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("NORMAL"),
        "normal mode should appear in status bar, got:\n{text}"
    );
}

#[test]
fn plan_panel_shows_multiple_steps_with_one_running() {
    let mut app = make_app();
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "read first",
        "read_file",
        "r1",
        HashMap::from([("path".to_string(), "a.txt".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "read second",
        "read_file",
        "r2",
        HashMap::from([("path".to_string(), "b.txt".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted(
        0,
        "r1".into(),
        "read_file".into(),
        "a.txt".into(),
    ));
    app.handle_agent_update(AgentUpdate::StepFinished(
        0,
        "r1".into(),
        StepResult {
            tool: "read_file".into(),
            arg_summary: "a.txt".into(),
            arg_full: None,
            status: StepStatus::Success,
            message: "ok".into(),
            detail: None,
            duration_us: Some(1),
            permission_label: None,
        },
    ));
    app.handle_agent_update(AgentUpdate::StepStarted(
        1,
        "r2".into(),
        "read_file".into(),
        "b.txt".into(),
    ));

    let backend = TestBackend::new(60, 12);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_plan_panel(frame, Rect::new(0, 0, 60, 12), &mut app))
        .expect("draw");

    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("read first") && text.contains("read second"),
        "plan panel should list all steps, got:\n{text}"
    );
}

#[test]
fn plan_panel_lists_failed_step_description() {
    let mut app = make_app();
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "failing read",
        "read_file",
        "fail1",
        HashMap::from([("path".to_string(), "nope.txt".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted(
        0,
        "fail1".into(),
        "read_file".into(),
        "nope.txt".into(),
    ));
    app.handle_agent_update(AgentUpdate::StepFailed(0, "fail1".into(), "file not found".into()));

    let text = render_app_text(&mut app, 120, 30);
    assert!(
        text.contains("failing read") || text.contains("nope.txt"),
        "failed step should remain visible in plan/log, got:\n{text}"
    );
}

#[test]
fn diff_popup_scroll_skips_leading_lines() {
    let mut app = make_app();
    let lines: String = (1..=20).map(|n| format!("line-{n}")).collect::<Vec<_>>().join("\n");
    seed_write_file_finished(&mut app, "scroll.rs", &lines);
    open_last_tool_popup(&mut app);
    if let Some(popup) = app.tools.popup.as_mut() {
        popup.scroll = 8;
        popup.file_path = None;
        popup.inline_content = Some(lines);
        popup.cached_content = None;
    }

    let text = render_main_area_text(&mut app, 100, 20);

    assert!(
        !text.contains("line-1\n") && !text.ends_with("line-1"),
        "scrolled popup should skip early lines, got:\n{text}"
    );
    assert!(
        text.contains("line-9") || text.contains("line-10"),
        "scrolled popup should show later lines, got:\n{text}"
    );
}

#[test]
fn code_popup_scroll_skips_leading_lines() {
    use crate::widgets::state::{CodeBlock, CodePopup};
    use ratatui::text::Line;

    let mut app = make_app();
    let content: String = (1..=15).map(|n| format!("row {n}")).collect::<Vec<_>>().join("\n");
    let styled: Vec<Line<'static>> = content
        .lines()
        .map(|l| Line::from(l.to_string()))
        .collect();
    app.code_blocks.push(CodeBlock {
        start_idx: 0,
        end_idx: styled.len(),
        lang: "rust".into(),
        content: content.clone(),
        styled,
    });
    app.code_popup = Some(CodePopup {
        block_idx: 0,
        lang: "rust".into(),
        scroll: 5,
    });

    let text = render_main_area_text(&mut app, 100, 18);

    assert!(
        text.contains("row 6") || text.contains("row 7"),
        "scrolled code popup should show later rows, got:\n{text}"
    );
}

#[test]
fn file_picker_highlights_selected_row() {
    let mut app = make_app();
    app.input_mode = InputMode::FilePicker;
    app.file_picker
        .set(vec!["src/a.rs".into(), "src/b.rs".into()]);
    app.file_picker.selected = 1;

    let text = render_app_text(&mut app, 80, 24);

    assert!(
        text.contains("▶") && text.contains("b.rs"),
        "selected file picker row should show arrow marker, got:\n{text}"
    );
}

#[test]
fn bottom_bar_shows_plan_focus_indicator() {
    let mut app = make_app();
    app.focused_panel = FocusedPanel::Plan;

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("[Plan]") || text.contains("Plan"),
        "plan focus should appear in bottom bar, got:\n{text}"
    );
}

#[test]
fn narrow_terminal_renders_without_empty_frame() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Narrow layout.".into()));

    let text = render_app_text(&mut app, 35, 18);

    assert!(
        !text.trim().is_empty() && text.contains("Narrow"),
        "narrow terminal should still render content, got:\n{text}"
    );
}

#[test]
fn thinking_popup_scroll_shows_later_lines() {
    use crate::widgets::state::{ThinkingBlock, ThinkingPopup};
    use ratatui::text::Line;

    let mut app = make_app();
    let markdown: Vec<Line> = (1..=12)
        .map(|n| Line::from(format!("reason-{n}")))
        .collect();
    app.raw_messages.push("Thinking".into());
    app.thinking.blocks.push(ThinkingBlock {
        title_idx: 0,
        end_idx: 11,
        scroll_offset: 0,
        cached_preview: vec!["reason-1".into()],
        cached_markdown: markdown,
        elapsed: Duration::from_millis(5),
    });
    app.thinking.popup = Some(ThinkingPopup {
        block_idx: 0,
        title: "Thinking".into(),
        scroll: 6,
    });

    let text = render_main_area_text(&mut app, 100, 16);

    assert!(
        text.contains("reason-7") || text.contains("reason-8"),
        "scrolled thinking popup should show later lines, got:\n{text}"
    );
}

// --- P2: Done timeout, status bar after expire ---

#[test]
fn done_status_reverts_to_idle_after_two_seconds() {
    let mut app = make_app();
    app.status = Status::Done;
    app.task_done_time = Some(chrono::Local::now() - chrono::Duration::seconds(3));

    app.maybe_expire_done_status();

    assert!(matches!(app.status, Status::Idle));
    assert!(app.task_done_time.is_none());
}

#[test]
fn done_status_persists_within_two_seconds() {
    let mut app = make_app();
    app.status = Status::Done;
    app.task_done_time = Some(chrono::Local::now());

    app.maybe_expire_done_status();

    assert!(matches!(app.status, Status::Done));
}

#[test]
fn status_bar_shows_idle_after_done_expires() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));
    app.task_done_time = Some(chrono::Local::now() - chrono::Duration::seconds(3));
    app.maybe_expire_done_status();

    let text = render_app_text(&mut app, 100, 24);
    assert!(
        text.contains("NORMAL") || text.contains("Idle") || !text.contains("Task completed"),
        "expired done should repaint idle-ish status bar, got:\n{text}"
    );
}

// --- Handler-adjacent render: Planning, NeedApproval, edit_file ---

#[test]
fn full_frame_planning_status_renders_in_status_bar() {
    let mut app = make_app();
    app.status = Status::Planning;
    app.input_mode = InputMode::Insert;

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("Planning"),
        "planning status should appear in status bar, got:\n{text}"
    );
}

#[test]
fn need_approval_update_sets_waiting_and_renders_banner() {
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();

    app.handle_agent_update(AgentUpdate::NeedApproval(
        "Allow edit_file on lib.rs?".into(),
        0,
        tx,
    ));

    assert!(matches!(app.status, Status::WaitingForUser { .. }));
    assert!(matches!(app.input_mode, InputMode::Normal));

    let text = render_app_text(&mut app, 100, 28);
    assert!(
        text.contains("Allow edit_file") || text.contains("lib.rs"),
        "NeedApproval should render approval prompt in full frame, got:\n{text}"
    );
}

#[test]
fn full_frame_edit_file_tool_shows_in_log() {
    let mut app = make_app();
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "patch lib",
        "edit_file",
        "edit1",
        HashMap::from([
            ("path".to_string(), "lib.rs".to_string()),
            ("old_text".to_string(), "fn old()".to_string()),
            ("new_text".to_string(), "fn new()".to_string()),
        ]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted(
        0,
        "edit1".into(),
        "edit_file".into(),
        "lib.rs".into(),
    ));
    app.handle_agent_update(AgentUpdate::StepFinished(
        0,
        "edit1".into(),
        StepResult {
            tool: "edit_file".into(),
            arg_summary: "lib.rs".into(),
            arg_full: Some("lib.rs".into()),
            status: StepStatus::Success,
            message: "patched".into(),
            detail: Some("- fn old()\n+ fn new()".into()),
            duration_us: Some(200),
            permission_label: None,
        },
    ));

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("edit_file") || text.contains("lib.rs") || text.contains("fn new"),
        "edit_file tool card should render in log, got:\n{text}"
    );
}

#[test]
fn balance_update_renders_in_bottom_bar() {
    use tact_protocol::{BalanceEntry, BalanceInfo};

    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::Balance(BalanceInfo {
        is_available: true,
        balance_infos: vec![BalanceEntry {
            currency: "USD".into(),
            total_balance: "42.00".into(),
            granted_balance: "40.00".into(),
            topped_up_balance: "2.00".into(),
        }],
    }));

    let text = render_app_text(&mut app, 120, 30);
    assert!(
        text.contains("USD") || text.contains("42.00"),
        "Balance update should render in bottom bar, got:\n{text}"
    );
}

#[test]
fn flash_msg_renders_warning_in_status_bar() {
    let mut app = make_app();
    app.flash_msg = Some((
        "Balance query failed: timeout".into(),
        std::time::Instant::now(),
    ));

    let text = render_app_text(&mut app, 100, 24);
    assert!(
        text.contains("Balance query failed") || text.contains('⚠'),
        "flash_msg should override status bar, got:\n{text}"
    );
}

#[test]
fn flash_msg_clears_after_three_seconds() {
    let mut app = make_app();
    app.flash_msg = Some((
        "stale warning".into(),
        std::time::Instant::now() - std::time::Duration::from_secs(4),
    ));

    app.maybe_clear_flash_msg();

    assert!(app.flash_msg.is_none());
}

#[test]
fn flash_msg_persists_within_three_seconds() {
    let mut app = make_app();
    app.flash_msg = Some(("fresh warning".into(), std::time::Instant::now()));

    app.maybe_clear_flash_msg();

    assert!(app.flash_msg.is_some());
}
