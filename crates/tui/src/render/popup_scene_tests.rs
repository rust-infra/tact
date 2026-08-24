//! Render tests for overlay popups (palette, slash, diff, code, thinking, file picker).

use std::time::Duration;

use ratatui::{Terminal, backend::TestBackend, style::Modifier, text::Line};

use super::test_harness::{
    buffer_contains, buffer_text, make_app, render_app_text, render_main_area_text,
};
use crate::widgets::state::{
    App, CodeBlock, CodePopup, DiffPopup, InputMode, LogItemKind, PopupTextSelection,
    SubagentPopup, ThinkingBlock, ThinkingPopup,
};

fn seed_diff_popup(app: &mut App) {
    app.tools_mut().popup = Some(DiffPopup {
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
    app.append_msg(
        Line::from("Thinking title"),
        "Thinking title".into(),
        LogItemKind::Thinking,
    );
    app.thinking_mut().blocks.push(ThinkingBlock {
        phys_idx: 0,
        content: "Deep reasoning line".into(),
        summary: "Deep reasoning line".into(),
        cached_markdown: vec![Line::from("Deep reasoning line")],
        elapsed: Duration::from_millis(10),
    });
    app.thinking_mut().popup = Some(ThinkingPopup {
        phys_idx: 0,
        title: "Thinking title".into(),
        scroll: 0,
        selection: None,
        selection_text: String::new(),
    });
}

fn render_subagent_popup_terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            super::popups::subagent_popup::render_subagent_popup(frame, frame.area(), app)
        })
        .expect("draw");
    terminal
}

fn render_subagent_popup_text(app: &mut App, width: u16, height: u16) -> String {
    let terminal = render_subagent_popup_terminal(app, width, height);
    buffer_text(terminal.backend().buffer())
}

fn seed_subagent_popup(app: &mut App, content: &str) {
    seed_subagent_popup_impl(app, Some(content.into()), None, "subagent");
}

/// Seed a completed subagent block carrying structured transcript sections.
/// `prompt` becomes the card's `arg_full` and is rendered inside the popup's
/// Context section.
fn seed_subagent_popup_sections(
    app: &mut App,
    sections: &[agent_tui_kit::protocol::SubagentSectionBlock],
    prompt: &str,
) {
    seed_subagent_popup_impl(app, None, Some(sections.to_vec()), prompt);
}

fn seed_subagent_popup_impl(
    app: &mut App,
    detail_full: Option<String>,
    detail_sections: Option<Vec<agent_tui_kit::protocol::SubagentSectionBlock>>,
    arg_full: &str,
) {
    use agent_tui_kit::state::ToolBlock;
    use agent_tui_kit::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput};
    use tact_protocol::ToolVisualKind;
    app.tools_mut().blocks.push(ToolBlock {
        phys_idx: 0,
        tool_id: "sub-1".into(),
        output: ToolRenderOutput {
            title_line: Line::from("🤖 Subagent"),
            title_raw: "subagent".into(),
            phase: ToolPhase::Success,
            permission_label: None,
            error_message: None,
            duration_us: Some(1),
            size_bytes: None,
            tool_name: "spawn_subagent".into(),
            use_diff_gutter: false,
            arg_summary: arg_full.into(),
            arg_full: arg_full.into(),
            layout: ToolLayout {
                visual_rows: 0,
                preview_lines: 0,
                has_detail_card: false,
            },
            detail_title: None,
            detail_preview: Vec::new(),
            detail_total_lines: 0,
            detail_full,
            detail_sections,
            card_bottom: String::new(),
            subagent_model: None,
            subagent_tokens: None,
            visual_kind: ToolVisualKind::Subagent,
        },
    });
    app.subagent_popup = Some(SubagentPopup {
        title: "subagent".into(),
        scroll: 0,
        tool_id: "sub-1".into(),
        cached_markdown: None,
        selection: None,
        layout_cache: None,
    });
}

