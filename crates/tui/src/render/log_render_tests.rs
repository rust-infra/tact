//! Log panel render coverage: P0 interaction, P1 content shapes, P2 chrome/edge cases.

use std::collections::HashMap;

use ratatui::{Terminal, backend::TestBackend, style::Modifier, text::Line};
use tact_protocol::{PlanStep, RuntimeEvent, StepStatus, ThinkingChunk, ToolPresentationInfo};

use super::log::render_log_panel;
use super::test_harness::{
    buffer_has_bg, buffer_has_modifier, buffer_text, make_app, render_log_panel_terminal,
    render_log_panel_text,
};
use crate::test_fixtures::StepCall;
use crate::widgets::state::{App, LogItemKind, LogSelection, Status};
use crate::widgets::tool_widget::TOOL_HEADER_ROWS;
use agent_tui_kit::widgets::button::Button;

fn seed_many_numbered_lines(app: &mut App, count: usize) {
    for i in 0..count {
        app.add_system_message(format!("log-row-{i:02}"));
    }
}

/// A finished `spawn_subagent` with many lines: the one successful tool left
/// that keeps a tall content card (commands, reads, writes and edits collapse to
/// their two header rows).
fn seed_tall_subagent_tool(app: &mut App, line_count: usize) {
    let output: String = (1..=line_count)
        .map(|n| format!("child-out-{n:02}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.handle_runtime_event(RuntimeEvent::StepAdded {
        run_id: None,
        step: PlanStep::new(
            "audit the repo",
            "spawn_subagent",
            "sub-tall",
            HashMap::from([("prompt".to_string(), "audit the repo".to_string())]),
        ),
    });
    app.handle_runtime_event(
        StepCall::new(0, "sub-tall", "spawn_subagent", "audit the repo").started(),
    );
    app.handle_runtime_event(
        StepCall::new(0, "sub-tall", "spawn_subagent", "audit the repo")
            .detail(output)
            .duration_us(100)
            .finished(),
    );
}

fn line_column_of(rendered: &str, needle: &str) -> Option<usize> {
    rendered.lines().find_map(|line| line.find(needle))
}

fn line_index_of(rendered: &str, needle: &str) -> Option<usize> {
    rendered.lines().position(|line| line.contains(needle))
}

fn buffer_column_of(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<u16> {
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let suffix: String = (x..buffer.area.width)
                .map(|col| buffer[(col, y)].symbol())
                .collect();
            if suffix.starts_with(needle) {
                return Some(x);
            }
        }
    }
    None
}

fn buffer_cell_of(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, u16)> {
    for y in 0..buffer.area.height {
        for x in 0..buffer.area.width {
            let suffix: String = (x..buffer.area.width)
                .map(|col| buffer[(col, y)].symbol())
                .collect();
            if suffix.starts_with(needle) {
                return Some((x, y));
            }
        }
    }
    None
}

// ── P0: selection, scroll ───────────────────────────────────────────────────

