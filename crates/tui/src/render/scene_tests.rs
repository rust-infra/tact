//! End-to-end render scene tests (TestBackend + full frame layout).

use super::plan::render_plan_panel;
use super::render_status_bar;
use super::test_harness::{buffer_text, make_app, render_app_text};
use crate::widgets::state::{App, HistoryEntry, InputMode, Status};
use ratatui::{Terminal, backend::TestBackend, layout::Rect};
use std::collections::HashMap;
use tact_protocol::{AgentErrorKind, AgentUpdate, PlanStep, StepResult, StepStatus};

fn seed_executing_read_step(app: &mut App) {
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "read config",
        "read_file",
        "tool_read_1",
        HashMap::from([("path".to_string(), "config.toml".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "tool_read_1".into(),
        tool_name: "read_file".into(),
        arg_summary: "config.toml".into(),
        arg_full: "config.toml".into(),
    });
}

#[test]
fn full_frame_idle_renders_status_and_input_placeholder() {
    let mut app = make_app();
    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("Type a task") || text.contains("Idle"),
        "idle frame should show placeholder or idle status, got:\n{text}"
    );
}

#[test]
fn full_frame_executing_renders_plan_and_tool() {
    let mut app = make_app();
    seed_executing_read_step(&mut app);

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("Execution Plan") || text.contains("read config"),
        "executing frame should show plan panel, got:\n{text}"
    );
    assert!(
        text.contains("read_file") || text.contains("config.toml"),
        "executing frame should show active tool, got:\n{text}"
    );
}

#[test]
fn full_frame_log_only_without_plan_panel() {
    let mut app = make_app();
    app.plan.visible = false;
    app.handle_agent_update(AgentUpdate::StreamChunk("Log-only content.".into()));

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        !text.contains("Execution Plan"),
        "log-only mode should not render plan title, got:\n{text}"
    );
    assert!(
        text.contains("Log-only content"),
        "stream text should appear in log, got:\n{text}"
    );
}

#[test]
fn full_frame_failed_tool_shows_in_log() {
    let mut app = make_app();
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "read missing",
        "read_file",
        "tool_fail",
        HashMap::from([("path".to_string(), "missing.txt".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "tool_fail".into(),
        tool_name: "read_file".into(),
        arg_summary: "missing.txt".into(),
        arg_full: "missing.txt".into(),
    });
    app.handle_agent_update(AgentUpdate::StepFinished {
        idx: 0,
        tool_id: "tool_fail".into(),
        result: StepResult {
            tool: "read_file".into(),
            arg_summary: "missing.txt".into(),
            arg_full: None,
            status: StepStatus::Failed,
            message: "file not found".into(),
            detail: Some("No such file".into()),
            duration_us: Some(500),
            permission_label: None,
        },
    });

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("read_file") || text.contains("missing.txt"),
        "failed tool card should render, got:\n{text}"
    );
}

#[test]
fn full_frame_stream_and_task_complete() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("Final answer text.".into()));
    app.handle_agent_update(AgentUpdate::TaskComplete("Final answer text.".into()));

    assert!(matches!(app.status, Status::Done));

    let text = render_app_text(&mut app, 100, 24);
    assert!(
        text.contains("Final answer text"),
        "completed stream should remain visible, got:\n{text}"
    );
}

#[test]
fn full_frame_thinking_then_stream() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::ThinkingChunk("Let me think…".into()));
    app.handle_agent_update(AgentUpdate::StreamChunk("Therefore: 42".into()));

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("Therefore: 42"),
        "stream after thinking should render, got:\n{text}"
    );
}

#[test]
fn full_frame_fatal_error_message() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::Error(AgentErrorKind::Other(
        "provider timeout".into(),
    )));

    let text = render_app_text(&mut app, 100, 24);
    assert!(
        text.contains("provider timeout")
            || app
                .raw_messages
                .iter()
                .any(|m| m.contains("provider timeout")),
        "fatal error should appear in frame, got:\n{text}"
    );
}

#[test]
fn full_frame_info_cancel_message() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::Info("Cancelling...".into()));

    let text = render_app_text(&mut app, 100, 20);
    assert!(
        text.contains("Cancelling"),
        "info message should render in log, got:\n{text}"
    );
}

#[test]
fn full_frame_token_usage_in_bottom_bar() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::TokenUsage(tact_protocol::TokenUsageInfo {
        prompt: 100,
        completion: 50,
        total: 150,
        prompt_cache_hit_tokens: 10,
        prompt_cache_miss_tokens: 90,
        reasoning_tokens: 5,
    }));
    app.handle_agent_update(AgentUpdate::ModelInfo(tact_protocol::ModelCallParams {
        model: "mock-model".into(),
        max_tokens: 8192,
        thinking_budget: Some(0),
        reasoning_effort: None,
        extra_body: None,
    }));

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("150") && text.contains("mock-model"),
        "bottom bar should show token total and model, got:\n{text}"
    );
}

#[test]
fn full_frame_select_popup_overlays() {
    let mut app = make_app();
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.handle_agent_update(AgentUpdate::RequestSelect {
        prompt: "Allow bash?".into(),
        options: vec!["Allow once".into(), "Deny".into()],
        respond: tx,
    });

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("Allow bash") || text.contains("Allow once"),
        "select popup should overlay frame, got:\n{text}"
    );
}

#[test]
fn full_frame_search_mode_command_line() {
    let mut app = make_app();
    app.input_mode = InputMode::Search;
    app.cmd_line = "find pattern".into();

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("find pattern"),
        "search cmd_line should render, got:\n{text}"
    );
}

#[test]
fn full_frame_insert_mode_shows_typed_input() {
    let mut app = make_app();
    app.input_mode = InputMode::Insert;
    app.input = "fix the bug in main.rs".into();
    app.input_cursor = app.input.len();

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("fix the bug"),
        "insert mode should show typed input, got:\n{text}"
    );
}

#[test]
fn full_frame_help_panel_replaces_main() {
    let mut app = make_app();
    app.show_help = true;

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("Keyboard Shortcuts"),
        "help panel should render shortcuts header, got:\n{text}"
    );
}

#[test]
fn full_frame_history_panel_lists_tasks() {
    let mut app = make_app();
    app.show_history = true;
    app.task_history.push(HistoryEntry {
        task: "refactor auth module".into(),
        timestamp: "12:00:00".into(),
        summary: "✅ Done".into(),
    });

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("refactor auth") || text.contains("12:00:00"),
        "history panel should list tasks, got:\n{text}"
    );
}

#[test]
fn plan_panel_direct_render_shows_steps() {
    let mut app = make_app();
    app.plan.visible = true;
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "write tests",
        "write_file",
        "w1",
        HashMap::from([("path".to_string(), "test.rs".to_string())]),
    )));

    let backend = TestBackend::new(60, 10);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_plan_panel(frame, Rect::new(0, 0, 60, 10), &mut app))
        .expect("draw");

    let text = buffer_text(terminal.backend().buffer());
    assert!(
        text.contains("write tests") || text.contains("write_file"),
        "plan panel should list step, got:\n{text}"
    );
}

#[test]
fn status_bar_executing_shows_progress_hint() {
    let mut app = make_app();
    seed_executing_read_step(&mut app);

    let backend = TestBackend::new(100, 1);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            render_status_bar(frame, Rect::new(0, 0, 100, 1), &app);
        })
        .expect("draw");

    let text = buffer_text(terminal.backend().buffer());
    assert!(
        !text.trim().is_empty(),
        "executing status bar should not be blank, got:\n{text}"
    );
}
