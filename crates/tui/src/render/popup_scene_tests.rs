//! Render tests for overlay popups (palette, slash, diff, code, thinking, file picker).

use std::time::Duration;

use ratatui::{Terminal, backend::TestBackend, style::Modifier, text::Line};

use super::test_harness::{buffer_text, make_app, render_app_text, render_main_area_text};
use crate::test_fixtures::StepCall;
use crate::widgets::state::{
    App, CodeBlock, CodePopup, DiffPopup, InputMode, LogItemKind, PopupTextSelection, SurfaceId,
    ThinkingBlock, ThinkingPopup,
};

fn seed_diff_popup(app: &mut App) {
    app.tools_mut().popup = Some(DiffPopup {
        title: "read_file".into(),
        tool_name: Some("read_image".into()),
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

/// The popup must show the syntax that follows a subcommand, not just its name:
/// `/mcp auth` is useless advice without `<server>`.
#[test]
fn full_frame_slash_popup_completes_subcommands_with_their_syntax() {
    let mut app = make_app();
    app.input_mode = InputMode::Insert;
    app.input = "/mcp ".into();
    app.input_cursor = app.input.len();
    app.slash_command.active = true;
    app.slash_command.start_pos = 0;

    let text = render_app_text(&mut app, 100, 30);

    for expected in ["/mcp auth", "/mcp login", "/mcp list", "<server>"] {
        assert!(text.contains(expected), "missing {expected} in:\n{text}");
    }
}

/// Index of the `skill-<n>` candidate in the `/skill ` popup.
///
/// Derived from the live candidate list rather than hardcoded: the built-in
/// subcommands come first, so a new one shifts every skill down and a magic
/// index silently starts asserting about the wrong row.
fn skill_row(app: &App, n: usize) -> usize {
    app.slash_candidates()
        .iter()
        .position(|candidate| candidate.path == format!("skill skill-{n:02}"))
        .expect("seeded skill is offered under /skill")
}

/// Seed a long slash list: the two built-in skill subcommands plus `count`
/// skill entries — which is where a long list lives now that skills are not
/// first-level entries.
fn seed_slash_skills(app: &mut App, count: usize) {
    use crate::widgets::state::SkillEntry;
    app.skills_data = (0..count)
        .map(|i| SkillEntry {
            name: format!("skill-{i:02}"),
            description: format!("Skill number {i} description"),
            body: String::new(),
        })
        .collect();
}

/// Open the popup on `/skill `, the level that carries the many entries.
fn open_slash_popup(app: &mut App) {
    app.input_mode = InputMode::Insert;
    app.input = "/skill ".into();
    app.input_cursor = app.input.len();
    app.slash_command.active = true;
    app.slash_command.start_pos = 0;
    app.slash_command.selected = 0;
}

#[test]
fn slash_popup_long_list_scrolls_selected_into_view() {
    let mut app = make_app();
    open_slash_popup(&mut app);
    seed_slash_skills(&mut app, 40);

    app.slash_command.selected = skill_row(&app, 30);
    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("skill-30"),
        "deep selection must be visible after scrolling, got:\n{text}"
    );
    assert!(
        !text.contains("/skill list"),
        "the top of the list must have scrolled out of view, got:\n{text}"
    );
}

#[test]
fn slash_popup_long_list_keeps_selected_visible_on_short_terminal() {
    let mut app = make_app();
    open_slash_popup(&mut app);
    seed_slash_skills(&mut app, 40);

    // Main area is only 7 rows tall on a 13-row terminal; the popup window is
    // clamped to what actually fits, so the selected row must never land
    // below the popup border (previously the anchor was off-screen and the
    // list appeared frozen / "did not scroll").
    app.slash_command.selected = skill_row(&app, 30);
    let text = render_app_text(&mut app, 100, 13);

    assert!(
        text.contains("skill-30"),
        "selected row must stay visible on a short terminal, got:\n{text}"
    );

    // The very last item must also be reachable on a short terminal.
    app.slash_command.selected = skill_row(&app, 39);
    let text = render_app_text(&mut app, 100, 13);
    assert!(
        text.contains("skill-39"),
        "last item must be reachable on a short terminal, got:\n{text}"
    );
}

#[test]
fn slash_popup_scroll_window_moves_with_selection() {
    let mut app = make_app();
    open_slash_popup(&mut app);
    seed_slash_skills(&mut app, 40);

    let top = render_app_text(&mut app, 100, 30);
    assert!(
        top.contains("/skill list"),
        "top of list shows the first entry, got:\n{top}"
    );

    app.slash_command.selected = skill_row(&app, 30);
    let deep = render_app_text(&mut app, 100, 30);
    assert!(
        deep.contains("skill-30") && !deep.contains("/skill list"),
        "moving the selection deep into the list must scroll the window, got:\n{deep}"
    );
}

#[test]
fn slash_popup_records_and_clears_mouse_area() {
    let mut app = make_app();
    open_slash_popup(&mut app);
    seed_slash_skills(&mut app, 5);

    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        !app.mouse.area(SurfaceId::SlashPopup).is_empty(),
        "active slash popup must record its area for mouse-wheel routing"
    );

    app.slash_command.active = false;
    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        app.mouse.area(SurfaceId::SlashPopup).is_empty(),
        "closed slash popup must clear its mouse area"
    );
}

