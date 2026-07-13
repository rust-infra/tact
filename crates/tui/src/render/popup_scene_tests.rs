//! Render tests for overlay popups (palette, slash, diff, code, thinking, file picker).

use super::test_harness::{make_app, render_app_text, render_main_area_text};
use crate::widgets::state::{
    App, CodeBlock, CodePopup, DiffPopup, InputMode, RawMessageType, ThinkingBlock, ThinkingPopup,
};
use ratatui::text::Line;
use std::time::Duration;

fn seed_diff_popup(app: &mut App) {
    app.tools.popup = Some(DiffPopup {
        title: "read_file".into(),
        file_path: None,
        git_diff_path: None,
        workspace_dir: None,
        inline_content: Some("fn render_test() {\n    assert!(true);\n}".into()),
        lang: "rust".into(),
        use_diff_gutter: false,
        is_diff: false,
        scroll: 0,
        cached_content: None,
        highlighted_lines: Vec::new(),
    });
}

fn seed_code_popup(app: &mut App) {
    app.code_blocks.push(CodeBlock {
        start_idx: 0,
        end_idx: 3,
        lang: "rust".into(),
        content: "fn main() {}".into(),
        styled: vec![Line::from("fn main() {}")],
    });
    app.code_popup = Some(CodePopup {
        block_idx: 0,
        lang: "rust".into(),
        scroll: 0,
    });
}

fn seed_thinking_popup(app: &mut App) {
    app.raw_messages.push("Thinking title".into());
    app.thinking.blocks.push(ThinkingBlock {
        title_idx: 0,
        end_idx: 1,
        scroll_offset: 0,
        cached_preview: vec!["Deep reasoning line".into()],
        cached_markdown: vec![Line::from("Deep reasoning line")],
        elapsed: Duration::from_millis(10),
    });
    app.thinking.popup = Some(ThinkingPopup {
        block_idx: 0,
        title: "Thinking title".into(),
        scroll: 0,
    });
}

#[test]
fn full_frame_command_palette_filters_commands() {
    let mut app = make_app();
    app.input_mode = InputMode::Palette;
    app.cmd_line = "quit".into();

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("Palette") && text.contains("quit"),
        "palette should show filtered quit command, got:\n{text}"
    );
}

#[test]
fn full_frame_slash_command_popup_lists_help() {
    let mut app = make_app();
    app.input_mode = InputMode::Insert;
    app.input = "/help".into();
    app.input_cursor = app.input.len();
    app.slash_command.active = true;
    app.slash_command.start_pos = 0;

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("help"),
        "slash popup should list help command, got:\n{text}"
    );
}

#[test]
fn full_frame_slash_command_no_match_shows_hint() {
    let mut app = make_app();
    app.input_mode = InputMode::Insert;
    app.input = "/zzzznotfound".into();
    app.input_cursor = app.input.len();
    app.slash_command.active = true;
    app.slash_command.start_pos = 0;

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("No matching command"),
        "unknown slash query should show empty hint, got:\n{text}"
    );
}

#[test]
fn full_frame_file_picker_lists_options() {
    let mut app = make_app();
    app.input_mode = InputMode::FilePicker;
    app.file_picker.options = vec!["src/main.rs".into(), "Cargo.toml".into()];
    app.file_picker.current_dir = app.work_dir.clone();
    app.file_picker.base_dir = app.work_dir.clone();

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("Attach file") || text.contains("main.rs"),
        "file picker should list paths, got:\n{text}"
    );
}

#[test]
fn main_area_diff_popup_renders_inline_content() {
    let mut app = make_app();
    seed_diff_popup(&mut app);

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(
        text.contains("render_test") || text.contains("assert!(true)"),
        "diff popup should show inline tool output, got:\n{text}"
    );
}

#[test]
fn main_area_code_popup_renders_rust_block() {
    let mut app = make_app();
    seed_code_popup(&mut app);

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(
        text.contains("fn main()"),
        "code popup should render block content, got:\n{text}"
    );
}