#[test]
fn log_line_selection_applies_reversed_modifier() {
    let mut app = make_app();
    app.add_system_message("select this entire line".into());
    let raw = app.log.items[0].raw.clone();
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

    app.log_scroll.visual_top = 0;
    let top = render_log_panel_text(&mut app, 60, 10);
    assert!(
        top.contains("log-row-00"),
        "at offset 0 the first row should be visible, got:\n{top}"
    );

    app.log_scroll.visual_top = usize::MAX;
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

#[test]
fn tall_markdown_cell_is_fully_traversable() {
    // Regression: a whole-Markdown message taller than the viewport (a long
    // `/skill list` table) used to be reachable only at its top/bottom; the
    // middle rows could never be scrolled into view.
    let mut app = make_app();
    let mut md = String::from("## Big table\n\n| Row | V |\n| --- | --- |\n");
    for i in 0..60 {
        if i > 0 {
            md.push_str("| --- | --- |\n");
        }
        md.push_str(&format!("| row-{i:02} | v |\n"));
    }
    app.append_markdown(md);

    // 60x10 terminal → 8 content lines for the bordered panel.
    let viewport_height = 8usize;
    let _ = render_log_panel_text(&mut app, 60, 10);
    app.scroll_log_to_top();

    let step = crate::widgets::state::app::scroll::key_cell_step(viewport_height);
    let mut seen_top = false;
    let mut seen_mid = false;
    let mut seen_bottom = false;
    for _ in 0..200 {
        let text = render_log_panel_text(&mut app, 60, 10);
        seen_top |= text.contains("row-00");
        seen_mid |= text.contains("row-30");
        seen_bottom |= text.contains("row-59");
        if seen_top && seen_mid && seen_bottom {
            break;
        }
        app.scroll_log_down(step);
    }
    assert!(
        seen_top && seen_mid && seen_bottom,
        "tall cell rows unreachable: top={seen_top} mid={seen_mid} bottom={seen_bottom}"
    );

    // Traverse back up: the first row must be reachable again.
    let mut back_to_top = false;
    for _ in 0..200 {
        let text = render_log_panel_text(&mut app, 60, 10);
        if text.contains("row-00") {
            back_to_top = true;
            break;
        }
        app.scroll_log_up(step);
    }
    assert!(back_to_top, "scrolling up must return to the first row");
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
    app.handle_runtime_event(RuntimeEvent::Text {
        run_id: None,
        role: "assistant".into(),
        content: "assistant reply".into(),
    });

    let text = render_log_panel_text(&mut app, 80, 20);
    assert!(
        text.contains("user task") && text.contains("assistant reply"),
        "log should render both user and assistant content after category gap, got:\n{text}"
    );
    let user_line = line_index_of(&text, "user task").expect("user line");
    let assistant_line = line_index_of(&text, "assistant reply").expect("assistant line");
    assert!(
        assistant_line >= user_line + 2,
        "assistant message should have a blank line after the user message (user={user_line}, assistant={assistant_line})"
    );
}

#[test]
fn log_assistant_reply_aligns_with_thinking_indent() {
    let mut app = make_app();
    app.add_user_message("user task".into());
    app.handle_runtime_event(RuntimeEvent::Thinking {
        run_id: None,
        chunk: ThinkingChunk::Delta("thinking reference".into()),
    });
    app.handle_runtime_event(RuntimeEvent::Thinking {
        run_id: None,
        chunk: ThinkingChunk::Finished,
    });
    app.add_system_message("final assistant reply".into());

    let terminal = render_log_panel_terminal(&mut app, 80, 20);
    let buffer = terminal.backend().buffer();
    let thinking_x = buffer_column_of(buffer, "thinking reference").expect("thinking line");
    let assistant_x = buffer_column_of(buffer, "final assistant reply").expect("assistant line");
    let user_x = buffer_column_of(buffer, "💬").expect("user line");

    assert_eq!(
        assistant_x, thinking_x,
        "normal assistant replies should align with Thinking body text"
    );
    assert!(
        user_x < assistant_x,
        "user messages should stay left of the indented assistant reply"
    );
}

#[test]
fn log_task_end_separator_renders_solid_rule() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));
    app.add_task_end_separator();

    let text = render_log_panel_text(&mut app, 60, 12);
    assert!(
        text.contains('─'),
        "task-end separator should render solid rule, got:\n{text}"
    );
    // The frozen seconds stay in the sentinel (and reach the task-stats row),
    // but the rule itself no longer draws them: the number is already on the
    // stats row below and in the bottom bar's turn segment.
    assert!(
        !text.contains("Elapsed"),
        "the rule must not draw an elapsed label, got:\n{text}"
    );
    assert_eq!(
        app.last_prompt_elapsed_secs,
        Some(65),
        "the turn's wall clock must still be frozen for the stats row"
    );
}

#[test]
fn log_thinking_title_shows_scroll_indicator_when_collapsed() {
    let mut app = make_app();
    for i in 1..=6 {
        app.handle_runtime_event(RuntimeEvent::Thinking {
            run_id: None,
            chunk: ThinkingChunk::Delta(format!("reason line {i}\n")),
        });
    }
    app.handle_runtime_event(RuntimeEvent::Text {
        run_id: None,
        role: "assistant".into(),
        content: "final answer".into(),
    });

    let text = render_log_panel_text(&mut app, 100, 24);
    assert!(
        text.contains('↕') || text.contains("Thinking"),
        "collapsed thinking block with >3 lines should show scroll indicator or title, got:\n{text}"
    );
}

