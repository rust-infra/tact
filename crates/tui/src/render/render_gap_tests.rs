//! Render gap tests: P0/P1 coverage for previously untested paths.

use std::{collections::HashMap, time::Duration};

use tact_protocol::{AccountUpdate, AgentUpdate, PlanStep, ThinkingChunk, ToolPresentationInfo};

use super::test_harness::{
    make_app, render_app_text, render_log_panel_text, render_main_area_text,
};
use crate::test_fixtures::StepCall;
use crate::widgets::state::{App, InputMode, Status};

#[test]
fn full_frame_keeps_bottom_bar_visible_when_terminal_is_short() {
    let mut app = make_app();
    let text = render_app_text(&mut app, 120, 8);

    assert!(
        !text.trim().is_empty(),
        "bottom bar should remain visible: {text}"
    );
}

fn seed_write_file_finished(app: &mut App, path: &str, content: &str) {
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "write file",
        "write_file",
        "wf1",
        HashMap::from([("path".to_string(), path.to_string())]),
    )));
    app.handle_agent_update(StepCall::new(0, "wf1", "write_file", path).started());
    app.handle_agent_update(
        StepCall::new(0, "wf1", "write_file", path)
            .message("written")
            .detail(content)
            .duration_us(50)
            .finished(),
    );
}

fn seed_bash_finished(app: &mut App, command: &str, output: &str) {
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "run shell",
        "bash",
        "bash1",
        HashMap::from([("command".to_string(), command.to_string())]),
    )));
    app.handle_agent_update(StepCall::new(0, "bash1", "bash", command).started());
    app.handle_agent_update(
        StepCall::new(0, "bash1", "bash", command)
            .detail(output)
            .duration_us(100)
            .finished(),
    );
}

fn open_last_tool_popup(app: &mut App) {
    let phys_idx = app.tools_mut().blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup_by_physical_index(phys_idx);
}

// --- P0: diff gutter, bash popup, inline cards ---

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
        app.tools_mut()
            .popup
            .as_ref()
            .is_some_and(|p| p.use_diff_gutter),
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
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
        "Analyzing the problem…".into(),
    )));
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
        " considering options.".into(),
    )));
    app.handle_agent_update(AgentUpdate::StreamChunk("Final answer.".into()));

    let text = render_main_area_text(&mut app, 100, 28);

    assert!(
        !app.thinking_mut().blocks.is_empty(),
        "thinking block should be closed after stream"
    );
    assert!(
        text.contains("Thinking") || text.contains("Analyzing") || text.contains("considering"),
        "collapsed thinking card should render in log, got:\n{text}"
    );
}

#[test]
fn log_markdown_list_then_empty_fence_stays_in_markdown_flow() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "- example:\n  - why not remote compact\n  - why not push current turn first\n```\n - why normalize assistant history\n```".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_log_panel_text(&mut app, 100, 24);

    assert!(
        text.contains("why not remote compact"),
        "first nested list item missing from log render, got:\n{text}"
    );
    assert!(
        text.contains("why not push current turn first"),
        "second nested list item missing from log render, got:\n{text}"
    );
    assert!(
        text.contains("why normalize assistant history"),
        "tail line missing from log render, got:\n{text}"
    );
    assert!(
        !text.contains("Click for full code"),
        "empty fence after markdown list should not be promoted to a code card, got:\n{text}"
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
fn log_renders_streamed_mermaid_without_code_card() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(
        app.code_blocks.is_empty(),
        "valid Mermaid must not become a code card"
    );
    assert_eq!(
        app.mermaid_blocks.len(),
        1,
        "valid Mermaid must retain source metadata"
    );
    assert!(
        app.mermaid_blocks[0].source.contains("Alice->>Bob: Hello"),
        "mermaid source missing: {}",
        app.mermaid_blocks[0].source
    );
    assert!(
        text.contains("Alice") && text.contains("Bob"),
        "diagram missing: {text}"
    );
    assert!(
        !text.contains("sequenceDiagram"),
        "raw Mermaid leaked: {text}"
    );
}