#[test]
fn main_area_thinking_popup_renders_reasoning() {
    let mut app = make_app();
    seed_thinking_popup(&mut app);

    let text = render_main_area_text(&mut app, 100, 30);

    assert!(
        text.contains("Deep reasoning") || text.contains("Thinking"),
        "thinking popup should show reasoning content, got:\n{text}"
    );
}

#[test]
fn full_frame_done_status_renders_in_status_bar() {
    use tact_protocol::AgentUpdate;

    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::StreamChunk("All done.".into()));
    app.handle_agent_update(AgentUpdate::TaskComplete("All done.".into()));

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("Done") || text.contains("done"),
        "done state should affect status bar, got:\n{text}"
    );
}

#[test]
fn full_frame_select_mode_shows_in_status_bar() {
    let mut app = make_app();
    app.input_mode = InputMode::Select;
    let (tx, _rx) = tokio::sync::oneshot::channel();
    app.select
        .set("Pick one".into(), vec!["A".into(), "B".into()], tx, false);

    let text = render_app_text(&mut app, 100, 24);

    assert!(
        text.contains("SELECT") || text.contains("Pick one"),
        "select mode should appear in status bar or popup, got:\n{text}"
    );
}

#[test]
fn main_area_markdown_stream_renders_in_log() {
    let mut app = make_app();
    app.handle_agent_update(tact_protocol::AgentUpdate::StreamChunk(
        "# Title\n\nBody paragraph.".into(),
    ));

    let text = render_main_area_text(&mut app, 100, 24);

    assert!(
        text.contains("Title") || text.contains("Body"),
        "markdown stream should render in log panel, got:\n{text}"
    );
}

#[test]
fn main_area_system_message_renders_in_log() {
    let mut app = make_app();
    app.add_system_message("System notice for render test".into());

    let text = render_main_area_text(&mut app, 100, 20);

    assert!(
        text.contains("System notice"),
        "system message should appear in log, got:\n{text}"
    );
}

#[test]
fn main_area_loading_spinner_when_executing() {
    use std::collections::HashMap;
    use tact_protocol::{AgentUpdate, PlanStep};

    let mut app = make_app();
    app.plan.visible = true;
    app.status = crate::widgets::state::Status::Executing {
        current_step: 0,
        total: 1,
    };
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "run tool",
        "bash",
        "bash1",
        HashMap::from([("command".to_string(), "sleep 1".to_string())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "bash1".into(),
        tool_name: "bash".into(),
        arg_summary: "sleep 1".into(),
        arg_full: "sleep 1".into(),
    });
    app.append_blank(RawMessageType::SysTool);
    app.loading_idx = Some(app.messages.len().saturating_sub(1));

    let text = render_main_area_text(&mut app, 100, 24);

    assert!(
        !text.trim().is_empty(),
        "executing log with loading placeholder should render, got:\n{text}"
    );
}

#[test]
fn full_frame_file_picker_empty_shows_placeholder() {
    let mut app = make_app();
    app.input_mode = InputMode::FilePicker;

    let text = render_app_text(&mut app, 80, 24);

    assert!(
        text.contains("No options"),
        "empty file picker should render placeholder, got:\n{text}"
    );
}

