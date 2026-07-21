//! Render tests for overlay popups (palette, slash, diff, code, thinking, file picker).

use std::time::Duration;

use ratatui::{Terminal, backend::TestBackend, style::Modifier, text::Line};

use super::test_harness::{buffer_text, make_app, render_app_text, render_main_area_text};
use crate::widgets::state::{
    App, CodeBlock, CodePopup, DiffPopup, InputMode, PopupTextSelection, RawMessageType,
    ThinkingBlock, ThinkingPopup,
};

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
        selection: None,
        cached_content: None,
        highlighted_lines: Vec::new(),
    });
}

fn render_main_area_terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::render_main_area(frame, frame.area(), app))
        .expect("draw");
    terminal
}

fn render_thinking_popup_text(app: &mut App, width: u16, height: u16) -> String {
    let terminal = render_thinking_popup_terminal(app, width, height);
    buffer_text(terminal.backend().buffer())
}

fn render_thinking_popup_terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            super::popups::thinking_popup::render_thinking_popup(frame, frame.area(), app)
        })
        .expect("draw");
    terminal
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
        phys_idx: 0,
        content: "Deep reasoning line".into(),
        summary: "Deep reasoning line".into(),
        cached_markdown: vec![Line::from("Deep reasoning line")],
        elapsed: Duration::from_millis(10),
    });
    app.thinking.popup = Some(ThinkingPopup {
        phys_idx: 0,
        title: "Thinking title".into(),
        scroll: 0,
        selection: None,
        selection_text: String::new(),
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
fn diff_popup_selection_reverses_source_cells_but_not_number_or_gutter() {
    let mut app = make_app();
    seed_diff_popup(&mut app);
    let popup = app.tools.popup.as_mut().expect("popup");
    popup.inline_content = Some("alpha\nbeta".into());
    popup.lang.clear();
    popup.use_diff_gutter = true;
    popup.selection = Some(PopupTextSelection::new(0, 5));

    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let buffer = terminal.backend().buffer();

    assert!(
        buffer[(row.text_x, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !buffer[(row.text_x - 2, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !buffer[(app.mouse.popup_text_body_area.x, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
fn diff_popup_selection_reverses_wide_scalar_and_maps_both_columns() {
    let mut app = make_app();
    seed_diff_popup(&mut app);
    let popup = app.tools.popup.as_mut().expect("popup");
    popup.inline_content = Some("a界z".into());
    popup.lang.clear();
    popup.selection = Some(PopupTextSelection::new(1, 4));

    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let buffer = terminal.backend().buffer();

    assert!(
        buffer[(row.text_x + 1, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !buffer[(row.text_x + 3, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert_eq!(row.cells[1], row.cells[2]);
    assert_eq!(row.cells[1].start, 1);
    assert_eq!(row.cells[1].end, 4);
}

fn assert_diff_popup_grapheme_selection(
    text: &str,
    grapheme: &str,
    grapheme_end: usize,
    following_end: usize,
) {
    let mut app = make_app();
    seed_diff_popup(&mut app);
    let popup = app.tools.popup.as_mut().expect("popup");
    popup.inline_content = Some(text.into());
    popup.lang.clear();

    let _terminal = render_main_area_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let grapheme_hit = row.hit(row.text_x + 1);
    assert_eq!(
        grapheme_hit,
        crate::widgets::state::PopupTextHit::new(1, grapheme_end)
    );
    assert_eq!(row.hit(row.text_x + 2), grapheme_hit);
    assert_eq!(
        row.hit(row.text_x + 3),
        crate::widgets::state::PopupTextHit::new(grapheme_end, following_end)
    );

    app.tools.popup.as_mut().expect("popup").selection = Some(PopupTextSelection::new(
        grapheme_hit.start,
        grapheme_hit.end,
    ));
    assert_eq!(
        app.tools
            .popup
            .as_ref()
            .expect("popup")
            .copy_content()
            .as_deref(),
        Some(grapheme)
    );

    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let buffer = terminal.backend().buffer();
    assert_eq!(buffer[(row.text_x + 1, row.screen_y)].symbol(), grapheme);
    assert!(
        buffer[(row.text_x + 1, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert_eq!(buffer[(row.text_x + 3, row.screen_y)].symbol(), "b");
}

#[test]
fn diff_popup_selects_and_highlights_complete_emoji_presentation_grapheme() {
    assert_diff_popup_grapheme_selection("a⌨️b", "⌨️", 7, 8);
}

#[test]
fn diff_popup_selects_and_highlights_complete_zwj_emoji_grapheme() {
    assert_diff_popup_grapheme_selection("a👩‍💻b", "👩‍💻", 12, 13);
}

#[test]
fn diff_popup_selection_highlights_visible_scrolled_row() {
    let mut app = make_app();
    seed_diff_popup(&mut app);
    let popup = app.tools.popup.as_mut().expect("popup");
    popup.inline_content = Some("zero\none\ntwo".into());
    popup.lang.clear();
    popup.scroll = 1;
    popup.selection = Some(PopupTextSelection::new(5, 8));

    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let buffer = terminal.backend().buffer();

    assert_eq!(row.line_start, 5);
    assert!(
        buffer[(row.text_x, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
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
fn active_thinking_popup_uses_buffered_content() {
    use tact_protocol::{AgentUpdate, ThinkingChunk};

    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
        "draft reasoning".into(),
    )));
    let phys_idx = app.thinking.active.as_ref().unwrap().phys_idx;
    app.open_thinking_popup(phys_idx);

    assert_eq!(
        app.thinking_popup_content(),
        Some("draft reasoning".to_string())
    );
    let text = render_main_area_text(&mut app, 100, 30);
    assert!(text.contains("draft reasoning"), "{text}");
}

#[test]
fn active_thinking_popup_preserves_blank_lines() {
    use tact_protocol::{AgentUpdate, ThinkingChunk};

    let mut app = make_app();
    app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
        "first line\n\nlast line".into(),
    )));
    let phys_idx = app.thinking.active.as_ref().unwrap().phys_idx;
    app.open_thinking_popup(phys_idx);

    let text = render_thinking_popup_text(&mut app, 100, 30);
    let first = text.lines().position(|line| line.contains("first line"));
    let last = text.lines().position(|line| line.contains("last line"));
    assert!(
        last.zip(first)
            .is_some_and(|(last, first)| last >= first + 2),
        "thinking popup should retain the blank content line, got:\n{text}"
    );
}

#[test]
fn completed_thinking_popup_separates_adjacent_ordered_list_items() {
    let mut app = make_app();
    app.thinking.blocks.push(ThinkingBlock {
        phys_idx: 0,
        content: "1. first item\n2. second item".into(),
        summary: "second item".into(),
        cached_markdown: vec![Line::from("1. first item"), Line::from("2. second item")],
        elapsed: Duration::ZERO,
    });
    app.thinking.popup = Some(ThinkingPopup {
        phys_idx: 0,
        title: "Thinking".into(),
        scroll: 0,
        selection: None,
        selection_text: String::new(),
    });

    let text = render_thinking_popup_text(&mut app, 100, 30);
    let first = text.lines().position(|line| line.contains("1. first item"));
    let second = text
        .lines()
        .position(|line| line.contains("2. second item"));
    assert!(
        second
            .zip(first)
            .is_some_and(|(second, first)| second >= first + 2),
        "ordered thinking items should have a blank row between them, got:\n{text}"
    );
}

#[test]
fn thinking_popup_selection_reverses_selected_body_text_only() {
    let mut app = make_app();
    seed_thinking_popup(&mut app);
    let block = app.thinking.blocks.first_mut().expect("thinking block");
    block.content = "alpha\nbeta".into();
    block.cached_markdown = vec![Line::from("alpha"), Line::from("beta")];
    app.thinking.popup.as_mut().expect("popup").selection = Some(PopupTextSelection::new(0, 5));

    let terminal = render_thinking_popup_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let buffer = terminal.backend().buffer();

    assert!(
        buffer[(row.text_x, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert!(
        !buffer[(app.mouse.thinking_popup_area.x, row.screen_y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
}

#[test]
fn thinking_popup_selection_maps_zwj_emoji_as_one_grapheme() {
    let mut app = make_app();
    seed_thinking_popup(&mut app);
    let block = app.thinking.blocks.first_mut().expect("thinking block");
    block.content = "a👩‍💻b".into();
    block.cached_markdown = vec![Line::from("a👩‍💻b")];

    let _terminal = render_thinking_popup_terminal(&mut app, 100, 30);
    let row = &app.mouse.popup_text_hit_rows[0];
    let hit = row.hit(row.text_x + 1);

    assert_eq!(hit, crate::widgets::state::PopupTextHit::new(1, 12));
    assert_eq!(row.hit(row.text_x + 2), hit);
}

#[test]
fn thinking_popup_selection_text_matches_visible_markdown_text() {
    let mut app = make_app();
    seed_thinking_popup(&mut app);
    let block = app.thinking.blocks.first_mut().expect("thinking block");
    block.content = "**bold reasoning**".into();
    block.cached_markdown = vec![Line::from("bold reasoning")];
    let full_content = block.content.clone();

    let _terminal = render_thinking_popup_terminal(&mut app, 100, 30);
    let popup = app.thinking.popup.as_mut().expect("thinking popup");
    popup.selection = Some(PopupTextSelection::new(0, 4));

    assert_eq!(popup.selection_text, "bold reasoning");
    assert_eq!(popup.copy_content(&full_content), "bold");
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
fn open_diff_popup_after_edit_file_step_uses_git_diff() {
    use std::{collections::HashMap, process::Command};

    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus};

    let tmp = std::env::temp_dir().join(format!("tact-edit-popup-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("lib.rs");
    std::fs::write(&file, "fn old() {}").unwrap();

    let git = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.current_dir(&tmp)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .args(args);
        cmd.output().unwrap()
    };
    git(&["init"]);
    git(&["add", "."]);
    git(&["commit", "-m", "init"]);

    std::fs::write(&file, "fn new() {}").unwrap();

    let mut app = make_app();
    app.work_dir = tmp.clone();

    let path = file.to_string_lossy().into_owned();
    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "edit",
        "edit_file",
        "edit_popup",
        HashMap::from([
            ("path".to_string(), path.clone()),
            ("old_text".to_string(), "fn old() {}".into()),
            ("new_text".to_string(), "fn new() {}".into()),
        ]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "edit_popup".into(),
        tool_name: "edit_file".into(),
        arg_summary: path.clone(),
        arg_full: path.clone(),
    });
    app.handle_agent_update(AgentUpdate::StepFinished {
        idx: 0,
        tool_id: "edit_popup".into(),
        result: StepResult {
            tool: "edit_file".into(),
            arg_summary: path.clone(),
            arg_full: Some(path.clone()),
            status: StepStatus::Success,
            message: "wrote".into(),
            detail: Some("fn new() {}".into()),
            duration_us: Some(100),
            permission_label: None,
        },
    });

    let phys_idx = app.tools.blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup(phys_idx);

    let text = render_main_area_text(&mut app, 100, 30);
    let _ = std::fs::remove_dir_all(&tmp);

    assert!(
        text.contains("fn new()") || text.contains("@@") || text.contains('+'),
        "edit_file popup should render git diff, got:\n{text}"
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
        title: "edit_file".into(),
        file_path: None,
        git_diff_path: None,
        workspace_dir: None,
        inline_content: Some(diff_content.into()),
        lang: String::new(),
        use_diff_gutter: false,
        is_diff: true,
        scroll: 0,
        selection: None,
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
        selection: None,
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
fn open_diff_popup_after_edit_file_step_shows_minus_and_plus() {
    use std::{collections::HashMap, process::Command};

    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus};

    let tmp = std::env::temp_dir().join(format!("tact-edit-popup-mp-{}", std::process::id()));
    std::fs::create_dir_all(&tmp).unwrap();
    let file = tmp.join("calc.rs");
    std::fs::write(&file, "fn add(a: i32, b: i32) -> i32 {\n    a + b\n}").unwrap();

    let git = |args: &[&str]| {
        let mut cmd = Command::new("git");
        cmd.current_dir(&tmp)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .args(args);
        cmd.output().unwrap()
    };
    git(&["init"]);
    git(&["add", "."]);
    git(&["commit", "-m", "init"]);

    // Edit: change `a + b` to `a - b`
    std::fs::write(&file, "fn add(a: i32, b: i32) -> i32 {\n    a - b\n}").unwrap();

    let mut app = make_app();
    app.work_dir = tmp.clone();
    let path = file.to_string_lossy().into_owned();

    app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
        "edit",
        "edit_file",
        "edit_calc",
        HashMap::from([
            ("path".to_string(), path.clone()),
            ("old_text".to_string(), "a + b".into()),
            ("new_text".to_string(), "a - b".into()),
        ]),
    )));
    app.handle_agent_update(AgentUpdate::StepStarted {
        idx: 0,
        tool_id: "edit_calc".into(),
        tool_name: "edit_file".into(),
        arg_summary: path.clone(),
        arg_full: path.clone(),
    });
    app.handle_agent_update(AgentUpdate::StepFinished {
        idx: 0,
        tool_id: "edit_calc".into(),
        result: StepResult {
            tool: "edit_file".into(),
            arg_summary: path.clone(),
            arg_full: Some(path.clone()),
            status: StepStatus::Success,
            message: "wrote".into(),
            detail: Some("fn add(a: i32, b: i32) -> i32 {\n    a - b\n}".into()),
            duration_us: Some(100),
            permission_label: None,
        },
    });

    let phys_idx = app.tools.blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup(phys_idx);

    let text = render_main_area_text(&mut app, 100, 30);
    let _ = std::fs::remove_dir_all(&tmp);

    // Unified diff must show both the removed line (-) and the added line (+)
    assert!(
        text.contains("-    a + b"),
        "git diff should show removed line '-    a + b', got:\n{text}"
    );
    assert!(
        text.contains("+    a - b"),
        "git diff should show added line '+    a - b', got:\n{text}"
    );
    // Context around the change
    assert!(
        text.contains("fn add"),
        "context line around diff should appear, got:\n{text}"
    );
    // Hunk header present
    assert!(
        text.contains("@@"),
        "diff should show @@ hunk header, got:\n{text}"
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
