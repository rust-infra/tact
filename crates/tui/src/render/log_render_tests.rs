//! Log panel render coverage: P0 interaction, P1 content shapes, P2 chrome/edge cases.

use super::test_harness::{
    buffer_has_modifier, make_app, render_log_panel_terminal, render_log_panel_text,
};
use crate::widgets::state::{App, LogSelection, RawMessageType, Status};
use ratatui::style::Modifier;
use std::collections::HashMap;
use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus, ThinkingChunk};

fn seed_many_numbered_lines(app: &mut App, count: usize) {
    for i in 0..count {
        app.add_system_message(format!("log-row-{i:02}"));
    }
}

fn seed_tall_bash_tool(app: &mut App, line_count: usize) {
    app.plan.visible = true;
    let output: String = (1..=line_count)
        .map(|n| format!("bash-out-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "run shell",
        "bash",
        "bash-tall",
        HashMap::from([("command".to_string(), "seq".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "bash-tall".into(),
        tool_name: "bash".into(),
        arg_summary: "seq".into(),
        arg_full: "seq".into(),
    });
    app.handle_agent_update(AgentUpdate::StepFinished {
        idx: 0,
        tool_id: "bash-tall".into(),
        result: StepResult {
            tool: "bash".into(),
            arg_summary: "seq".into(),
            arg_full: Some("seq".into()),
            status: StepStatus::Success,
            message: "ok".into(),
            detail: Some(output),
            duration_us: Some(100),
            permission_label: None,
        },
    });
}

fn line_column_of(rendered: &str, needle: &str) -> Option<usize> {
    rendered.lines().find_map(|line| line.find(needle))
}

fn line_index_of(rendered: &str, needle: &str) -> Option<usize> {
    rendered.lines().position(|line| line.contains(needle))
}

// ── P0: selection, scroll ───────────────────────────────────────────────────

#[test]
fn log_line_selection_applies_reversed_modifier() {
    let mut app = make_app();
    app.add_system_message("select this entire line".into());
    let raw = app.raw_messages[0].clone();
    app.mouse.log_selection = Some(LogSelection::full_message(0, raw.len()));

    let terminal = render_log_panel_terminal(&mut app, 80, 16);
    assert!(
        buffer_has_modifier(terminal.backend().buffer(), Modifier::REVERSED),
        "line selection should apply REVERSED modifier in log buffer"
    );
}

#[test]
fn log_partial_selection_applies_reversed_modifier() {
    let mut app = make_app();
    app.add_system_message("alpha beta gamma".into());
    app.mouse.log_selection = Some(LogSelection::span(0, 6, 10));

    let terminal = render_log_panel_terminal(&mut app, 80, 16);
    assert!(
        buffer_has_modifier(terminal.backend().buffer(), Modifier::REVERSED),
        "word selection should apply REVERSED modifier in log buffer"
    );
}

#[test]
fn log_scroll_offset_hides_early_lines() {
    let mut app = make_app();
    seed_many_numbered_lines(&mut app, 40);

    app.log_scroll.offset = 0;
    let top = render_log_panel_text(&mut app, 60, 10);
    assert!(
        top.contains("log-row-00"),
        "at offset 0 the first row should be visible, got:\n{top}"
    );

    app.log_scroll.offset = u16::MAX;
    let bottom = render_log_panel_text(&mut app, 60, 10);
    assert!(
        !bottom.contains("log-row-00"),
        "scrolled to bottom should hide the first row, got:\n{bottom}"
    );
    assert!(
        bottom.contains("log-row-39") || bottom.contains("log-row-38"),
        "scrolled to bottom should show the last rows, got:\n{bottom}"
    );
}

// ── P1: message shapes, separators, wrap, stream ─────────────────────────────

#[test]
fn log_user_message_shows_prefix() {
    let mut app = make_app();
    app.add_user_message("hello from user".into());

    let text = render_log_panel_text(&mut app, 80, 16);
    assert!(
        text.contains("💬") && text.contains("hello from user"),
        "user messages should render with 💬 prefix, got:\n{text}"
    );
}

#[test]
fn log_mixed_categories_render_user_and_assistant() {
    let mut app = make_app();
    app.add_user_message("user task".into());
    app.handle_agent_update(AgentUpdate::StreamChunk("assistant reply".into()));

    let text = render_log_panel_text(&mut app, 80, 20);
    assert!(
        text.contains("user task") && text.contains("assistant reply"),
        "log should render both user and assistant content after category gap, got:\n{text}"
    );
    let user_line = line_index_of(&text, "user task").expect("user line");
    let assistant_line = line_index_of(&text, "assistant reply").expect("assistant line");
    assert!(
        assistant_line > user_line,
        "assistant message should render after user message (user={user_line}, assistant={assistant_line})"
    );
}

#[test]
fn log_task_end_separator_renders_dashed_rule() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.add_task_end_separator();

    let text = render_log_panel_text(&mut app, 60, 12);
    assert!(
        text.contains('─'),
        "task-end separator should render dashed rule, got:\n{text}"
    );
}

#[test]
fn log_thinking_title_shows_scroll_indicator_when_collapsed() {
    let mut app = make_app();
    for i in 1..=6 {
        app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(format!(
            "reason line {i}\n"
        ))));
    }
    app.handle_agent_update(AgentUpdate::StreamChunk("final answer".into()));

    let text = render_log_panel_text(&mut app, 100, 24);
    assert!(
        text.contains('↕') || text.contains("Thinking"),
        "collapsed thinking block with >3 lines should show scroll indicator or title, got:\n{text}"
    );
}