#[test]
fn mermaid_popup_copy_uses_source_not_ascii() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    assert_eq!(app.mermaid_blocks.len(), 1);
    let ascii = app.log.items[app.mermaid_blocks[0].start_idx].raw.clone();
    assert!(
        !ascii.contains("sequenceDiagram"),
        "raw_messages should hold ASCII diagram, got: {ascii}"
    );

    app.open_mermaid_popup_at_physical_index(0);
    let popup = app.mermaid_popup.as_ref().expect("popup open");
    assert_eq!(
        app.mermaid_blocks
            .iter()
            .find(|block| block.block_id == popup.block_id)
            .expect("popup block")
            .source,
        "sequenceDiagram\n  Alice->>Bob: Hello",
        "popup must point at Mermaid source"
    );
    // Exercise the copy path (system clipboard may or may not be available).
    app.copy_mermaid_popup();
}

#[test]
fn log_falls_back_to_code_card_for_invalid_streamed_mermaid() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nnot valid Mermaid\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert_eq!(
        app.code_blocks.len(),
        1,
        "invalid Mermaid should use code fallback"
    );
    assert!(
        app.mermaid_blocks.is_empty(),
        "invalid Mermaid must not register a MermaidBlock"
    );
    assert!(
        text.contains("not valid Mermaid"),
        "fallback lost source: {text}"
    );
}

#[test]
fn flush_consumes_closing_fence_without_trailing_newline() {
    // Stream ends with ``` and no final \n — the close fence stays in stream.buffer.
    // Flush must treat it as a close, not re-render it as leaked ``` lines.
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "已提交。\n\n```text\nCommit: abc\n```\n\n当前状态：\n\n```text\na\nb\nc\n```".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    assert_eq!(
        app.code_blocks.len(),
        2,
        "both fenced blocks should become cards"
    );
    let leaked: Vec<_> = app
        .log
        .items
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            let t = item.raw.trim();
            t == "```" || t.starts_with("```")
        })
        .map(|(i, item)| (i, item.raw.as_str()))
        .collect();
    assert!(
        leaked.is_empty(),
        "closing fence without trailing newline must not leak into raw_messages: {leaked:?}"
    );

    let text = render_main_area_text(&mut app, 100, 32);
    assert!(
        text.contains("Commit: abc") && (text.contains("当前状态") || text.contains("a")),
        "cards/content should still render, got:\n{text}"
    );
}

#[test]
fn flush_renders_streamed_mermaid_without_trailing_newline() {
    // Stream ends with ``` and no final \n — the close fence stays in
    // stream.buffer and flush must finalize a valid diagram, never a code card.
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(
        app.code_blocks.is_empty(),
        "valid Mermaid must not become a code card"
    );
    assert!(
        text.contains("Alice") && text.contains("Bob"),
        "diagram missing: {text}"
    );
    assert!(
        !text.contains("sequenceDiagram"),
        "raw Mermaid leaked: {text}"
    );
}

#[test]
fn flush_falls_back_to_code_card_for_unclosed_streamed_mermaid() {
    // The Mermaid body is fully parseable, but the closing fence never
    // arrives: the stream was interrupted, so the buffered block must take
    // the code-card fallback and retain its source — never splice a diagram.
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert_eq!(
        app.code_blocks.len(),
        1,
        "unclosed Mermaid must use code fallback"
    );
    let block = &app.code_blocks[0];
    assert!(
        block.content.contains("sequenceDiagram") && block.content.contains("Alice->>Bob: Hello"),
        "fallback lost source: {:?}",
        block.content
    );
    // The card preview must show the readable raw source, not a diagram drawn
    // from a fence that never actually closed.
    let preview = block
        .styled
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        preview.contains("sequenceDiagram") && preview.contains("Alice"),
        "styled preview must contain the raw source, got:\n{preview}"
    );
    assert!(
        !preview.contains('─') && !preview.contains('│'),
        "styled preview must not contain diagram box-art, got:\n{preview}"
    );
    assert!(
        text.contains("mermaid"),
        "code card should still render in the log, got:\n{text}"
    );
}