/// Seed the select popup (model picker) with `count` options.
fn open_select_popup(app: &mut App, count: usize) {
    use crate::widgets::state::{ModelTarget, SelectKind};
    app.input_mode = InputMode::Select;
    app.select_kind = SelectKind::ModelPick(ModelTarget::Main);
    app.select.set_local(
        "Select model".into(),
        (0..count).map(|i| format!("model-{i:02}")).collect(),
        0,
        false,
    );
}

#[test]
fn select_popup_long_list_scrolls_selected_into_view() {
    let mut app = make_app();
    open_select_popup(&mut app, 30);
    app.select.selected = 25;

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("▶ model-25"),
        "deep selection must be visible after scrolling, got:\n{text}"
    );
    assert!(
        !text.contains("model-00"),
        "the top of the list must have scrolled out of view, got:\n{text}"
    );
}

#[test]
fn select_popup_long_list_keeps_selected_visible_on_short_terminal() {
    let mut app = make_app();
    open_select_popup(&mut app, 30);
    app.select.selected = 25;

    let text = render_app_text(&mut app, 100, 13);

    assert!(
        text.contains("▶ model-25"),
        "selected row must stay visible on a short terminal, got:\n{text}"
    );

    // The very last item must also be reachable on a short terminal.
    app.select.selected = 29;
    let text = render_app_text(&mut app, 100, 13);
    assert!(
        text.contains("▶ model-29"),
        "last item must be reachable on a short terminal, got:\n{text}"
    );
}

#[test]
fn select_popup_window_moves_with_selection() {
    let mut app = make_app();
    open_select_popup(&mut app, 30);

    let top = render_app_text(&mut app, 100, 30);
    assert!(
        top.contains("▶ model-00"),
        "top of list shows the first option, got:\n{top}"
    );

    app.select.selected = 25;
    let deep = render_app_text(&mut app, 100, 30);
    assert!(
        deep.contains("▶ model-25") && !deep.contains("model-00"),
        "moving the selection deep into the list must scroll the window, got:\n{deep}"
    );
}

#[test]
fn select_popup_records_and_clears_mouse_area() {
    let mut app = make_app();
    open_select_popup(&mut app, 5);

    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        !app.mouse.area(SurfaceId::SelectPopup).is_empty(),
        "active select popup must record its area for mouse-wheel routing"
    );

    app.input_mode = InputMode::Normal;
    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        app.mouse.area(SurfaceId::SelectPopup).is_empty(),
        "closed select popup must clear its mouse area"
    );
}