#[test]
fn active_thinking_card_renders_a_three_line_tail_without_source_rows() {
    let mut app = make_app();
    app.handle_runtime_event(RuntimeEvent::Thinking {
        run_id: None,
        chunk: ThinkingChunk::Delta("one\ntwo\nthree\nfour\n".into()),
    });

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
        LogItemKind::AssistantMarkdown,
    );
    app.append_msg(
        ratatui::text::Line::from("nested tool line"),
        "nested tool line".into(),
        LogItemKind::SystemTool,
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
fn log_full_width_nested_line_wraps_before_indentation_clip() {
    let mut app = make_app();
    let line = "abcdefghijklmnopqrstuvwxyz";
    app.append_msg(Line::from(line), line.into(), LogItemKind::SystemTool);

    let text = render_log_panel_text(&mut app, 30, 12);

    assert!(
        text.contains("yz"),
        "right edge must not lose characters: {text}"
    );
    assert_eq!(app.log_scroll.visual_cache.len(), 2);
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
    app.handle_runtime_event(RuntimeEvent::Text {
        run_id: None,
        role: "assistant".into(),
        content: "streaming partial".into(),
    });

    let text = render_log_panel_text(&mut app, 80, 16);
    assert!(
        text.contains("streaming partial"),
        "in-progress stream buffer should render in log, got:\n{text}"
    );
    assert!(
        !app.stream_mut().buffer.is_empty(),
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
        text.contains('▐')
            || text.contains('█')
            || text.contains('│')
            || text.contains('▲')
            || text.contains('▼'),
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

/// A stored tool block repaints its title row when the theme changes: the title
/// *text* is a fact of the call, its color is not, so the cell styles it from the
/// frame's theme instead of a line built once at `StepFinished`.
#[test]
fn theme_change_repaints_existing_tool_title_rows() {
    let mut app = make_app();
    let mut themes = Vec::new();

    for i in 0..3 {
        app.handle_runtime_event(RuntimeEvent::StepAdded {
            run_id: None,
            step: PlanStep::new(
                "run",
                "bash",
                format!("theme-probe-{i}"),
                HashMap::from([("command".to_string(), "echo hi".to_string())]),
            ),
        });
        app.handle_runtime_event(
            StepCall::new(i, format!("theme-probe-{i}"), "bash", "echo hi").started(),
        );
        app.handle_runtime_event(
            StepCall::new(i, format!("theme-probe-{i}"), "bash", "echo hi")
                .detail("hi\n")
                .finished(),
        );

        // `toggle_theme` only changes `app.theme`; the blocks above are already
        // built and must still follow it.
        app.toggle_theme();
        let expect_fg = app.theme.fg;
        let terminal = render_log_panel_terminal(&mut app, 100, 20);
        let buf = terminal.backend().buffer();
        let (x, y) = buffer_cell_of(buf, "echo hi").expect("title row is drawn");
        assert_eq!(
            buf[(x, y)].fg,
            expect_fg,
            "the title row must follow the current theme"
        );
        assert!(
            buf[(x, y)].modifier.contains(Modifier::BOLD),
            "the title stays bold"
        );
        themes.push(expect_fg);
    }

    themes.dedup();
    assert!(
        themes.len() > 1,
        "the probe must actually cycle through different fg colors"
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
    seed_tall_subagent_tool(&mut app, 25);
    let _ = render_log_panel_text(&mut app, 100, 14);
    let block = app.tools().blocks.last().expect("tool block");
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

    app.log_scroll.visual_top = app.log_scroll.visual_start_cache[placeholder_logical];
    let mid = render_log_panel_text(&mut app, 100, 14);
    assert!(
        mid.contains("audit the repo"),
        "starting viewport inside placeholder rows should still render full tool card, got:\n{mid}"
    );

    app.log_scroll.visual_top = usize::MAX;
    let bottom = render_log_panel_text(&mut app, 100, 14);
    assert!(
        bottom.contains("Subagent") && bottom.contains("1/25"),
        "bottom scroll should keep tool card metadata visible, got:\n{bottom}"
    );
}

/// A `background_run` card shows the id of the task it started, right after
/// the phase word, for as long as the task runs (the invocation has already
/// returned by then, so this row is where the id is readable).
#[test]
fn running_background_card_shows_the_task_id() {
    let mut app = make_app();
    let mut presentation = ToolPresentationInfo::generic("background_run");
    presentation.keep_live = true;
    app.handle_runtime_event(RuntimeEvent::StepAdded {
        run_id: None,
        step: PlanStep::new(
            "run build in background",
            "background_run",
            "bg1",
            HashMap::from([("command".to_string(), "cargo build".to_string())]),
        ),
    });
    app.handle_runtime_event(
        StepCall::new(0, "bg1", "background_run", "cargo build")
            .presentation(presentation)
            .started(),
    );
    // What `background_run` sends once the task exists.
    app.handle_runtime_event(RuntimeEvent::ToolMeta {
        run_id: None,
        tool_id: "bg1".into(),
        model: None,
        token_usage: None,
        task_id: Some("018f3a2c".into()),
    });

    let text = render_log_panel_text(&mut app, 100, 20);
    assert!(
        text.contains("Background Run"),
        "card title missing:\n{text}"
    );
    assert!(
        text.contains("Running") && text.contains("018f3a2c"),
        "the task id must be on the running row:\n{text}"
    );
    // The id belongs between the phase and the elapsed time, not at the tail
    // where subagent metadata goes.
    let running_at = text.find("Running").expect("phase word");
    let id_at = text.find("018f3a2c").expect("task id");
    assert!(
        id_at > running_at,
        "the id must follow the phase word:\n{text}"
    );
}

#[test]
fn completed_command_renders_header_rows_only() {
    let mut app = make_app();
    app.handle_runtime_event(RuntimeEvent::StepAdded {
        run_id: None,
        step: PlanStep::new(
            "run shell",
            "bash",
            "bash-collapsed",
            HashMap::from([("command".to_string(), "cargo build".to_string())]),
        ),
    });
    app.handle_runtime_event(StepCall::new(0, "bash-collapsed", "bash", "cargo build").started());
    app.handle_runtime_event(
        StepCall::new(0, "bash-collapsed", "bash", "cargo build")
            .detail("Compiling tact\ndone\n")
            .duration_us(100)
            .finished(),
    );

    let block = app.tools().blocks.last().expect("tool block");
    assert_eq!(block.output.visual_rows(false), TOOL_HEADER_ROWS);
    assert!(block.output.layout.detail_collapsed);
    let hint_cols = block
        .output
        .collapsed_action_cols(&app.msgs())
        .expect("hint");

    let text = render_log_panel_text(&mut app, 100, 14);
    assert!(
        text.contains("cargo build"),
        "the command must stay visible in the title, got:\n{text}"
    );
    assert!(
        !text.contains("Command output") && !text.contains("Compiling tact"),
        "finished command output must not be drawn inline, got:\n{text}"
    );
    assert!(
        text.contains("4 lines · [󰜼 Open]"),
        "the meta row must report the hidden output, got:\n{text}"
    );

    // The click target is measured from the block indent, so pin it against the
    // real buffer: the hint's range must cover exactly the `[󰜼 Open]` glyphs one
    // column past the panel border, and no more.
    let terminal = render_log_panel_terminal(&mut app, 100, 14);
    let buf = terminal.backend().buffer();
    let content_x = 1; // left border
    let needle = "[󰜼 Open]";
    let meta_row = (0..buf.area.height)
        .find(|y| {
            let line: String = (0..buf.area.width)
                .map(|x| buf[(x, *y)].symbol().to_string())
                .collect();
            line.contains(needle)
        })
        .unwrap_or_else(|| panic!("row with {needle:?} not found in:\n{text}"));
    // Compare column by column (the row holds `✓`/`·`, so byte offsets would
    // drift): each buffer cell is one column.
    let cells: Vec<String> = (content_x..buf.area.width)
        .map(|x| buf[(x, meta_row)].symbol().to_string())
        .collect();
    let needle_cells: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
    let glyph_start = cells
        .windows(needle_cells.len())
        .position(|w| w == needle_cells.as_slice())
        .expect("the hint is drawn");
    assert_eq!(
        glyph_start, hint_cols.start as usize,
        "the hit range must start on the first glyph of {needle:?}"
    );
    assert_eq!(
        hint_cols.end as usize,
        glyph_start + unicode_width::UnicodeWidthStr::width(needle),
        "the hit range must end on the last glyph of {needle:?}"
    );

    // The button patches its own `bg` onto the row, so pin the row's surface
    // over the columns it covers: no cell may keep another surface's colour.
    let surface_bg = app.theme.bg;
    for x in content_x..hint_cols.end {
        assert_eq!(
            buf[(x, meta_row)].bg,
            surface_bg,
            "column {x} of the meta row must carry the panel background"
        );
    }
}

/// Card chrome (the border title and the bottom hint) is localized when the card
/// is *painted*, not when the block is built: `/lang` has to repaint cards that
/// already exist.
///
/// `App` is constructed in English regardless of the locale, so a tool that
/// finishes and *then* flips the language is the everyday `/lang` case — and it
/// is the case a build-time snapshot gets wrong.
#[test]
fn language_toggle_repaints_tool_card_chrome() {
    let mut app = make_app();
    app.handle_runtime_event(RuntimeEvent::StepAdded {
        run_id: None,
        step: PlanStep::new(
            "run shell",
            "bash",
            "bash-failed",
            HashMap::from([("command".to_string(), "false".to_string())]),
        ),
    });
    app.handle_runtime_event(StepCall::new(0, "bash-failed", "bash", "false").started());
    app.handle_runtime_event(
        StepCall::new(0, "bash-failed", "bash", "false")
            .status(StepStatus::Failed)
            .message("exit 1")
            .detail("boom\n")
            .finished(),
    );

    // The card is built while the UI is still English; only then does the
    // language change.
    app.toggle_language();
    let msgs = app.msgs();
    let text = render_log_panel_text(&mut app, 100, 14);

    // Wide glyphs leave a filler cell behind them, so compare with whitespace
    // squeezed out instead of matching the rendered text literally.
    let squeezed = |s: &str| s.split_whitespace().collect::<String>();
    let drawn = squeezed(&text);
    for (label, localized, built_in) in [
        ("meta row", msgs.tool_phase_failed, "Failed"),
        ("card title", msgs.tool_error_card_title, "Error"),
        (
            "card bottom",
            msgs.tool_error_card_bottom,
            "Click for full error",
        ),
    ] {
        assert!(
            drawn.contains(&squeezed(localized)),
            "{label} must be drawn in the current language ({localized:?}), got:\n{text}"
        );
        assert!(
            !drawn.contains(&squeezed(built_in)),
            "{label} must not keep the language it was built in ({built_in:?}), got:\n{text}"
        );
    }
}

#[test]
fn log_loading_spinner_shows_braille_and_label() {
    let mut app = make_app();
    app.status = Status::Executing {
        current_step: 0,
        total: 1,
    };
    app.append_blank(LogItemKind::SystemTool);
    app.loading_idx = Some(app.log.items.len().saturating_sub(1));
    app.spinner_frame = 3;

    let text = render_log_panel_text(&mut app, 80, 16);
    assert!(
        text.contains('⠸') || text.contains('⠋') || text.contains("Thinking"),
        "loading placeholder should render braille spinner or Thinking label, got:\n{text}"
    );
}

#[test]
fn log_scroll_from_code_card_to_plain_text_restores_theme_background() {
    use crate::widgets::state::CodeBlock;

    let mut app = make_app();
    for i in 0..3 {
        app.add_system_message(format!("code-card-placeholder-{i}"));
    }
    app.add_system_message("plain-after-card".into());
    for i in 0..5 {
        app.add_system_message(format!("scroll-tail-{i}"));
    }
    app.code_blocks.push(CodeBlock {
        block_id: "test-code".into(),
        start_idx: 0,
        end_idx: 3,
        lang: "rust".into(),
        content: "let stale_background = true;".into(),
        styled: vec![Line::from("let stale_background = true;")],
    });

    let surface_bg = app.theme.bg;
    let card_bg = app.theme.code_card_bg();
    assert_ne!(
        card_bg, surface_bg,
        "fixture requires a contrasting overlay"
    );

    let backend = TestBackend::new(80, 6);
    let mut terminal = Terminal::new(backend).expect("test terminal");
    terminal
        .draw(|frame| render_log_panel(frame, frame.area(), &mut app))
        .expect("first code-card frame");

    let plain_logical = app.log_scroll.phys_to_logical_cache[3].expect("plain row logical index");
    app.log_scroll.visual_top = app.log_scroll.visual_start_cache[plain_logical];
    terminal
        .draw(|frame| render_log_panel(frame, frame.area(), &mut app))
        .expect("second plain-text frame");

    let buffer = terminal.backend().buffer();
    let (x, y) = buffer_cell_of(buffer, "plain-after-card").expect("plain row in viewport");
    assert_eq!(buffer[(x, y)].bg, surface_bg);
    assert_ne!(buffer[(x, y)].bg, card_bg);
}

#[test]
fn log_left_border_force_updates_and_stays_theme_border_color() {
    use ratatui::buffer::CellDiffOption;

    use crate::theme::ThemeName;

    let mut app = make_app();
    assert_eq!(app.theme.name, ThemeName::Ink);

    app.handle_runtime_event(RuntimeEvent::Thinking {
        run_id: None,
        chunk: ThinkingChunk::Delta("Checking git status\nline2\nline3".into()),
    });
    app.handle_runtime_event(RuntimeEvent::Thinking {
        run_id: None,
        chunk: ThinkingChunk::Finished,
    });
    seed_tall_subagent_tool(&mut app, 10);

    let terminal = render_log_panel_terminal(&mut app, 100, 30);
    let buf = terminal.backend().buffer();
    let border = app.theme.border;
    let accent = app.theme.accent;

    let mut side_rows = 0usize;
    for y in 1..buf.area.height.saturating_sub(1) {
        let cell = &buf[(0, y)];
        assert_ne!(
            cell.fg,
            accent,
            "left chrome must not keep accent glyphs at y={y}: {:?}",
            cell.symbol()
        );
        if cell.symbol() == "│" {
            side_rows += 1;
            assert_eq!(cell.fg, border, "left border fg at y={y}");
            assert_eq!(
                cell.diff_option,
                CellDiffOption::AlwaysUpdate,
                "left border must force-emit each frame at y={y}"
            );
        }
    }
    assert!(
        side_rows >= 5,
        "expected multiple restamped left-border rows, got {side_rows}"
    );
}

#[test]
fn heading_rows_carry_no_highlight_band() {
    // Regression: the restyle pass used to paint H1 headings with the
    // theme.highlight background (a tui-markdown leftover). On wrapped
    // headings the band covered every wrapped row and read as a shadow
    // block behind the list heading; headings must render backgroundless
    // like the MarkdownCell path.
    let mut app = make_app();
    app.add_system_message(
        "# A very long heading that wraps at this panel width\n\n- item one\n- item two"
            .to_string(),
    );

    let terminal = render_log_panel_terminal(&mut app, 40, 12);
    let buf = terminal.backend().buffer();
    assert!(
        !buffer_has_bg(buf, app.theme.highlight),
        "heading must not paint the highlight band anywhere in the log"
    );
    // The heading text itself still renders.
    let text = super::test_harness::buffer_text(buf);
    assert!(text.contains("A very long heading"), "{text}");
    assert!(
        text.contains("item one") && text.contains("item two"),
        "{text}"
    );
}

#[test]
fn subagent_cancel_button_rect_matches_the_drawn_glyphs() {
    let mut app = make_app();
    app.handle_runtime_event(RuntimeEvent::StepAdded {
        run_id: None,
        step: PlanStep::new(
            "audit the repo",
            "spawn_subagent",
            "sub-live",
            HashMap::from([("prompt".to_string(), "audit the repo".to_string())]),
        ),
    });
    let mut presentation = ToolPresentationInfo::generic("spawn_subagent");
    presentation.keep_live = true;
    app.handle_runtime_event(
        StepCall::new(0, "sub-live", "spawn_subagent", "audit the repo")
            .presentation(presentation.clone())
            .started(),
    );
    // What the async branch sends back while the child keeps running.
    app.handle_runtime_event(
        StepCall::new(0, "sub-live", "spawn_subagent", "audit the repo")
            .message("async_launched { child-123 }")
            .presentation(presentation)
            .finished(),
    );

    let terminal = render_log_panel_terminal(&mut app, 100, 20);
    let buffer = terminal.backend().buffer();
    let (col, row) =
        buffer_cell_of(buffer, "[Cancel]").expect("the live card must draw a [Cancel] button");
    let areas = &app.mouse.subagent_cancel_btn_areas;
    assert_eq!(areas.len(), 1, "exactly one live subagent card");
    let (child_id, rect) = &areas[0];
    assert_eq!(child_id, "child-123");
    assert_eq!(rect.y, row, "hit row must be the drawn row");
    assert_eq!(rect.x, col, "hit area must start at the drawn glyphs");
    assert_eq!(
        rect.width,
        "[Cancel]".chars().count() as u16,
        "hit area must be exactly the glyphs, not a column wider"
    );
    assert!(Button::hit_test(*rect, col, row));
}

/// The live stats row is a *row of the panel*, not a log item: while a task is
/// in flight the last content row carries the line the task-end block will
/// freeze into the log, so the turn's numbers are readable without scrolling to
/// the end of the turn.
#[test]
fn live_stats_row_sits_on_the_last_content_row_while_a_task_runs() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));
    app.status_bar_mut().model_name = "deepseek-flash".into();
    app.status_bar_mut().token_prompt = 60_826;
    app.status_bar_mut().token_completion = 3_420;
    app.status_bar_mut().token_total = 64_246;

    let text = render_log_panel_text(&mut app, 120, 12);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines[10].contains(
            "Task stats:⏱ 01:05 · deepseek-flash · 64246 tokens (prompt 60826 · completion 3420)"
        ),
        "the live line must be drawn on the last content row (10), got:\n{text}"
    );
    assert!(
        lines[11].contains('─'),
        "the box's bottom border stays below the live line, got:\n{text}"
    );
    assert!(
        text.contains("task body"),
        "the log keeps its own rows, got:\n{text}"
    );
}