#[test]
fn flush_falls_back_to_code_card_for_unclosed_streamed_mermaid_with_nested_fence() {
    // The buffered Mermaid source itself contains a literal ```mermaid fence
    // line. The fallback preview must be rendered through the plain code path
    // exactly once: re-routing the reconstructed preview fence through the
    // Mermaid router would parse the nested fence as a real diagram and draw
    // box-art inside the code card.
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```mermaid\nflowchart TD\n  A --> B\n"
            .into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));

    let text = render_main_area_text(&mut app, 100, 30);

    assert_eq!(
        app.code_blocks.len(),
        1,
        "unclosed Mermaid must use code fallback"
    );
    let block = &app.code_blocks[0];
    assert_eq!(
        block.lang, "mermaid",
        "card must keep the original language metadata"
    );
    assert!(
        block.content.contains("sequenceDiagram") && block.content.contains("A --> B"),
        "fallback lost source: {:?}",
        block.content
    );
    let preview = block
        .styled
        .iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        preview.contains("sequenceDiagram")
            && preview.contains("Alice")
            && preview.contains("A --> B"),
        "styled preview must contain the raw source, got:\n{preview}"
    );
    assert!(
        !preview.contains('─') && !preview.contains('│'),
        "styled preview must not contain diagram box-art, got:\n{preview}"
    );
    assert!(
        text.contains("mermaid"),
        "code card should still render in the log, got:\n{text}"
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
fn plan_steps_track_multiple_steps_with_one_running() {
    let mut app = make_app();
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
    app.handle_agent_update(StepCall::new(0, "r1", "read_file", "a.txt").started());
    app.handle_agent_update(
        StepCall::new(0, "r1", "read_file", "a.txt")
            .no_arg_full()
            .finished(),
    );
    app.handle_agent_update(StepCall::new(1, "r2", "read_file", "b.txt").started());

    assert_eq!(
        app.plan_mut().steps.len(),
        2,
        "both steps should be tracked"
    );
    assert_eq!(app.plan_mut().steps[0].description, "read first");
    assert_eq!(app.plan_mut().steps[1].description, "read second");

    let text = render_app_text(&mut app, 100, 24);
    assert!(
        text.contains("read_file") || text.contains("a.txt") || text.contains("b.txt"),
        "running/finished steps should still render as log tool cards, got:\n{text}"
    );
}

#[test]
fn plan_panel_lists_failed_step_description() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "failing read",
        "read_file",
        "fail1",
        HashMap::from([("path".to_string(), "nope.txt".to_string())]),
    )));
    app.handle_agent_update(StepCall::new(0, "fail1", "read_file", "nope.txt").started());
    app.handle_agent_update(AgentUpdate::StepFailed {
        idx: 0,
        tool_id: "fail1".into(),
        arg_summary: String::new(),
        error: "file not found".into(),
    });

    let text = render_app_text(&mut app, 120, 30);
    assert!(
        text.contains("failing read") || text.contains("nope.txt"),
        "failed step should remain visible in plan/log, got:\n{text}"
    );
}

#[test]
fn diff_popup_scroll_skips_leading_lines() {
    let mut app = make_app();
    let lines: String = (1..=20)
        .map(|n| format!("line-{n}"))
        .collect::<Vec<_>>()
        .join("\n");
    seed_write_file_finished(&mut app, "scroll.rs", &lines);
    open_last_tool_popup(&mut app);
    if let Some(popup) = app.tools_mut().popup.as_mut() {
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
    use ratatui::text::Line;

    use crate::widgets::state::{CodeBlock, CodePopup};

    let mut app = make_app();
    let content: String = (1..=15)
        .map(|n| format!("row {n}"))
        .collect::<Vec<_>>()
        .join("\n");
    let styled: Vec<Line<'static>> = content.lines().map(|l| Line::from(l.to_string())).collect();
    app.code_blocks.push(CodeBlock {
        block_id: "test-code".into(),
        start_idx: 0,
        end_idx: styled.len(),
        lang: "rust".into(),
        content: content.clone(),
        styled,
    });
    app.code_popup = Some(CodePopup {
        block_id: "test-code".into(),
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
    app.file_picker.options = vec!["src/a.rs".into(), "src/b.rs".into()];
    app.file_picker.current_dir = app.work_dir.clone();
    app.file_picker.base_dir = app.work_dir.clone();
    app.file_picker.selected = 1;

    let text = render_app_text(&mut app, 80, 24);

    assert!(
        text.contains("▶") && text.contains("b.rs"),
        "selected file picker row should show arrow marker, got:\n{text}"
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
    use ratatui::text::Line;

    use crate::widgets::state::{ThinkingBlock, ThinkingPopup};

    let mut app = make_app();
    let markdown: Vec<Line> = (1..=12)
        .map(|n| Line::from(format!("reason-{n}")))
        .collect();
    app.append_msg(
        Line::from("Thinking"),
        "Thinking".into(),
        crate::widgets::state::LogItemKind::Thinking,
    );
    app.thinking_mut().blocks.push(ThinkingBlock {
        block_id: "test-thinking".into(),
        phys_idx: 0,
        content: (1..=12)
            .map(|n| format!("reason-{n}"))
            .collect::<Vec<_>>()
            .join("\n"),
        summary: "reason-12".into(),
        cached_markdown: markdown,
        elapsed: Duration::from_millis(5),
    });
    app.thinking_mut().popup = Some(ThinkingPopup {
        block_id: "test-thinking".into(),
        phys_idx: 0,
        title: "Thinking".into(),
        scroll: 6,
        selection: None,
        selection_text: String::new(),
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

// --- Handler-adjacent render: Planning, RequestSelect, edit_file ---

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
fn request_select_update_renders_select_popup() {
    let mut app = make_app();

    app.handle_agent_update(AgentUpdate::RequestSelect {
        request_id: 0,
        prompt: "Allow edit_file on lib.rs?".into(),
        options: vec!["Allow once".into(), "Deny".into()],
        log_confirm: false,
    });

    assert!(matches!(app.input_mode, InputMode::Select));

    let text = render_app_text(&mut app, 100, 28);
    assert!(
        text.contains("Allow edit_file") || text.contains("lib.rs"),
        "RequestSelect should render permission prompt in full frame, got:\n{text}"
    );
}

#[test]
fn full_frame_edit_file_tool_shows_in_log() {
    let mut app = make_app();
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
    app.handle_agent_update(StepCall::new(0, "edit1", "edit_file", "lib.rs").started());
    app.handle_agent_update(
        StepCall::new(0, "edit1", "edit_file", "lib.rs")
            .message("patched")
            .detail("- fn old()\n+ fn new()")
            .duration_us(200)
            .finished(),
    );

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("lib.rs"),
        "the edit's path must stay visible in the title, got:\n{text}"
    );
    assert!(
        !text.contains("fn old()") && !text.contains("fn new()"),
        "a finished edit hides its diff behind the popup, got:\n{text}"
    );
    assert!(
        text.contains("2 lines · [󰜼 Open]"),
        "the meta row must report the hidden diff, got:\n{text}"
    );
}

/// A finished read collapses like a command: the body is out of the log, the
/// two header rows stay, and the meta row says how much it hid.
#[test]
fn full_frame_read_file_tool_shows_in_log() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "read lib",
        "read_file",
        "read1",
        HashMap::from([("path".to_string(), "lib.rs".to_string())]),
    )));
    app.handle_agent_update(StepCall::new(0, "read1", "read_file", "lib.rs").started());
    app.handle_agent_update(
        StepCall::new(0, "read1", "read_file", "lib.rs")
            .detail("body-one\nbody-two\nbody-three")
            .duration_us(200)
            .finished(),
    );

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("lib.rs"),
        "the read's path must stay visible in the title, got:\n{text}"
    );
    assert!(
        !text.contains("body-one"),
        "a finished read hides its body behind the popup, got:\n{text}"
    );
    assert!(
        text.contains("3 lines · [󰜼 Open]"),
        "the meta row must report the hidden body, got:\n{text}"
    );
}

/// A finished write collapses like the rest: the content is out of the log, the
/// two header rows stay, and the meta row says how much it hid.
#[test]
fn full_frame_write_file_tool_shows_in_log() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "write lib",
        "write_file",
        "w1",
        HashMap::from([("path".to_string(), "lib.rs".to_string())]),
    )));
    app.handle_agent_update(StepCall::new(0, "w1", "write_file", "lib.rs").started());
    app.handle_agent_update(
        StepCall::new(0, "w1", "write_file", "lib.rs")
            .message("wrote")
            .detail("wrote-one\nwrote-two\nwrote-three")
            .duration_us(200)
            .finished(),
    );

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        text.contains("lib.rs"),
        "the write's path must stay visible in the title, got:\n{text}"
    );
    assert!(
        !text.contains("wrote-one"),
        "a finished write hides its content behind the popup, got:\n{text}"
    );
    assert!(
        text.contains("3 lines · [󰜼 Open]"),
        "the meta row must report the hidden content, got:\n{text}"
    );
}

/// A kind that never drew a card used to lose its result outright. A multi-line
/// readout (`task_list`) now costs the same two rows and says it can be opened.
#[test]
fn full_frame_cardless_tool_result_is_openable() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "list tasks",
        "task_list",
        "t1",
        HashMap::<String, String>::new(),
    )));
    app.handle_agent_update(
        StepCall::new(0, "t1", "task_list", String::new())
            .presentation(ToolPresentationInfo {
                visual_kind: tact_protocol::ToolVisualKind::Task,
                display_name: "📋 Task".into(),
                ..ToolPresentationInfo::generic("task_list")
            })
            .started(),
    );
    app.handle_agent_update(
        StepCall::new(0, "t1", "task_list", String::new())
            .no_arg_full()
            .message("2 tasks")
            .detail("[1] pending  wire the parser\n[2] in_progress  run the suite")
            .duration_us(200)
            .presentation(ToolPresentationInfo {
                visual_kind: tact_protocol::ToolVisualKind::Task,
                display_name: "📋 Task".into(),
                ..ToolPresentationInfo::generic("task_list")
            })
            .finished(),
    );

    let text = render_app_text(&mut app, 120, 30);

    assert!(
        !text.contains("wire the parser"),
        "the task list stays behind the popup, got:\n{text}"
    );
    assert!(
        text.contains("2 lines · [󰜼 Open]"),
        "a multi-line result must advertise that it can be opened, got:\n{text}"
    );
}

#[test]
fn balance_update_renders_in_bottom_bar() {
    use tact_protocol::{BalanceEntry, BalanceInfo};

    let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = make_app();
    app.account_rx = Some(account_rx);
    app.handle_account_update(AccountUpdate::Balance(BalanceInfo {
        is_available: true,
        balance_infos: vec![BalanceEntry {
            currency: "USD".into(),
            total_balance: 42.00,
            granted_balance: 40.00,
            topped_up_balance: 2.00,
        }],
    }));

    let text = render_app_text(&mut app, 120, 12);
    assert!(
        text.contains("42.00") || text.contains("USD"),
        "Balance amount should append on bottom bar row 1, got:\n{text}"
    );
}

#[test]
fn usage_quota_update_renders_in_bottom_bar() {
    use tact_protocol::{UsageQuotaInfo, UsageQuotaWindow};

    let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut app = make_app();
    app.account_rx = Some(account_rx);
    app.handle_account_update(AccountUpdate::UsageQuota(UsageQuotaInfo {
        is_available: true,
        windows: vec![UsageQuotaWindow {
            label: "week".into(),
            limit: Some(100.0),
            remaining: Some(42.0),
            reset_time: None,
        }],
        membership_level: None,
    }));

    let text = render_app_text(&mut app, 120, 12);
    assert!(
        text.contains("week") && text.contains("58%"),
        "usage quota should render label and percentage on bottom bar row 1, got:\n{text}"
    );
    assert!(
        text.contains('¤'),
        "usage quota should use balance/quota icon, got:\n{text}"
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

#[test]
fn copy_flash_expires_after_its_window() {
    let mut app = make_app();
    app.copy_flash_at = Some(std::time::Instant::now() - std::time::Duration::from_millis(2_000));

    app.maybe_clear_copy_flash();

    assert!(
        app.copy_flash_at.is_none(),
        "a stale confirmation must clear"
    );
}

#[test]
fn copy_flash_persists_within_its_window() {
    let mut app = make_app();
    app.copy_flash_at = Some(std::time::Instant::now());

    app.maybe_clear_copy_flash();

    assert!(
        app.copy_flash_at.is_some(),
        "a fresh confirmation must stay on screen"
    );
}

/// The startup output is three blocks — banner, welcome/mode hints, then
/// everything else — and the log is a single scrolling column, so the
/// separation has to be written as rows.
///
/// Regression: with one trailing row the first system line to land after
/// startup (a `/theme` or `/model` write, a plugin briefing, the restored
/// session) sat flush against "Current mode: …" and read as part of the banner.
#[test]
fn startup_banner_separates_itself_from_what_follows() {
    let mut app = make_app();
    app.add_startup_banner();
    app.add_system_message("✓ Saved theme = \"gruvbox-dark\" to config".into());

    let rows: Vec<String> = app.log.items.iter().map(|item| item.raw.clone()).collect();
    let hint = app.msgs().startup_mode_hint;
    let hint_at = rows
        .iter()
        .position(|row| row == hint)
        .expect("the mode hint is in the log");

    for gap in 1..=2 {
        assert!(
            rows[hint_at + gap].is_empty(),
            "row {gap} after the mode hint must be blank, got {:?}",
            rows[hint_at + gap]
        );
    }
    assert!(
        rows[hint_at + 3].contains("Saved theme"),
        "the next message starts after the gap, got {:?}",
        rows[hint_at + 3]
    );

    // And the banner is its own block too: two rows under the tagline, then the
    // welcome.
    let welcome = app.msgs().startup_welcome;
    let welcome_at = rows
        .iter()
        .position(|row| row == welcome)
        .expect("the welcome is in the log");
    assert!(
        rows[welcome_at - 1].is_empty() && rows[welcome_at - 2].is_empty(),
        "the banner must be followed by two blank rows, got {:?} / {:?}",
        rows[welcome_at - 2],
        rows[welcome_at - 1]
    );
}

#[test]
fn startup_logo_renders_in_full_frame() {
    let mut app = make_app();
    app.add_startup_logo();

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains('█') || text.contains('T') || text.contains("tact"),
        "startup logo should render ASCII art in log, got:\n{text}"
    );
}

#[test]
fn toggle_theme_renders_changed_message_in_log() {
    let mut app = make_app();
    let before = app.theme.name;
    app.toggle_theme();

    let text = render_app_text(&mut app, 100, 24);
    assert_ne!(app.theme.name, before);
    assert!(
        !text.trim().is_empty(),
        "theme toggle should produce visible log update, got:\n{text}"
    );
}

#[test]
fn mermaid_popup_opens_on_rendered_diagram_not_source() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));
    assert_eq!(app.mermaid_blocks.len(), 1);

    app.open_mermaid_popup_at_physical_index(0);
    let popup = app.mermaid_popup.as_ref().expect("popup open");
    assert_eq!(
        popup.view,
        agent_tui_kit::state::MermaidPopupView::Diagram,
        "popup must open on the rendered diagram"
    );

    let text = render_main_area_text(&mut app, 100, 30);
    assert!(
        text.contains("Alice") && text.contains("Bob"),
        "diagram art missing from popup: {text}"
    );
    assert!(
        !text.contains("sequenceDiagram"),
        "raw Mermaid source leaked while in diagram view: {text}"
    );
}

#[test]
fn mermaid_popup_tab_switches_to_source_and_back() {
    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk(
        "```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n".into(),
    ));
    app.handle_agent_update(AgentUpdate::TaskComplete("done".into()));
    app.open_mermaid_popup_at_physical_index(0);

    app.toggle_mermaid_popup_view();
    let source_text = render_main_area_text(&mut app, 100, 30);
    assert!(
        source_text.contains("sequenceDiagram"),
        "source view must show the fence body: {source_text}"
    );
    assert_eq!(
        app.mermaid_popup.as_ref().unwrap().view,
        agent_tui_kit::state::MermaidPopupView::Source
    );

    app.toggle_mermaid_popup_view();
    let diagram_text = render_main_area_text(&mut app, 100, 30);
    assert_eq!(
        app.mermaid_popup.as_ref().unwrap().view,
        agent_tui_kit::state::MermaidPopupView::Diagram
    );
    assert!(
        !diagram_text.contains("sequenceDiagram"),
        "toggling back must restore the diagram: {diagram_text}"
    );
}