#[test]
fn select_popup_shows_navigation_footer_hint() {
    let mut app = make_app();
    open_select_popup(&mut app, 5);

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("↑↓"),
        "select popup must show the arrow navigation hint in its footer, got:\n{text}"
    );
    // A local pick is filterable, so `j`/`k` are filter characters here and
    // must not be advertised as navigation.
    assert!(
        !text.contains("↑↓/j/k"),
        "a filterable select must not advertise j/k as navigation, got:\n{text}"
    );
    assert!(
        text.contains("a-z") && (text.contains("Filter") || text.contains("筛选")),
        "select popup footer must offer the filter, got:\n{text}"
    );
    assert!(
        text.contains("Enter") && text.contains("Esc"),
        "select popup footer must list Enter/Esc, got:\n{text}"
    );
    assert!(
        text.contains("Confirm") || text.contains("确认"),
        "select popup footer must label the confirm action, got:\n{text}"
    );
}

#[test]
fn select_popup_multi_shows_toggle_footer_hint() {
    let mut app = make_app();
    app.input_mode = InputMode::Select;
    app.select.set_multi(
        "Pick options".into(),
        (0..3).map(|i| format!("opt-{i}")).collect(),
        1,
        false,
    );

    let text = render_app_text(&mut app, 100, 30);

    assert!(
        text.contains("Space") && (text.contains("Toggle") || text.contains("勾选")),
        "multi select footer must list the Space toggle, got:\n{text}"
    );
    assert!(
        text.contains("↑↓/j/k"),
        "multi select footer must still show navigation, got:\n{text}"
    );
}

/// The `/model` list is not a renderer of its own: `start_model_picker` builds
/// a `SelectPopup` and `SelectPopupWidget` renders it through the shared
/// `ListPopup`. This pins that path, marker and band included.
#[test]
fn model_picker_renders_through_the_shared_list_popup() {
    use crate::widgets::state::{ModelTarget, SelectKind};

    let mut app = make_app();
    app.input_mode = InputMode::Select;
    app.select_kind = SelectKind::ModelPick(ModelTarget::Main);
    // Exactly what `start_model_picker` builds: the current model carries `*`.
    app.select.set_local(
        "Select model".into(),
        vec!["model-a".into(), "model-b *".into(), "model-c".into()],
        1,
        false,
    );

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();

    // The current-model marker survives the migration.
    let focused = cell_at(buf, "model-b *");
    assert_eq!(focused.fg, app.theme.fg, "the focused row is theme.fg");
    assert_eq!(focused.bg, app.theme.highlight, "on theme.highlight");

    // And the band is the component's row-wide one, not a per-glyph patch.
    let area = app.mouse.area(SurfaceId::SelectPopup);
    assert!(
        !area.is_empty(),
        "the model picker must record its mouse area"
    );
    let y = row_y(buf, "model-b *");
    for x in (area.x + 1)..area.right() - 1 {
        assert_eq!(
            buf[(x, y)].bg,
            app.theme.highlight,
            "the focused model row's band must span the popup (x={x})"
        );
    }
}