#[test]
fn live_stats_row_is_absent_when_no_task_is_in_flight() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.status_bar_mut().model_name = "deepseek-flash".into();
    app.status_bar_mut().token_total = 10;

    let text = render_log_panel_text(&mut app, 120, 12);
    assert!(
        !text.contains("Task stats:"),
        "idle must not spend a log row on live stats, got:\n{text}"
    );
}

/// The live row comes out of the viewport, not out of the log: the newest log
/// line must stay on screen above it.
#[test]
fn live_stats_row_takes_its_row_from_the_viewport_not_from_the_log() {
    let mut app = make_app();
    seed_many_numbered_lines(&mut app, 30);
    app.task_start_time = Some(chrono::Local::now());
    app.status_bar_mut().model_name = "mock-model".into();
    app.log_scroll.visual_top = usize::MAX;

    let text = render_log_panel_text(&mut app, 80, 12);
    let lines: Vec<&str> = text.lines().collect();
    assert!(
        lines.iter().any(|line| line.contains("log-row-29")),
        "the newest log row must stay visible above the live row, got:\n{text}"
    );
    assert!(
        lines[10].contains("Task stats:"),
        "the live row is the last content row, got:\n{text}"
    );
}

/// Render invariant (AGENTS.md): the live row paints its own background across
/// the whole row, so a shorter line leaves no residue in the tail.
#[test]
fn live_stats_row_paints_the_theme_bg_across_its_tail() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.task_start_time = Some(chrono::Local::now());
    app.status_bar_mut().model_name = "m".into();

    let terminal = render_log_panel_terminal(&mut app, 60, 12);
    let buffer = terminal.backend().buffer();
    let bg = app.theme.bg;
    for x in 1..59 {
        let cell = &buffer[(x, 10)];
        assert_eq!(cell.bg, bg, "live row cell at x={x} must carry theme.bg");
    }
}