/// Seed an active (live) subagent block with the given tagged chunks and a
/// prompt, then open its popup.
fn seed_active_subagent_popup(
    app: &mut App,
    chunks: &[tact_protocol::ToolOutputChunk],
    prompt: &str,
) {
    use agent_tui_kit::state::ActiveToolBlock;
    use agent_tui_kit::widgets::tool_widget::{ToolLayout, ToolPhase, ToolRenderOutput};
    use tact_protocol::{ToolOutputBuffer, ToolVisualKind};
    let mut live_output = ToolOutputBuffer::new_full(50_000);
    live_output.push_chunks(chunks);
    app.tools_mut().active.push(ActiveToolBlock {
        phys_idx: 0,
        tool_id: "sub-1".into(),
        output: ToolRenderOutput {
            title_line: Line::from("🤖 Subagent"),
            title_raw: "subagent".into(),
            phase: ToolPhase::Running,
            permission_label: None,
            error_message: None,
            duration_us: Some(1),
            size_bytes: None,
            tool_name: "spawn_subagent".into(),
            use_diff_gutter: false,
            arg_summary: prompt.into(),
            arg_full: prompt.into(),
            layout: ToolLayout {
                visual_rows: 0,
                preview_lines: 0,
                has_detail_card: false,
            },
            detail_title: None,
            detail_preview: Vec::new(),
            detail_total_lines: 0,
            detail_full: None,
            detail_sections: None,
            card_bottom: String::new(),
            subagent_model: None,
            subagent_tokens: None,
            visual_kind: ToolVisualKind::Subagent,
        },
        live_output,
        started_at: std::time::Instant::now(),
    });
    app.subagent_popup = Some(SubagentPopup {
        title: "subagent".into(),
        scroll: 0,
        tool_id: "sub-1".into(),
        cached_markdown: None,
        selection: None,
        layout_cache: None,
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
fn full_frame_palette_popup_stays_inside_main_area() {
    // Regression: the palette popup was centered on the full frame and its
    // height cap was `frame.height - 4`, so with the full command list on a
    // short terminal it overlapped the command-line input box and the bottom
    // bar — palette rows interleaved with the input border glyphs and read
    // as a shadow/mess. The popup must stay within the main area (below the
    // status bar, above the input box).
    let mut app = make_app();
    app.input_mode = InputMode::Palette; // unfiltered: full command list

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();

    // Input box top border (row 25 on this layout) must be intact and the
    // rows below it must carry no palette list rows.
    let input_row: String = (0..buf.area.width)
        .map(|x| buf[(x, 25)].symbol().to_string())
        .collect();
    assert!(
        input_row.starts_with("┌ ⌘ Command"),
        "input box top border must be visible, got: {input_row}"
    );
    for y in 24..30 {
        let row: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        assert!(
            !row.contains("background") && !row.contains("Tools") && !row.contains("theme"),
            "palette rows leaked onto y={y}: {row}"
        );
    }
    // The palette itself is still rendered (bottom border visible above the
    // log panel bottom border at y=22).
    let popup_bottom: String = (0..buf.area.width)
        .map(|x| buf[(x, 22)].symbol().to_string())
        .collect();
    assert!(
        popup_bottom.contains('└'),
        "palette popup bottom border missing at y=22: {popup_bottom}"
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
    assert!(
        text.contains("[Esc]") || text.contains("Close") || text.contains("关闭"),
        "slash popup title should hint Esc closes, got:\n{text}"
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
    let popup = app.tools_mut().popup.as_mut().expect("popup");
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
    let popup = app.tools_mut().popup.as_mut().expect("popup");
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
    let popup = app.tools_mut().popup.as_mut().expect("popup");
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

    app.tools_mut().popup.as_mut().expect("popup").selection = Some(PopupTextSelection::new(
        grapheme_hit.start,
        grapheme_hit.end,
    ));
    assert_eq!(
        app.tools_mut()
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
    let popup = app.tools_mut().popup.as_mut().expect("popup");
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
    let phys_idx = app.thinking_mut().active.as_ref().unwrap().phys_idx;
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
    let phys_idx = app.thinking_mut().active.as_ref().unwrap().phys_idx;
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
    app.thinking_mut().blocks.push(ThinkingBlock {
        phys_idx: 0,
        content: "1. first item\n2. second item".into(),
        summary: "second item".into(),
        cached_markdown: vec![Line::from("1. first item"), Line::from("2. second item")],
        elapsed: Duration::ZERO,
    });
    app.thinking_mut().popup = Some(ThinkingPopup {
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
    let block = app
        .thinking_mut()
        .blocks
        .first_mut()
        .expect("thinking block");
    block.content = "alpha\nbeta".into();
    block.cached_markdown = vec![Line::from("alpha"), Line::from("beta")];
    app.thinking_mut().popup.as_mut().expect("popup").selection =
        Some(PopupTextSelection::new(0, 5));

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
    let block = app
        .thinking_mut()
        .blocks
        .first_mut()
        .expect("thinking block");
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
    let block = app
        .thinking_mut()
        .blocks
        .first_mut()
        .expect("thinking block");
    block.content = "**bold reasoning**".into();
    block.cached_markdown = vec![Line::from("bold reasoning")];
    let full_content = block.content.clone();

    let _terminal = render_thinking_popup_terminal(&mut app, 100, 30);
    let popup = app.thinking_mut().popup.as_mut().expect("thinking popup");
    popup.selection = Some(PopupTextSelection::new(0, 4));

    assert_eq!(popup.selection_text, "bold reasoning");
    assert_eq!(popup.copy_content(&full_content), "bold");
}

#[test]
fn thinking_popup_marks_headings_with_hash_prefix() {
    let mut app = make_app();
    seed_thinking_popup(&mut app);
    let block = app
        .thinking_mut()
        .blocks
        .first_mut()
        .expect("thinking block");
    block.content = "# Title\n\n## Sub\n\nplain body".into();
    block.cached_markdown = Vec::new();

    let text = render_thinking_popup_text(&mut app, 100, 30);

    assert!(text.contains("# Title"), "H1 needs a # prefix:\n{text}");
    assert!(text.contains("## Sub"), "H2 needs a ## prefix:\n{text}");
    assert!(text.contains("plain body"), "{text}");
}

#[test]
fn thinking_popup_code_rows_fill_the_tail_with_code_background() {
    let mut app = make_app();
    seed_thinking_popup(&mut app);
    let block = app
        .thinking_mut()
        .blocks
        .first_mut()
        .expect("thinking block");
    block.content = "```rust\nfn main() {}\n```".into();
    block.cached_markdown = Vec::new();

    let terminal = render_thinking_popup_terminal(&mut app, 100, 30);
    let buf = terminal.backend().buffer();
    let code_bg = app.theme.code_block_bg();

    // Find the row containing the code text and assert the tail cells carry
    // the code background all the way to the popup body edge.
    let body = app.mouse.popup_text_body_area;
    let body_right = body.x + body.width - 1;
    let code_row = (0..buf.area.height)
        .find(|&y| {
            (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "m" && buf[(x, y)].bg == code_bg)
        })
        .expect("code row");

    for x in body.x..body_right {
        assert_eq!(
            buf[(x, code_row)].bg,
            code_bg,
            "code row tail must carry the code background at x={x}"
        );
    }
}

#[test]
fn completed_subagent_popup_marks_headings_with_hash_prefix() {
    let mut app = make_app();
    seed_subagent_popup(&mut app, "# Title\n\n## Sub\n\nplain body");

    let text = render_subagent_popup_text(&mut app, 100, 30);

    assert!(text.contains("# Title"), "H1 needs a # prefix:\n{text}");
    assert!(text.contains("## Sub"), "H2 needs a ## prefix:\n{text}");
    assert!(text.contains("plain body"), "{text}");
}

#[test]
fn completed_subagent_popup_code_rows_fill_the_tail_with_code_background() {
    let mut app = make_app();
    seed_subagent_popup(&mut app, "```rust\nfn main() {}\n```");

    let terminal = render_subagent_popup_terminal(&mut app, 100, 30);
    let buf = terminal.backend().buffer();
    let code_bg = app.theme.code_block_bg();

    // Find the row containing the code text and assert the tail cells carry
    // the code background all the way to the popup body edge.
    let body = app.mouse.popup_text_body_area;
    let body_right = body.x + body.width - 1;
    let code_row = (0..buf.area.height)
        .find(|&y| {
            (0..buf.area.width).any(|x| buf[(x, y)].symbol() == "m" && buf[(x, y)].bg == code_bg)
        })
        .expect("code row");

    for x in body.x..body_right {
        assert_eq!(
            buf[(x, code_row)].bg,
            code_bg,
            "code row tail must carry the code background at x={x}"
        );
    }
}

#[test]
fn completed_subagent_popup_separates_adjacent_ordered_list_items() {
    let mut app = make_app();
    seed_subagent_popup(&mut app, "1. first item\n2. second item");

    let text = render_subagent_popup_text(&mut app, 100, 30);
    let first = text.lines().position(|line| line.contains("1. first item"));
    let second = text
        .lines()
        .position(|line| line.contains("2. second item"));
    assert!(
        second
            .zip(first)
            .is_some_and(|(second, first)| second >= first + 2),
        "ordered items should have a blank row between them, got:\n{text}"
    );
}

#[test]
fn completed_subagent_popup_wraps_wide_tables_inside_the_body() {
    // A wide pipe table must shrink/wrap at the actual popup body width
    // (width-aware pipeline) instead of rendering at the fixed 80-column
    // card width and overflowing the popup.
    let mut app = make_app();
    seed_subagent_popup(
        &mut app,
        "| Skill | Description |\n| ----- | ----------- |\n| code-reviewer | A very long description that definitely exceeds the available width of the popup body and must wrap inside the table |",
    );

    let terminal = render_subagent_popup_terminal(&mut app, 50, 24);
    let buf = terminal.backend().buffer();
    let body = app.mouse.popup_text_body_area;
    let body_right = body.x + body.width;

    let mut table_rows = 0;
    for y in body.y..body.y + body.height {
        let row_text: String = (body.x..body_right).map(|x| buf[(x, y)].symbol()).collect();
        let trimmed = row_text.trim_end();
        assert!(
            trimmed.chars().count() <= body.width as usize,
            "table row exceeds the popup body width at y={y}:\n{row_text}"
        );
        if trimmed.contains('|') {
            table_rows += 1;
        }
    }
    assert!(
        table_rows >= 3,
        "expected the table (header + separator + wrapped rows) inside the popup, got {table_rows} table rows"
    );
    assert!(
        buffer_contains(buf, "Skill") && buffer_contains(buf, "exceeds"),
        "table content missing:\n{}",
        buffer_text(buf)
    );
}

#[test]
fn completed_subagent_popup_renders_sectioned_headers_in_order() {
    use agent_tui_kit::protocol::{SubagentSection, SubagentSectionBlock, THINKING_SECTION_HEADER};
    let mut app = make_app();
    seed_subagent_popup_sections(
        &mut app,
        &[
            SubagentSectionBlock {
                section: SubagentSection::Thinking,
                text: format!("{THINKING_SECTION_HEADER}\n\nplan the answer"),
            },
            SubagentSectionBlock {
                section: SubagentSection::Tool,
                text: "→ bash ls\n\n✓ main.rs".into(),
            },
            SubagentSectionBlock {
                section: SubagentSection::Context,
                text: "I read the code".into(),
            },
        ],
        "fix the bug",
    );

    let text = render_subagent_popup_text(&mut app, 100, 30);
    // The popup's own display text (single-space emoji headers); the buffer
    // text widens emoji cells with a continuation space.
    let raw = app
        .subagent_popup
        .as_ref()
        .and_then(|p| p.layout_cache.as_ref())
        .map(|c| c.raw_text.clone())
        .expect("layout cache populated");

    // Headers render as `## `-prefixed Markdown headings, in canonical order.
    let thinking = raw.find("## 🧠 Thinking").expect("thinking header");
    let tools = raw.find("## 🔧 Tools").expect("tools header");
    let context = raw.find("## 📄 Context").expect("context header");
    assert!(
        thinking < tools && tools < context,
        "headers out of order:\n{raw}"
    );

    // Section bodies.
    assert!(text.contains("plan the answer"), "{text}");
    assert!(text.contains("→ bash ls"), "{text}");
    assert!(text.contains("✓ main.rs"), "{text}");
    assert!(text.contains("I read the code"), "{text}");

    // Thinking body is indented 4 columns (non-breaking spaces in the
    // completed Markdown pass, so CommonMark does not turn it into code).
    assert!(
        raw.contains("\u{00A0}\u{00A0}\u{00A0}\u{00A0}plan the answer"),
        "thinking body must be indented 4 chars:\n{raw}"
    );

    // The forwarder's thinking marker is stripped from the body: the only
    // occurrence is the section header itself.
    assert_eq!(raw.matches("🧠 Thinking").count(), 1, "{raw}");

    // Prompt appears (bold `Prompt:` label) inside the Context section, after
    // its header.
    let prompt = raw.find("Prompt:").expect("prompt label");
    let prompt_text = raw.find("fix the bug").expect("prompt body");
    assert!(context < prompt, "prompt must live under Context:\n{raw}");
    assert!(
        prompt < prompt_text,
        "prompt text follows its label:\n{raw}"
    );
}

#[test]
fn completed_subagent_popup_single_section_has_no_headers() {
    use agent_tui_kit::protocol::{SubagentSection, SubagentSectionBlock};
    let mut app = make_app();
    seed_subagent_popup_sections(
        &mut app,
        &[SubagentSectionBlock {
            section: SubagentSection::Context,
            text: "just streamed text".into(),
        }],
        "",
    );

    let text = render_subagent_popup_text(&mut app, 100, 30);

    assert!(text.contains("just streamed text"), "{text}");
    assert!(
        !text.contains("## 🧠 Thinking")
            && !text.contains("## 🔧 Tools")
            && !text.contains("## 📄 Context"),
        "single-section transcript must stay flat:\n{text}"
    );
    // No prompt label when the card carries no prompt.
    assert!(!text.contains("Prompt:"), "{text}");
}

#[test]
fn live_subagent_popup_renders_section_headers() {
    use agent_tui_kit::protocol::{SubagentSection, THINKING_SECTION_HEADER};
    use tact_protocol::ToolOutputChunk;
    let mut app = make_app();
    seed_active_subagent_popup(
        &mut app,
        &[
            ToolOutputChunk::other(format!(
                "\n\n{THINKING_SECTION_HEADER}\n\nthinking live\n\n"
            ))
            .with_section(SubagentSection::Thinking),
            ToolOutputChunk::other("\n\n→ bash ls\n\n").with_section(SubagentSection::Tool),
            ToolOutputChunk::other("streaming now"),
        ],
        "live prompt",
    );

    let text = render_subagent_popup_text(&mut app, 100, 30);
    let raw = app
        .subagent_popup
        .as_ref()
        .and_then(|p| p.layout_cache.as_ref())
        .map(|c| c.raw_text.clone())
        .expect("layout cache populated");

    assert!(raw.contains("🧠 Thinking"), "live thinking header:\n{raw}");
    assert!(raw.contains("🔧 Tools"), "live tools header:\n{raw}");
    assert!(raw.contains("📄 Context"), "live context header:\n{raw}");
    assert!(
        raw.contains("    thinking live"),
        "live thinking body must be indented 4 chars:\n{raw}"
    );
    assert!(raw.contains("→ bash ls"), "{raw}");
    assert!(text.contains("streaming now"), "{text}");
    // Live mode shows the prompt label too.
    assert!(raw.contains("Prompt:"), "{raw}");
    assert!(raw.contains("live prompt"), "{raw}");
    // Only one thinking marker (the section header).
    assert_eq!(raw.matches("🧠 Thinking").count(), 1, "{raw}");
}

#[test]
fn live_subagent_popup_without_sections_stays_flat() {
    use tact_protocol::ToolOutputChunk;
    let mut app = make_app();
    seed_active_subagent_popup(
        &mut app,
        &[ToolOutputChunk::other("only context")],
        "prompt",
    );

    let text = render_subagent_popup_text(&mut app, 100, 30);

    assert!(text.contains("only context"), "{text}");
    assert!(
        !text.contains("🧠 Thinking") && !text.contains("🔧 Tools") && !text.contains("📄 Context"),
        "single-section live transcript must stay flat:\n{text}"
    );
}

#[test]
fn sectioned_subagent_popup_blank_separator_carries_theme_background() {
    use agent_tui_kit::protocol::{SubagentSection, SubagentSectionBlock, THINKING_SECTION_HEADER};
    let mut app = make_app();
    seed_subagent_popup_sections(
        &mut app,
        &[
            SubagentSectionBlock {
                section: SubagentSection::Thinking,
                text: format!("{THINKING_SECTION_HEADER}\n\nplan"),
            },
            SubagentSectionBlock {
                section: SubagentSection::Context,
                text: "stream".into(),
            },
        ],
        "",
    );

    let terminal = render_subagent_popup_terminal(&mut app, 100, 30);
    let buf = terminal.backend().buffer();
    let body = app.mouse.popup_text_body_area;
    let surface_bg = app.theme.bg;

    // The blank separator row under the Thinking header must paint the theme
    // background across the whole body width (no highlight band residue).
    let header_y = (body.y..body.y + body.height)
        .find(|&y| (0..body.width).any(|x| buf[(body.x + x, y)].symbol().contains('🧠')))
        .expect("thinking header row");
    for x in body.x..body.x + body.width {
        assert_eq!(
            buf[(x, header_y + 1)].bg,
            surface_bg,
            "blank separator must carry theme.bg at x={x}"
        );
    }
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
fn session_stats_popup_renders_gfm_table() {
    let mut app = make_app();
    let stats = concat!(
        "── Session Stats ──\n",
        "\n",
        "| Metric | Value |\n",
        "|--------|------:|\n",
        "| Elapsed | 1.0s |\n",
    );
    app.handle_agent_update(tact_protocol::AgentUpdate::SessionStats(stats.into()));

    let popup = app
        .system_prompt_popup
        .as_ref()
        .expect("session stats popup");
    // The popup stores raw Markdown source and lays it out at render time.
    assert!(
        popup.source.contains('|'),
        "GFM table source should carry pipes:\n{}",
        popup.source
    );
    assert!(
        popup.source.lines().count() > 3,
        "GFM table source must be multi-line, got {}:\n{}",
        popup.source.lines().count(),
        popup.source
    );

    let text = render_main_area_text(&mut app, 100, 30);
    assert!(
        text.contains("Session Statistics"),
        "popup title missing:\n{text}"
    );
    assert!(text.contains("Metric"), "header missing:\n{text}");
    assert!(text.contains("Elapsed"), "row missing:\n{text}");

    let metric_pos = text.find("Metric").expect("Metric");
    let elapsed_pos = text.find("Elapsed").expect("Elapsed");
    assert!(
        text[metric_pos..elapsed_pos].contains('\n'),
        "Metric and Elapsed must stay on separate rows:\n{text}"
    );
}

#[test]
fn main_area_loading_spinner_when_executing() {
    use std::collections::HashMap;

    use tact_protocol::{AgentUpdate, PlanStep, ToolPresentationInfo};

    let mut app = make_app();
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
        presentation: ToolPresentationInfo::generic("bash"),
    });
    app.append_blank(LogItemKind::SystemTool);
    app.loading_idx = Some(app.log.items.len().saturating_sub(1));

    let text = render_main_area_text(&mut app, 100, 24);

    assert!(
        !text.trim().is_empty(),
        "executing log with loading placeholder should render, got:\n{text}"
    );
}

#[test]
fn open_diff_popup_after_edit_file_step_uses_git_diff() {
    use std::{collections::HashMap, process::Command};

    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus, ToolPresentationInfo};

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
        presentation: ToolPresentationInfo::generic("edit_file"),
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
            presentation: ToolPresentationInfo::generic("edit_file"),
        },
    });

    let phys_idx = app.tools_mut().blocks.last().expect("tool block").phys_idx;
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
    app.tools_mut().popup = Some(DiffPopup {
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
    app.tools_mut().popup = Some(DiffPopup {
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

    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus, ToolPresentationInfo};

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
        presentation: ToolPresentationInfo::generic("edit_file"),
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
            presentation: ToolPresentationInfo::generic("edit_file"),
        },
    });

    let phys_idx = app.tools_mut().blocks.last().expect("tool block").phys_idx;
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

    use tact_protocol::{AgentUpdate, PlanStep, StepResult, StepStatus, ToolPresentationInfo};

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
        presentation: ToolPresentationInfo::generic("read_file"),
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
            presentation: ToolPresentationInfo::generic("read_file"),
        },
    });

    let phys_idx = app.tools_mut().blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup(phys_idx);

    let text = render_main_area_text(&mut app, 100, 30);
    let _ = std::fs::remove_file(&file);

    assert!(
        text.contains("popup_real_path"),
        "open_diff_popup should render file content from StepFinished tool block, got:\n{text}"
    );
}

#[test]
fn tasks_dag_popup_renders_mermaid_markdown() {
    use tact_protocol::{TaskSnapshot, TaskStatusSnapshot};

    let mut app = make_app();
    app.task_panel_mut().snapshot = vec![
        TaskSnapshot {
            id: 1,
            subject: "root".into(),
            status: TaskStatusSnapshot::Completed,
            owner: String::new(),
            blocks: vec![2],
            blocked_by: Vec::new(),
            ..Default::default()
        },
        TaskSnapshot {
            id: 2,
            subject: "child".into(),
            status: TaskStatusSnapshot::Pending,
            owner: String::new(),
            blocks: Vec::new(),
            blocked_by: vec![1],
            ..Default::default()
        },
    ];
    app.open_task_dag_popup();
    let text = render_main_area_text(&mut app, 100, 30);
    assert!(
        text.contains("tasks-dag") || text.contains("DAG"),
        "popup chrome missing, got:\n{text}"
    );
    assert!(
        text.contains('─') || text.contains('│') || text.contains('#'),
        "expected mermaid diagram content, got:\n{text}"
    );
    assert!(
        text.contains("root") && text.contains("child"),
        "legend should list subjects, got:\n{text}"
    );
}
