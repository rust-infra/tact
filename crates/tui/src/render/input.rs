//! Input box / command line — app-layer wrappers.
//!
//! The rendering moved verbatim into `agent_tui_kit::render::input` (pure
//! `&RenderCtx`). This module keeps the `&mut App` prepare phase (caret-scroll
//! adjustment, `[Cancel]` hit area, voice button hit area) plus the
//! App-integration tests that render through the wrapper.

use ratatui::{Frame, layout::Rect};

use agent_tui_kit::render::input as kit_input;

use super::slash_style::skill_name_set;
use crate::widgets::state::{App, InputMode, VoicePhase};

/// Render the input box (insert mode) or command line (palette mode).
pub(crate) fn render_input_box(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.input_mode == InputMode::Palette {
        render_command_line(frame, area, app);
        return;
    }

    // Codex-style queued messages: the hint + "↳ message" rows sit above the
    // bordered input box, which shrinks to the remaining height.
    let pending_lines = if app.pending_messages.is_empty() {
        0
    } else {
        (1 + app.pending_messages.len()).min(4) as u16
    };
    let input_area = if pending_lines == 0 {
        area
    } else {
        Rect::new(
            area.x,
            area.y + pending_lines,
            area.width,
            area.height - pending_lines,
        )
    };

    // Prepare: keep the caret visible (mutable — the kit render pass is pure).
    let inner_width = input_area.width.saturating_sub(2).max(1) as usize;
    let (cursor_display_line, _) =
        kit_input::caret_display_line(&app.input, app.input_cursor, inner_width);
    let visible_lines = input_area.height.saturating_sub(2) as usize;
    if cursor_display_line < app.input_scroll as usize {
        app.input_scroll = cursor_display_line as u16;
    } else if cursor_display_line >= app.input_scroll as usize + visible_lines {
        app.input_scroll = (cursor_display_line - visible_lines + 1) as u16;
    }

    // Prepare: voice button hit area (app-layer extension).
    update_voice_button_area(app, input_area);

    let ctx = app.render_ctx();
    let skill_names = skill_name_set(&app.skills_data);
    let cancel_area = kit_input::render_input_box(frame, area, &ctx, &skill_names);
    app.set_cancel_button_area(cancel_area);
}

/// Render command-line input (Palette mode).
pub(crate) fn render_command_line(frame: &mut Frame, area: Rect, app: &App) {
    let ctx = app.render_ctx();
    kit_input::render_command_line(frame, area, &ctx);
}

fn update_voice_button_area(app: &mut App, area: Rect) {
    if matches!(app.voice.phase, VoicePhase::Disabled) {
        app.voice.set_button_area(Rect::default());
        return;
    }
    let label = app.voice_button_label();
    let width = unicode_width::UnicodeWidthStr::width(label.as_str()) as u16;
    if width == 0 {
        app.voice.set_button_area(Rect::default());
        return;
    }
    // Centered Block title: starts at left_border + (inner_width - label_w) / 2.
    let inner_width = area.width.saturating_sub(2);
    let x = area
        .x
        .saturating_add(1)
        .saturating_add(inner_width.saturating_sub(width) / 2);
    app.voice.set_button_area(Rect::new(x, area.y, width, 1));
}

#[cfg(test)]
mod render_tests {
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

    use super::{
        super::test_harness::{buffer_text, make_app},
        render_input_box,
    };
    use crate::widgets::state::{SkillEntry, VoicePhase, VoiceState};
    use agent_tui_kit::render::input::{caret_in_wrapped, truncate_to_width, wrap_line};