/// Typing narrows the model list: the rows the filter hides must be gone, and
/// the filter itself must be on screen.
#[test]
fn model_picker_filter_hides_the_rows_that_do_not_match() {
    use crate::widgets::state::{ModelTarget, SelectKind};

    let mut app = make_app();
    app.input_mode = InputMode::Select;
    app.select_kind = SelectKind::ModelPick(ModelTarget::Main);
    app.select.set_local(
        "Select model".into(),
        vec![
            "kimi-k2.5".into(),
            "kimi-for-coding".into(),
            "claude-sonnet".into(),
        ],
        0,
        false,
    );

    let unfiltered = render_popup_only(&mut app, 100, 30, |f, area, app| {
        super::render_select_popup(f, area, app)
    });
    for model in ["kimi-k2.5", "kimi-for-coding", "claude-sonnet"] {
        assert!(unfiltered.contains(model), "unfiltered list misses {model}");
    }
    // An empty filter line says what it is: a magnifier and a grey placeholder,
    // not a bare `>` that reads as another row of the list.
    assert!(
        unfiltered.contains('\u{1f50d}'),
        "the filter line needs a search icon:\n{unfiltered}"
    );
    assert!(
        unfiltered.contains(app.msgs().select_filter_placeholder),
        "an empty filter line needs its placeholder:\n{unfiltered}"
    );

    for c in "cod".chars() {
        app.select.push_query(c);
    }
    let filtered = render_popup_only(&mut app, 100, 30, |f, area, app| {
        super::render_select_popup(f, area, app)
    });

    assert!(
        filtered.contains('\u{1f50d}') && filtered.contains("cod"),
        "the filter line must show what is being filtered on:\n{filtered}"
    );
    assert!(
        !filtered.contains(app.msgs().select_filter_placeholder),
        "the placeholder must give way to the query:\n{filtered}"
    );
    assert!(
        filtered.contains("kimi-for-coding"),
        "the match must stay:\n{filtered}"
    );
    for hidden in ["kimi-k2.5", "claude-sonnet"] {
        assert!(
            !filtered.contains(hidden),
            "the filter must hide {hidden}:\n{filtered}"
        );
    }

    // A filter that matches nothing says so, instead of claiming the list is
    // empty.
    app.select.clear_query();
    app.select.push_query('z');
    let empty = render_popup_only(&mut app, 100, 30, |f, area, app| {
        super::render_select_popup(f, area, app)
    });
    assert!(
        empty.contains(app.msgs().select_no_match),
        "a filter matching nothing must say so:\n{empty}"
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

/// Render one list popup into its own buffer: no status bar or input box noise,
/// so a row assertion is about the popup and nothing else.
fn render_popup_only(
    app: &mut App,
    width: u16,
    height: u16,
    draw: fn(&mut ratatui::Frame, ratatui::layout::Rect, &mut App),
) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            let area = frame.area();
            draw(frame, area, app);
        })
        .expect("draw");
    buffer_text(terminal.backend().buffer())
}

fn render_palette_only(app: &mut App, width: u16, height: u16) -> String {
    render_popup_only(app, width, height, |f, area, app| {
        super::render_command_palette(f, area, app)
    })
}

fn render_file_picker_only(app: &mut App, width: u16, height: u16) -> String {
    render_popup_only(app, width, height, |f, area, app| {
        super::render_file_picker(f, area, app)
    })
}

#[test]
fn command_palette_long_list_scrolls_selected_into_view() {
    let mut app = make_app();
    app.input_mode = InputMode::Palette;
    let quit = app
        .palette_commands()
        .iter()
        .position(|(cmd, _)| cmd == "quit")
        .expect("the palette lists quit");

    // Short terminal: the frame fits two rows, so the window has to move.
    app.palette_selected = quit;
    let text = render_palette_only(&mut app, 100, 8);

    assert!(
        text.contains("quit"),
        "the focused command must stay inside the popup:\n{text}"
    );
    assert!(
        !text.contains("🎨"),
        "the first command must have scrolled out of view:\n{text}"
    );
}

#[test]
fn command_palette_records_and_clears_mouse_area() {
    let mut app = make_app();
    app.input_mode = InputMode::Palette;

    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        !app.mouse.area(SurfaceId::PalettePopup).is_empty(),
        "an open palette must record its area for mouse-wheel routing"
    );

    app.input_mode = InputMode::Normal;
    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        app.mouse.area(SurfaceId::PalettePopup).is_empty(),
        "a closed palette must clear its mouse area"
    );
}

#[test]
fn file_picker_long_list_scrolls_selected_into_view() {
    let mut app = make_app();
    app.input_mode = InputMode::FilePicker;
    app.file_picker.options = (0..30).map(|i| format!("file-{i:02}.rs")).collect();
    app.file_picker.current_dir = app.work_dir.clone();
    app.file_picker.base_dir = app.work_dir.clone();
    app.file_picker.selected = 25;

    let text = render_file_picker_only(&mut app, 100, 12);

    assert!(
        text.contains("file-25.rs"),
        "the focused file must stay inside the popup:\n{text}"
    );
    assert!(
        !text.contains("file-00.rs"),
        "the first file must have scrolled out of view:\n{text}"
    );
}