/// The live row is a HUD on the turn boundary, not a log row: it is centered
/// in the panel (the full-width rule above it is the boundary, and the clock
/// that used to sit centered *in* that rule now sits centered under it).
#[test]
fn live_stats_row_is_centered_in_the_panel() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.task_start_time = Some(chrono::Local::now());
    app.status_bar_mut().model_name = "mock-model".into();
    app.status_bar_mut().token_total = 100;

    let terminal = render_log_panel_terminal(&mut app, 120, 12);
    let buffer = terminal.backend().buffer();
    // Measure the padding between the two border columns, cell by cell (a wide
    // glyph's continuation cell is blank, so char counting would be off).
    let blank = |x: u16| buffer[(x, 10)].symbol().trim().is_empty();
    let left = (1..119).take_while(|&x| blank(x)).count();
    let right = (1..119).rev().take_while(|&x| blank(x)).count();

    assert!(left > 0, "a centered row is not flush left");
    assert!(
        left.abs_diff(right) <= 1,
        "the live row must be centered, got left={left} right={right}:\n{}",
        buffer_text(buffer)
    );
}

/// Padding `(left, right)` of the row carrying `Task stats:` in a buffer,
/// measured inside the panel's two border columns.
fn stats_row_padding(buffer: &ratatui::buffer::Buffer) -> (usize, usize) {
    let row = (0..buffer.area.height)
        .find(|&y| {
            (1..buffer.area.width - 1)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
                .contains("Task stats:")
        })
        .expect("a stats row in the buffer");
    let blank = |x: u16| buffer[(x, row)].symbol().trim().is_empty();
    let left = (1..buffer.area.width - 1).take_while(|&x| blank(x)).count();
    let right = (1..buffer.area.width - 1)
        .rev()
        .take_while(|&x| blank(x))
        .count();
    (left, right)
}