    #[test]
    fn input_box_renders_multiline_content() {
        let mut app = make_app();
        app.input = "line one\nline two".into();
        app.input_cursor = app.input.len();

        let backend = TestBackend::new(80, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 80, 5), &mut app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("line one"), "multiline input visible: {text}");
    }

    #[test]
    fn wrap_line_splits_at_column_width() {
        assert_eq!(wrap_line("", 4), vec![""]);
        assert_eq!(wrap_line("abcd", 2), vec!["ab", "cd"]);
        assert_eq!(wrap_line("abcdef", 2), vec!["ab", "cd", "ef"]);
        // CJK wide characters count as two columns.
        assert_eq!(wrap_line("中文abc", 4), vec!["中文", "abc"]);
        assert_eq!(wrap_line("中文", 3), vec!["中", "文"]);
        // Oversized single character stays on its own row.
        assert_eq!(wrap_line("ab中", 3), vec!["ab", "中"]);
    }

    #[test]
    fn caret_in_wrapped_maps_logical_column_to_display_row() {
        assert_eq!(caret_in_wrapped("abcdef", 2, 0), (0, 0));
        assert_eq!(caret_in_wrapped("abcdef", 2, 2), (0, 2));
        assert_eq!(caret_in_wrapped("abcdef", 2, 4), (1, 2));
        assert_eq!(caret_in_wrapped("abcdef", 2, 6), (2, 2));
        // CJK caret: "中文ab" wraps as ["中文", "ab"]; caret after "中文" (col 4).
        assert_eq!(caret_in_wrapped("中文ab", 4, 4), (0, 4));
        assert_eq!(caret_in_wrapped("中文ab", 4, 6), (1, 2));
    }

    #[test]
    fn input_box_soft_wraps_overlong_line() {
        let mut app = make_app();
        app.input = "x".repeat(100);
        app.input_cursor = app.input.len();

        // Inner width = 38; 100 chars wrap into 38+38+24 = 3 display rows.
        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 40, 5), &mut app))
            .expect("draw");

        let buf = terminal.backend().buffer();
        // Content rows sit between the left/right border cells (row 0/39),
        // so count the 'x' glyphs rather than trimming.
        let row1: String = (0..40).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        let row2: String = (0..40).map(|x| buf[(x, 2)].symbol().to_string()).collect();
        let row3: String = (0..40).map(|x| buf[(x, 3)].symbol().to_string()).collect();
        assert_eq!(row1.matches('x').count(), 38, "first wrap row full");
        assert_eq!(row2.matches('x').count(), 38, "second wrap row full");
        assert_eq!(row3.matches('x').count(), 24, "tail wrap row");
    }

    #[test]
    fn input_box_scrolls_to_caret_on_wrapped_line() {
        let mut app = make_app();
        app.input = "y".repeat(200); // inner width 38 → 6 display rows
        app.input_cursor = app.input.len();

        // Height 3 → only 1 visible content row; caret on the last row forces
        // scroll to the bottom.
        let backend = TestBackend::new(40, 3);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 40, 3), &mut app))
            .expect("draw");

        assert_eq!(app.input_scroll, 5, "scrolled so caret row is visible");
        let buf = terminal.backend().buffer();
        let row: String = (0..40).map(|x| buf[(x, 1)].symbol().to_string()).collect();
        assert_eq!(row.matches('y').count(), 10, "tail row 200-5*38=10");
    }

    #[test]
    fn input_box_renders_skill_and_args() {
        let mut app = make_app();
        app.skills_data = vec![SkillEntry {
            name: "demo-test".into(),
            description: "d".into(),
            body: "body".into(),
        }];
        app.input = "/demo-test hi".into();
        app.input_cursor = app.input.len();

        let backend = TestBackend::new(80, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 80, 5), &mut app))
            .expect("draw");

        let buf = terminal.backend().buffer();
        let text = buffer_text(buf);
        assert!(text.contains("/demo-test"), "skill visible: {text}");
        assert!(text.contains("hi"), "args visible: {text}");

        // Find cells for skill vs arg and assert different fg.
        let mut skill_fg = None;
        let mut arg_fg = None;
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                let cell = &buf[(x, y)];
                if cell.symbol() == "/" && skill_fg.is_none() {
                    // start of /demo-test
                    skill_fg = cell.style().fg;
                }
            }
        }
        // Scan the content row for 'h' of "hi" after skill
        'outer: for y in 0..buf.area.height {
            let mut row = String::new();
            for x in 0..buf.area.width {
                row.push_str(buf[(x, y)].symbol());
            }
            if let Some(pos) = row.find("/demo-test hi") {
                let skill_x = pos as u16;
                let arg_x = (pos + "/demo-test ".len()) as u16;
                skill_fg = buf[(skill_x, y)].style().fg;
                arg_fg = buf[(arg_x, y)].style().fg;
                break 'outer;
            }
        }
        assert!(skill_fg.is_some() && arg_fg.is_some());
        assert_ne!(
            skill_fg, arg_fg,
            "skill and args should use different fg colors"
        );
    }

    #[test]
    fn input_box_renders_recording_elapsed_time() {
        let mut app = make_app();
        app.voice.phase = VoicePhase::Recording {
            started_at: std::time::Instant::now() - std::time::Duration::from_secs(8),
        };

        let backend = TestBackend::new(100, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 100, 5), &mut app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains("00:08"), "recording elapsed label: {text}");
    }

    #[test]
    fn truncate_to_width_appends_ellipsis_for_cjk() {
        assert_eq!(truncate_to_width("abc", 5), "abc");
        assert_eq!(truncate_to_width("abcdef", 4), "abc…");
        // CJK wide chars count as two columns; truncation targets `max` cols.
        assert_eq!(truncate_to_width("中文abc", 5), "中文…");
        assert_eq!(truncate_to_width("中文abc", 6), "中文a…");
        assert_eq!(truncate_to_width("中文abc", 7), "中文abc");
    }

    #[test]
    fn input_box_renders_pending_block_above_input() {
        let mut app = make_app();
        app.status = crate::widgets::state::Status::Executing {
            current_step: 0,
            total: 1,
        };
        app.queue_pending_message("fix the auth bug".into(), "fix the auth bug".into());

        // Height = hint row + 1 message row + input box (2 border + 1 content).
        let backend = TestBackend::new(80, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 80, 5), &mut app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("Message will be submitted after the current task"),
            "pending hint visible: {text}"
        );
        let hint_line = text
            .lines()
            .find(|line| line.contains(app.msgs().pending_submit_hint))
            .expect("pending hint must be rendered on one line");
        let expected_hint_with_cancel = format!("{} [Cancel]", app.msgs().pending_submit_hint);
        assert!(
            hint_line.contains(&expected_hint_with_cancel),
            "Cancel must immediately follow the hint text: {hint_line:?}"
        );
        assert!(
            text.contains("↳ fix the auth bug"),
            "pending message row visible: {text}"
        );
        assert!(
            text.contains("[Cancel]"),
            "cancel button visible on wide enough terminal: {text}"
        );
        assert!(
            !app.pending_cancel_btn_area.is_empty(),
            "render must record the cancel button hit area"
        );
        assert!(text.contains("Type a task"), "input box still rendered");
        // Pending block rows must carry the input-box background (no shadow).
        let buf = terminal.backend().buffer();
        for x in 0..buf.area.width {
            for y in 0..2 {
                assert_eq!(
                    buf[(x, y)].style().bg,
                    Some(app.theme.input_box_bg),
                    "pending row {y} col {x} must paint its own background"
                );
            }
        }
    }

    #[test]
    fn input_box_pending_button_hidden_on_narrow_terminal() {
        let mut app = make_app();
        app.status = crate::widgets::state::Status::Planning;
        app.queue_pending_message("narrow".into(), "narrow".into());

        // Too narrow for the hint + button: button must be hidden and the hit
        // area cleared.
        let backend = TestBackend::new(30, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 30, 5), &mut app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            !text.contains("[Cancel]"),
            "button must be hidden on narrow terminals: {text}"
        );
        assert!(
            app.pending_cancel_btn_area.is_empty(),
            "cancel hit area must be cleared when the button is hidden"
        );
    }

    #[test]
    fn input_box_pending_block_truncates_long_messages() {
        let mut app = make_app();
        app.status = crate::widgets::state::Status::Planning;
        app.queue_pending_message("x".repeat(200), "x".repeat(200));

        let backend = TestBackend::new(40, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 40, 5), &mut app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(text.contains('…'), "overlong pending message ellipsized");
        assert!(
            !text.contains("xxxxx".repeat(20).as_str()),
            "pending message must not overflow the width"
        );
    }
    #[test]
    fn input_box_keeps_top_border_between_title_and_voice() {
        let mut app = make_app();
        app.voice = VoiceState::idle_visible_for_tests();

        // Wide enough that the centered Voice title does not collide with the left title.
        let backend = TestBackend::new(120, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 120, 5), &mut app))
            .expect("draw");

        let buf = terminal.backend().buffer();
        let y = 0u16;
        let mut row = String::new();
        for x in 0..buf.area.width {
            row.push_str(buf[(x, y)].symbol());
        }
        // Border glyphs must remain between left title and Voice. Space-padded
        // titles with a background used to overwrite this segment.
        let title = app.msgs().input_box_title;
        let voice = if row.contains("Voice") {
            "Voice"
        } else {
            "语音"
        };
        let after_title = row.find(title.trim()).expect("input title") + title.trim().len();
        let voice_at = row.find(voice).expect("voice label");
        assert!(
            voice_at > after_title,
            "voice should sit after title:\n{row}"
        );
        let between = &row[after_title..voice_at];
        assert!(
            between.chars().any(|c| c == '─' || c == '━' || c == '-'),
            "top border between title and voice must not be eaten:\n{row}"
        );
    }

    #[test]
    fn input_box_renders_voice_button_when_enabled() {
        let mut app = make_app();
        app.voice = VoiceState::idle_visible_for_tests();

        let backend = TestBackend::new(80, 5);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| render_input_box(frame, Rect::new(0, 0, 80, 5), &mut app))
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("Voice") || text.contains("语音"),
            "voice label: {text}"
        );
        assert!(!app.voice.button_area.is_empty());
        assert!(matches!(app.voice.phase, VoicePhase::Idle));
    }
}