#[test]
fn mermaid_popup_falls_back_to_source_and_labels_unsupported_syntax() {
    // `style` statements are not supported by the upstream renderer. The log
    // already falls back to a code card, so register the block the way a
    // rendered diagram would be and drive the popup directly.
    let mut app = make_app();
    app.mermaid_blocks
        .push(crate::widgets::state::MermaidBlock {
            block_id: "test-mermaid-0".into(),
            start_idx: 0,
            end_idx: 1,
            source: "flowchart TD\n    A[Start] --> B[Done]\n    style B fill:#ddffdd".into(),
        });
    app.open_mermaid_popup_at_physical_index(0);

    assert_eq!(
        app.mermaid_popup.as_ref().unwrap().view,
        agent_tui_kit::state::MermaidPopupView::Diagram,
        "popup still opens in diagram mode; the renderer downgrades it"
    );

    let text = render_main_area_text(&mut app, 100, 30);
    assert!(
        text.contains("flowchart TD") && text.contains("style B"),
        "unrenderable diagram must show its source instead of blank art: {text}"
    );
    assert!(
        text.contains("does not render"),
        "fallback must explain why no diagram is shown: {text}"
    );
}

#[test]
fn mermaid_popup_renders_diagram_at_wider_width_than_log() {
    // The popup's value is width: it re-lays out the diagram against ~80% of
    // the frame instead of the narrower log panel.
    let source = "flowchart TD\n    A[Context too large] --> B[collect user messages]\n    B --> C[rebuild history]";
    let mut app = make_app();
    app.mermaid_blocks
        .push(crate::widgets::state::MermaidBlock {
            block_id: "test-mermaid-1".into(),
            start_idx: 0,
            end_idx: 1,
            source: source.into(),
        });
    app.open_mermaid_popup_at_physical_index(0);

    let text = render_main_area_text(&mut app, 120, 30);
    assert!(
        text.contains("Context") && text.contains("collect user messages"),
        "diagram node labels missing from popup: {text}"
    );
    assert!(
        !text.contains("flowchart TD"),
        "diagram view must not leak the fence header: {text}"
    );
}