#[test]
fn active_thinking_card_renders_a_three_line_tail_without_source_rows() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
        "one\ntwo\nthree\nfour\n".into(),
    )));

    let text = render_log_panel_text(&mut app, 100, 24);
    assert!(text.contains("two") && text.contains("four"), "{text}");
    assert!(!text.contains("│ one"), "{text}");
    assert!(!text.contains("one\n"), "{text}");
}

#[test]
fn log_sys_tool_message_uses_extra_indent() {
    let mut app = make_app();
    app.append_msg(
        ratatui::text::Line::from("plain assistant"),
        "plain assistant".into(),
        RawMessageType::LLM,
    );
    app.append_msg(
        ratatui::text::Line::from("nested tool line"),
        "nested tool line".into(),
        RawMessageType::SysTool,
    );

    let text = render_log_panel_text(&mut app, 80, 12);
    let plain_x = line_column_of(&text, "plain assistant").expect("plain line");
    let nested_x = line_column_of(&text, "nested tool line").expect("nested line");

    assert!(
        nested_x > plain_x,
        "SysTool rows should indent further than LLM rows (plain={plain_x}, nested={nested_x})"
    );
}

#[test]
fn log_narrow_width_wraps_long_paragraph() {
    let mut app = make_app();
    app.add_system_message("word ".repeat(40));

    render_log_panel_text(&mut app, 100, 20);
    let wide_lines = app.log_scroll.visual_cache.len();

    render_log_panel_text(&mut app, 28, 20);
    let narrow_lines = app.log_scroll.visual_cache.len();

    assert!(
        narrow_lines > wide_lines,
        "narrow panel should produce more visual lines ({narrow_lines}) than wide ({wide_lines})"
    );
}

#[test]
fn log_stream_buffer_shows_in_progress_text() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("streaming partial".into()));

    let text = render_log_panel_text(&mut app, 80, 16);
    assert!(
        text.contains("streaming partial"),
        "in-progress stream buffer should render in log, got:\n{text}"
    );
    assert!(
        !app.stream.buffer.is_empty(),
        "stream buffer should remain until task completes"
    );
}

// ── P2: scrollbar, cache, tool viewport, spinner ────────────────────────────

#[test]
fn log_scrollbar_shows_when_content_overflows() {
    let mut app = make_app();
    seed_many_numbered_lines(&mut app, 50);

    let text = render_log_panel_text(&mut app, 60, 8);
    assert!(
        text.contains('█') || text.contains('│') || text.contains('▲') || text.contains('▼'),
        "overflowing log should render vertical scrollbar glyphs, got:\n{text}"
    );
}

#[test]
fn log_visual_cache_rebuilds_on_width_change() {
    let mut app = make_app();
    app.add_system_message("wrap me ".repeat(30));

    render_log_panel_text(&mut app, 90, 16);
    let wide_cache_len = app.log_scroll.visual_cache.len();
    assert_eq!(app.log_scroll.visual_cache_width, 88); // area.width - 2 borders

    render_log_panel_text(&mut app, 34, 16);
    assert_eq!(app.log_scroll.visual_cache_width, 32);
    assert!(
        app.log_scroll.visual_cache.len() > wide_cache_len,
        "width shrink should rebuild wrap cache with more visual lines"
    );
}

#[test]
fn log_visual_cache_rebuilds_on_theme_change() {
    let mut app = make_app();
    app.add_system_message("theme cache probe".into());
    render_log_panel_text(&mut app, 80, 16);
    let before = app.log_scroll.visual_cache_theme;

    app.toggle_theme();
    render_log_panel_text(&mut app, 80, 16);

    assert_ne!(
        before, app.log_scroll.visual_cache_theme,
        "theme toggle should invalidate visual cache theme tag"
    );
    assert_eq!(app.log_scroll.visual_cache_theme, app.theme.name);
}

#[test]
fn log_tool_card_renders_when_scrolled_into_placeholder_rows() {
    let mut app = make_app();
    seed_tall_bash_tool(&mut app, 25);
    let _ = render_log_panel_text(&mut app, 100, 14);
    let block = app.tools.blocks.last().expect("tool block");
    let summary_logical = app
        .log_scroll
        .phys_to_logical_cache
        .get(block.phys_idx)
        .and_then(|v| *v)
        .expect("summary row should map to logical index");
    let placeholder_phys = block.phys_idx + 1;
    let placeholder_logical = app
        .log_scroll
        .phys_to_logical_cache
        .get(placeholder_phys)
        .and_then(|v| *v)
        .expect("placeholder row should map to logical index");
    assert!(
        placeholder_logical > summary_logical,
        "placeholder row should be below summary row"
    );

    app.log_scroll.offset = placeholder_logical as u16;
    let mid = render_log_panel_text(&mut app, 100, 14);
    assert!(
        mid.contains("bash") && mid.contains("Command output (25 lines)"),
        "starting viewport inside placeholder rows should still render full tool card, got:\n{mid}"
    );

    app.log_scroll.offset = u16::MAX;
    let bottom = render_log_panel_text(&mut app, 100, 14);
    assert!(
        bottom.contains("Command output (25 lines)") && bottom.contains("1/25"),
        "bottom scroll should keep tool card metadata visible, got:\n{bottom}"
    );
}

#[test]
fn log_loading_spinner_shows_braille_and_label() {
    let mut app = make_app();
    app.status = Status::Executing {
        current_step: 0,
        total: 1,
    };
    app.append_blank(RawMessageType::SysTool);
    app.loading_idx = Some(app.messages.len().saturating_sub(1));
    app.spinner_frame = 3;

    let text = render_log_panel_text(&mut app, 80, 16);
    assert!(
        text.contains('⠸') || text.contains('⠋') || text.contains("Thinking"),
        "loading placeholder should render braille spinner or Thinking label, got:\n{text}"
    );
}