#[test]
fn file_picker_records_and_clears_mouse_area() {
    let mut app = make_app();
    app.input_mode = InputMode::FilePicker;
    app.file_picker.options = vec!["src/main.rs".into()];

    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        !app.mouse.area(SurfaceId::FilePickerPopup).is_empty(),
        "an open file picker must record its area for mouse-wheel routing"
    );

    app.input_mode = InputMode::Normal;
    let _ = render_app_text(&mut app, 100, 30);
    assert!(
        app.mouse.area(SurfaceId::FilePickerPopup).is_empty(),
        "a closed file picker must clear its mouse area"
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
fn tool_popup_bottom_border_names_the_tool_and_its_keys() {
    let mut app = make_app();
    seed_diff_popup(&mut app);

    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let area = app.mouse.area(SurfaceId::DiffPopup);
    assert!(!area.is_empty(), "tool popup must have rendered");

    // Read the bottom border row itself: the tool id belongs there, not on the
    // title row (which names the file / command instead).
    let buffer = terminal.backend().buffer();
    let bottom: String = (0..buffer.area.width)
        .map(|x| buffer[(x, area.bottom() - 1)].symbol().to_string())
        .collect();

    assert!(
        bottom.contains("read_image"),
        "tool id missing from the popup bottom border: {bottom}"
    );
    for hint in ["copy", "close", "scroll"] {
        assert!(
            bottom.contains(hint),
            "the {hint} hint is missing from the popup bottom border: {bottom}"
        );
    }
    assert!(
        !bottom.contains("read_file"),
        "the title must stay on the top border: {bottom}"
    );

    // The border row keeps the theme background on every column, footer text
    // included — otherwise the note would leave a default-bg patch behind it.
    let theme_bg = app.theme.bg;
    assert!(
        (area.left()..area.right()).all(|x| buffer[(x, area.bottom() - 1)].bg == theme_bg),
        "the popup bottom border row must carry the theme background"
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
        !buffer[(app.mouse.area(SurfaceId::ThinkingPopup).x, row.screen_y)]
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
    app.select
        .set("Pick one".into(), vec!["A".into(), "B".into()], 0, false);

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
fn background_popup_keeps_the_listing_out_of_the_log() {
    let mut app = make_app();
    let listing = "```text\n018f3a2c  running   cargo build\n```";
    let log_len = app.log.items.len();
    app.handle_agent_update(tact_protocol::AgentUpdate::PopupMarkdown {
        title: "⚙️ Background Tasks".to_string(),
        source: listing.to_string(),
    });

    let popup = app.system_prompt_popup.as_ref().expect("background popup");
    assert_eq!(popup.title, "⚙️ Background Tasks");
    assert_eq!(popup.source, listing);
    assert_eq!(
        app.log.items.len(),
        log_len,
        "the listing is a read-out: it must not join the log"
    );

    let text = render_main_area_text(&mut app, 100, 30);
    assert!(
        text.contains("Background Tasks"),
        "popup title missing:\n{text}"
    );
    assert!(
        text.contains("018f3a2c"),
        "task row missing from the popup:\n{text}"
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
    app.handle_agent_update(tact_protocol::AgentUpdate::PopupMarkdown {
        title: "Session Statistics".to_string(),
        source: stats.to_string(),
    });

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

    // The session-stats popup reuses the system-prompt popup chrome, which
    // must show the bottom-border navigation hint like other scrollable
    // popups (code/mermaid).
    assert!(
        text.contains("j/k") && text.contains("scroll"),
        "session stats popup footer must show the j/k scroll hint, got:\n{text}"
    );
    assert!(
        text.contains("Esc") && text.contains("close"),
        "session stats popup footer must show Esc close, got:\n{text}"
    );

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

    use tact_protocol::{AgentUpdate, PlanStep};

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
    app.handle_agent_update(StepCall::new(0, "bash1", "bash", "sleep 1").started());
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

    use tact_protocol::{AgentUpdate, PlanStep};

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
    app.handle_agent_update(StepCall::new(0, "edit_popup", "edit_file", path.clone()).started());
    app.handle_agent_update(
        StepCall::new(0, "edit_popup", "edit_file", path.clone())
            .message("wrote")
            .detail("fn new() {}")
            .duration_us(100)
            .finished(),
    );

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
        tool_name: None,
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
        tool_name: None,
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

    use tact_protocol::{AgentUpdate, PlanStep};

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
    app.handle_agent_update(StepCall::new(0, "edit_calc", "edit_file", path.clone()).started());
    app.handle_agent_update(
        StepCall::new(0, "edit_calc", "edit_file", path.clone())
            .message("wrote")
            .detail("fn add(a: i32, b: i32) -> i32 {\n    a - b\n}")
            .duration_us(100)
            .finished(),
    );

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

    use tact_protocol::{AgentUpdate, PlanStep};

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
    app.handle_agent_update(StepCall::new(0, "read_popup", "read_file", path.clone()).started());
    app.handle_agent_update(
        StepCall::new(0, "read_popup", "read_file", path.clone())
            .detail("fn popup_real_path() {}")
            .duration_us(100)
            .finished(),
    );

    let phys_idx = app.tools_mut().blocks.last().expect("tool block").phys_idx;
    app.open_diff_popup(phys_idx);

    assert_eq!(
        app.tools_mut()
            .popup
            .as_ref()
            .and_then(|p| p.tool_name.as_deref()),
        Some("read_file"),
        "the popup must carry the raw tool id of the block it was opened from"
    );

    let text = render_main_area_text(&mut app, 100, 30);
    let _ = std::fs::remove_file(&file);

    assert!(
        text.contains("popup_real_path"),
        "open_diff_popup should render file content from StepFinished tool block, got:\n{text}"
    );
    assert!(
        text.contains("read_file"),
        "the tool id must reach the popup bottom border, got:\n{text}"
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

/// Buffer-level background check for the `/tasks-dag` overlay.
///
/// It is the newest popup drawn through the kit's `ScrollableTextPopup`
/// skeleton, and the log behind it is full of wide (CJK / box-drawing) glyphs —
/// exactly the case where a cell "restored" by a narrower repaint keeps a stale
/// style. Every cell of the popup rect must therefore carry `theme.bg`, not just
/// the rows that happen to hold text (see `docs/tui_rendering.md` and the
/// no-shadow rule in `AGENTS.md`).
#[test]
fn tasks_dag_popup_paints_the_theme_background_over_its_whole_rect() {
    use tact_protocol::{TaskSnapshot, TaskStatusSnapshot};

    let mut app = make_app();
    // Seed the log with wide glyphs so a leave-behind would be visible.
    app.handle_agent_update(tact_protocol::AgentUpdate::StreamChunk(
        "│ ── 中文宽字符 ── │\n".repeat(4),
    ));
    app.task_panel_mut().apply_snapshot(vec![TaskSnapshot {
        id: 1,
        subject: "root".into(),
        status: TaskStatusSnapshot::Pending,
        owner: String::new(),
        blocks: vec![2],
        blocked_by: Vec::new(),
        ..Default::default()
    }]);
    app.open_task_dag_popup();

    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let area = app.mouse.area(SurfaceId::TaskDagPopup);
    assert!(!area.is_empty(), "the overlay must have rendered");

    let buffer = terminal.backend().buffer();
    let theme_bg = app.theme.bg;
    let mut wrong = Vec::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            if buffer[(x, y)].bg != theme_bg {
                wrong.push(format!("({x},{y}) bg={:?}", buffer[(x, y)].bg));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "every cell of the popup must carry the theme background; offenders: {wrong:?}"
    );
}

#[test]
fn popup_footer_swaps_the_copy_hint_for_the_confirmation_after_a_copy() {
    let mut app = make_app();
    seed_code_popup(&mut app);

    // Nothing copied yet: the footer offers the key pair as usual.
    let before = render_main_area_text(&mut app, 100, 30);
    assert!(
        before.contains("copy"),
        "the copy hint must be offered:\n{before}"
    );
    assert!(
        !before.contains("Copied"),
        "no confirmation before a copy:\n{before}"
    );

    // A copy landed: the footer confirms instead of offering the key.
    app.copy_text("fn main() {}");
    let terminal = render_main_area_terminal(&mut app, 100, 30);
    let area = app.mouse.area(SurfaceId::CodePopup);
    assert!(!area.is_empty(), "code popup must have rendered");
    let buffer = terminal.backend().buffer();
    let bottom: String = (0..buffer.area.width)
        .map(|x| buffer[(x, area.bottom() - 1)].symbol().to_string())
        .collect();
    assert!(
        bottom.contains("Copied"),
        "the confirmation must replace the hint: {bottom}"
    );
    assert!(
        !bottom.contains("copy "),
        "the old hint must be gone while the confirmation shows: {bottom}"
    );

    // Drawn in the success color, on the popup's own background. Locate the
    // glyphs by their cell suffix, not by the flattened string index: a wide
    // grapheme earlier in the row leaves an empty continuation cell and shifts
    // every string offset past it.
    let theme = app.theme;
    let row = area.bottom() - 1;
    let copied_at = (0..buffer.area.width)
        .find(|x| {
            let suffix: String = (*x..buffer.area.width)
                .map(|col| buffer[(col, row)].symbol())
                .collect();
            suffix.starts_with("Copied")
        })
        .expect("confirmation column");
    for x in copied_at..copied_at + "Copied".len() as u16 {
        let cell = &buffer[(x, row)];
        assert_eq!(
            cell.fg, theme.success,
            "confirmation must use the success color"
        );
        assert_eq!(
            cell.bg, theme.bg,
            "the footer row keeps the popup background"
        );
    }
}

// ===== Popups must draw with the theme, not with literals =====
//
// Regression: the slash popup (and the palette, the file picker and the select
// popup) styled their rows with `Color::White` / `Color::Cyan` /
// `Color::DarkGray`. Under a light theme the popup's own background is white
// (`Theme::bg`), so every unselected row was white-on-white — the command list
// was invisible, and the highlighted row came out in Dark's cyan while the
// theme's accent is blue. The colors now come from `Theme`.

/// Row index of the first row whose flattened text contains `needle`.
fn row_y(buf: &ratatui::buffer::Buffer, needle: &str) -> u16 {
    for y in 0..buf.area.height {
        let row: String = (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect();
        if row.contains(needle) {
            return y;
        }
    }
    panic!("{needle:?} not found in:\n{}", buffer_text(buf));
}

/// The style of the cell where `needle` starts: scans the buffer cell by cell
/// so a wide glyph (emoji, CJK) earlier in the row cannot shift the answer.
fn cell_at<'a>(buf: &'a ratatui::buffer::Buffer, needle: &str) -> &'a ratatui::buffer::Cell {
    let width = buf.area.width;
    for y in 0..buf.area.height {
        for x in 0..width {
            let mut candidate = String::new();
            for dx in 0..needle.chars().count() as u16 {
                if let Some(cell) = buf.cell((x + dx, y)) {
                    candidate.push_str(cell.symbol());
                }
            }
            if candidate == needle {
                return &buf[(x, y)];
            }
        }
    }
    panic!("{needle:?} not found in:\n{}", buffer_text(buf));
}

fn light_app() -> App {
    use agent_tui_kit::theme::{Theme, ThemeName};
    let mut app = make_app();
    app.theme = Theme::from(ThemeName::Light);
    app
}

#[test]
fn slash_popup_rows_take_their_colors_from_the_theme() {
    let mut app = light_app();
    // The command list, not the `/skill ` subcommands: this is the surface the
    // regression was reported on.
    app.input_mode = InputMode::Insert;
    app.input = "/".into();
    app.input_cursor = 1;
    app.slash_command.active = true;
    app.slash_command.start_pos = 0;

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();

    assert_eq!(
        cell_at(buf, "/theme").fg,
        app.theme.accent,
        "the highlighted row is the theme's accent"
    );
    assert_eq!(
        cell_at(buf, "/model").fg,
        app.theme.fg,
        "an unselected row is the theme's foreground — white here would be \
         white-on-white, because the popup background is `theme.bg`"
    );
    assert_ne!(cell_at(buf, "/model").fg, ratatui::style::Color::White);
}

#[test]
fn command_palette_selected_row_is_legible_on_a_light_theme() {
    let mut app = light_app();
    app.input_mode = InputMode::Palette;

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();

    let selected = cell_at(buf, "theme");
    assert_eq!(
        selected.fg, app.theme.fg,
        "the selected palette row puts `theme.fg` on `theme.highlight`"
    );
    assert_eq!(selected.bg, app.theme.highlight);
}

#[test]
fn file_picker_selected_row_is_legible_on_a_light_theme() {
    let mut app = light_app();
    app.input_mode = InputMode::FilePicker;
    app.file_picker.options = vec!["src/main.rs".into(), "Cargo.toml".into()];
    app.file_picker.current_dir = app.work_dir.clone();
    app.file_picker.base_dir = app.work_dir.clone();

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();

    assert_eq!(
        cell_at(buf, "main.rs").fg,
        app.theme.fg,
        "the selected file row must not use a literal white"
    );
}

#[test]
fn select_popup_rows_and_empty_hint_take_their_colors_from_the_theme() {
    let mut app = light_app();
    open_select_popup(&mut app, 3);

    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();

    assert_eq!(
        cell_at(buf, "model-00").fg,
        app.theme.fg,
        "the selected option is `theme.fg` over `theme.highlight`"
    );

    // And the empty state is muted, not a literal gray.
    let mut app = light_app();
    open_select_popup(&mut app, 0);
    let backend = TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
        .expect("draw");
    let buf = terminal.backend().buffer();
    let empty = app.msgs().select_empty;
    assert_eq!(cell_at(buf, empty).fg, app.theme.muted);
}

/// The invariant the literals broke, over every built-in theme: no popup row
/// may be painted in the popup's own background color.
#[test]
fn no_theme_draws_a_popup_row_in_its_own_background_color() {
    use agent_tui_kit::theme::{Theme, ThemeName};

    let mut unreadable = Vec::new();
    // `next()` cycles the full set; `ThemeName::all()` is private to the kit.
    let mut name = ThemeName::Dark;
    loop {
        let theme = Theme::from(name);
        let mut app = make_app();
        app.theme = theme;
        app.input_mode = InputMode::Insert;
        app.input = "/".into();
        app.input_cursor = 1;
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;

        let backend = TestBackend::new(100, 30);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| super::test_harness::draw_full_ui(frame, frame.area(), &mut app))
            .expect("draw");
        let buf = terminal.backend().buffer();

        let row = cell_at(buf, "/model");
        if row.fg == theme.bg {
            unreadable.push(format!("{name:?}: fg={:?} on bg={:?}", row.fg, theme.bg));
        }

        name = name.next();
        if name == ThemeName::Dark {
            break;
        }
    }
    assert!(
        unreadable.is_empty(),
        "popup rows painted in the popup background:\n{}",
        unreadable.join("\n")
    );
}

/// The scrollbar is drawn over the *popup* rect, not the body.
///
/// Four popups reach the bar through
/// `agent_tui_kit::render::popups::render_popup_scrollbar`, so a wrong rect here
/// moves the bar inward in all of them at once. `popup_inner` has already
/// excluded the border, so the bar's column is the last column of `popup_area`
/// and one past the body's right edge.
#[test]
fn the_popup_scrollbar_uses_the_popup_column_not_the_body_column() {
    let popup_area = ratatui::layout::Rect::new(5, 2, 12, 6);
    let backend = TestBackend::new(30, 12);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| {
            agent_tui_kit::render::popups::render_popup_scrollbar(frame, popup_area, 40, 6, 0);
        })
        .expect("draw");
    let buf = terminal.backend().buffer();

    let bar_col = popup_area.right() - 1;
    let painted = (popup_area.y..popup_area.bottom())
        .filter(|y| buf[(bar_col, *y)].symbol() != " ")
        .count();
    assert!(
        painted > 0,
        "the bar drew nothing in the popup's right column"
    );

    // Nothing may land in the column just outside the popup.
    for y in popup_area.y..popup_area.bottom() {
        assert_eq!(
            buf[(popup_area.right(), y)].symbol(),
            " ",
            "the bar spilled past the popup at row {y}"
        );
    }
}