#[test]
fn mermaid_popup_paints_theme_bg_across_its_area() {
    // AGENTS.md: a render unit must paint its own bg over its full area, or
    // ratatui's cell diffing leaves stale styled cells behind.
    let mut app = make_app();
    app.mermaid_blocks
        .push(crate::widgets::state::MermaidBlock {
            block_id: "test-mermaid-2".into(),
            start_idx: 0,
            end_idx: 1,
            source: "sequenceDiagram\n  Alice->>Bob: Hello".into(),
        });
    app.open_mermaid_popup_at_physical_index(0);

    let theme_bg = app.theme.bg;
    let terminal = crate::render::test_harness::render_main_area_terminal(&mut app, 100, 30);
    let buffer = terminal.backend().buffer();
    let expected = theme_bg;

    // The popup is 80% of the frame, centered.
    let popup_x = (100u16 * 20) / 100;
    let popup_y = (30u16 * 20) / 100;
    let mut offenders = 0;
    for y in popup_y..(30 - popup_y) {
        for x in popup_x..(100 - popup_x) {
            let cell = &buffer[(x, y)];
            if cell.bg != expected {
                offenders += 1;
                if offenders <= 3 {
                    eprintln!(
                        "non-bg cell at ({x},{y}): {:?} {:?}",
                        cell.symbol(),
                        cell.bg
                    );
                }
            }
        }
    }
    assert_eq!(
        offenders, 0,
        "popup area must be fully painted with theme bg"
    );
}