#[test]
fn diff_popup_renders_unified_diff_markers() {
    let diff_content = "\
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,5 +1,7 @@
 fn existing() {}
-fn removed() {}
+fn added() {}
 fn unchanged() {}
+fn another_new() {}
+fn yet_another() {}
";
    let mut app = make_app();
    app.tools.popup = Some(DiffPopup {
        title: "write_file".into(),
        file_path: None,
        git_diff_path: None,
        workspace_dir: None,
        inline_content: Some(diff_content.into()),
        lang: String::new(),
        use_diff_gutter: false,
        is_diff: true,
        scroll: 0,
        cached_content: None,
        highlighted_lines: Vec::new(),
    });

    let text = render_main_area_text(&mut app, 100, 30);

    // Title indicates diff mode, not a language name
    assert!(
        text.contains("(diff,"),
        "diff popup title should indicate diff mode, got:\n{text}"
    );

    // All unified diff marker lines present
    assert!(text.contains("--- a/src/lib.rs"), "missing --- header");
    assert!(text.contains("+++ b/src/lib.rs"), "missing +++ header");
    assert!(text.contains("@@ -1,5 +1,7 @@"), "missing hunk header @@");

    // Deletion line shown with leading -
    assert!(text.contains("-fn removed()"), "missing deletion line");
    // Addition lines shown with leading +
    assert!(text.contains("+fn added()"), "missing addition line");
    assert!(text.contains("+fn another_new()"), "missing addition line");
    assert!(text.contains("+fn yet_another()"), "missing addition line");
    // Context lines included
    assert!(text.contains("fn existing()"), "missing context line");
    assert!(text.contains("fn unchanged()"), "missing context line");

    // No line numbers in diff mode
    let line_with_num = text
        .lines()
        .any(|l| l.trim_start().starts_with(|c: char| c.is_ascii_digit()));
    assert!(
        !line_with_num,
        "diff mode should not show line numbers, got:\n{text}"
    );
}

#[test]
fn diff_popup_no_diff_mode_shows_line_numbers_and_syntax() {
    let mut app = make_app();
    app.tools.popup = Some(DiffPopup {
        title: "read_file".into(),
        file_path: None,
        git_diff_path: None,
        workspace_dir: None,
        inline_content: Some("fn one() {}\nfn two() {}".into()),
        lang: "rust".into(),
        use_diff_gutter: false,
        is_diff: false,
        scroll: 0,
        cached_content: None,
        highlighted_lines: Vec::new(),
    });

    let text = render_main_area_text(&mut app, 100, 20);

    // Title shows language, not diff
    assert!(
        text.contains("(2 lines, rust"),
        "plain code popup should show lang in title, got:\n{text}"
    );
    assert!(!text.contains("(diff,"), "should not say diff in title");

    // Content rendered
    assert!(text.contains("fn one()"), "missing function one");
    assert!(text.contains("fn two()"), "missing function two");

    // Line numbers present (e.g. "1 fn one()" after border prefix)
    let has_line_num = text.contains("1 fn one()") && text.contains("2 fn two()");
    assert!(
        has_line_num,
        "plain mode should show line numbers, got:\n{text}"
    );
}

#[test]
fn open_diff_popup_after_read_file_step_finish() {
    use std::collections::HashMap;
    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus};

    let mut app = make_app();
    let file = std::env::temp_dir().join(format!("tact-popup-{}.rs", std::process::id()));
    std::fs::write(&file, "fn popup_real_path() {}").expect("write temp file");
    let path = file.to_string_lossy().into_owned();

    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "read",
        "read_file",
        "read_popup",
        HashMap::from([("path".to_string(), path.clone())]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "read_popup".into(),
        tool_name: "read_file".into(),
        arg_summary: path.clone(),
        arg_full: path.clone(),
    });
    app.handle_agent_update(AgentUpdate::StepFinished {
        idx: 0,
        tool_id: "read_popup".into(),
        result: StepResult {
            tool: "read_file".into(),
            arg_summary: path.clone(),
            arg_full: Some(path.clone()),
            status: StepStatus::Success,
            message: "ok".into(),
            detail: Some("fn popup_real_path() {}".into()),
            duration_us: Some(100),
            permission_label: None,
        },
    });

    let phys_idx = app.tools.blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup(phys_idx);

    let text = render_main_area_text(&mut app, 100, 30);
    let _ = std::fs::remove_file(&file);

    assert!(
        text.contains("popup_real_path"),
        "open_diff_popup should render file content from StepFinished tool block, got:\n{text}"
    );
}