/// The frozen row — the task-end stats block written into the log — is centered
/// like the live band it replaces. Its pad is baked into the *line* (never into
/// `raw`, which the `⎘` byte mapping reads), so it re-centers whenever the
/// panel width changes.
#[test]
fn frozen_stats_row_is_centered_like_the_live_row() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.last_prompt_elapsed_secs = Some(65);
    app.status_bar_mut().model_name = "deepseek-flash".into();
    app.status_bar_mut().token_total = 100;
    app.add_task_end_separator();
    app.add_task_stats_block();

    let terminal = render_log_panel_terminal(&mut app, 120, 12);
    let (left, right) = stats_row_padding(terminal.backend().buffer());
    assert!(left > 0, "a centered row is not flush left");
    assert!(
        left.abs_diff(right) <= 1,
        "the frozen row must be centered, got left={left} right={right}:\n{}",
        buffer_text(terminal.backend().buffer())
    );
}

/// The handoff: the frozen row written at task end says exactly what the live
/// row was saying, because both come from `stats_line::task_stats_body`.
#[test]
fn the_live_row_hands_off_to_the_frozen_row() {
    let mut app = make_app();
    app.add_system_message("task body".into());
    app.task_start_time = Some(chrono::Local::now() - chrono::Duration::seconds(65));
    app.status_bar_mut().model_name = "deepseek-flash".into();
    app.status_bar_mut().token_total = 100;

    let live = render_log_panel_text(&mut app, 120, 12);
    assert!(
        live.lines()
            .nth(10)
            .is_some_and(|l| l.contains("Task stats:⏱ 01:05 · deepseek-flash · 100 tokens")),
        "got:\n{live}"
    );

    app.last_prompt_elapsed_secs = Some(65);
    app.task_start_time = None;
    app.add_task_stats_block();

    let frozen = render_log_panel_text(&mut app, 120, 12);
    assert!(
        frozen.contains("Task stats:⏱ 01:05 · deepseek-flash · 100 tokens"),
        "the frozen row must repeat the live row's numbers, got:\n{frozen}"
    );
    assert!(
        !frozen
            .lines()
            .nth(10)
            .is_some_and(|l| l.contains("Task stats:")),
        "the live row must be gone once the task ends, got:\n{frozen}"
    );
}
