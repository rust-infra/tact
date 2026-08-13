use ratatui::{
    Frame,
    layout::{Alignment, Rect},
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::slash_style::{skill_name_set, style_input_skill_line};
use crate::widgets::state::{App, InputMode, VoicePhase};

/// Soft-wrap a logical line into display-line slices no wider than
/// `max_width` columns, splitting at character boundaries.
///
/// The renderer draws exactly these rows (Paragraph stays unwrapped), so the
/// caret computed from the same function lands where the text is visible.
pub(crate) fn wrap_line(line: &str, max_width: usize) -> Vec<&str> {
    let mut rows = Vec::new();
    if max_width == 0 {
        rows.push(line);
        return rows;
    }
    let mut start = 0;
    let mut width = 0;
    for (byte_idx, ch) in line.char_indices() {
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + w > max_width && width > 0 {
            rows.push(&line[start..byte_idx]);
            start = byte_idx;
            width = 0;
        }
        width += w;
    }
    rows.push(&line[start..]);
    rows
}

/// Display (row, column) of a caret sitting at logical `cursor_col` within
/// `line`, after soft-wrapping at `max_width`. The caret is placed at the end
/// of the prefix's last display row.
fn caret_in_wrapped(line: &str, max_width: usize, cursor_col: usize) -> (usize, usize) {
    let mut acc = 0;
    let mut prefix_len = 0;
    for (byte_idx, ch) in line.char_indices() {
        if acc >= cursor_col {
            break;
        }
        let w = UnicodeWidthChar::width(ch).unwrap_or(0);
        if acc + w > cursor_col {
            break;
        }
        acc += w;
        prefix_len = byte_idx + ch.len_utf8();
    }
    let wrapped = wrap_line(&line[..prefix_len], max_width);
    let row = wrapped.len().saturating_sub(1);
    let col = UnicodeWidthStr::width(wrapped.last().copied().unwrap_or(""));
    (row, col)
}

/// Render command-line input (Palette mode).
pub(crate) fn render_command_line(frame: &mut Frame, area: Rect, app: &App) {
    let content = app.cmd_line.clone();
    let input_para = Paragraph::new(content)
        .style(
            Style::default()
                .fg(app.theme.input_box_fg)
                .bg(app.theme.input_box_bg),
        )
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.msgs().command_title),
        );
    frame.render_widget(input_para, area);
    let cmd_width = UnicodeWidthStr::width(app.cmd_line.as_str()) as u16;
    let cursor_pos = (area.x + 2 + cmd_width).min(area.x + area.width - 2);
    frame.set_cursor_position((cursor_pos, area.y + 1));
}

/// Render the main input box (Insert mode), or delegate to command-line rendering.
pub(crate) fn render_input_box(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.input_mode == InputMode::Palette {
        render_command_line(frame, area, app);
        return;
    }

    // Logical rows are separated by explicit `\n`; the caret line/column is
    // computed on the raw input, then mapped through soft-wrapping below.
    let mut cursor_line = 0;
    let mut cursor_col = 0;
    for (i, c) in app.input.char_indices() {
        if i >= app.input_cursor {
            break;
        }
        if c == '\n' {
            cursor_line += 1;
            cursor_col = 0;
        } else {
            cursor_col += UnicodeWidthChar::width(c).unwrap_or(0);
        }
    }

    let inner_width = area.width.saturating_sub(2).max(1) as usize;
    let lines: Vec<&str> = app.input.split('\n').collect();

    // Caret position after soft-wrapping: rows before the caret's logical
    // line plus the caret's row within that line.
    let cursor_logical = lines.get(cursor_line).copied().unwrap_or("");
    let (caret_row_in_line, caret_col_in_line) =
        caret_in_wrapped(cursor_logical, inner_width, cursor_col);
    let prior_display_rows: usize = lines[..cursor_line]
        .iter()
        .map(|line| wrap_line(line, inner_width).len())
        .sum();
    let cursor_display_line = prior_display_rows + caret_row_in_line;

    // Every display row the input occupies after soft-wrapping.
    let display_lines: Vec<&str> = lines
        .iter()
        .flat_map(|line| wrap_line(line, inner_width))
        .collect();

    let visible_lines = area.height.saturating_sub(2) as usize;

    if cursor_display_line < app.input_scroll as usize {
        app.input_scroll = cursor_display_line as u16;
    } else if cursor_display_line >= app.input_scroll as usize + visible_lines {
        app.input_scroll = (cursor_display_line - visible_lines + 1) as u16;
    }

    let line_stats = if app.input.is_empty() {
        None
    } else {
        Some((display_lines.len(), app.input.chars().count()))
    };

    let start = app.input_scroll as usize;
    let end = (start + visible_lines).min(display_lines.len());
    let placeholder_mode = app.input.is_empty();

    let display: Text<'static> = if placeholder_mode {
        Text::from(Span::styled(
            app.msgs().input_box_placeholder.to_string(),
            Style::default()
                .fg(Color::Rgb(100, 100, 120))
                .bg(app.theme.input_box_bg),
        ))
    } else {
        let skill_names = skill_name_set(&app.skills_data);
        let styled_lines: Vec<Line<'static>> = display_lines[start..end]
            .iter()
            .map(|line| {
                style_input_skill_line(line, &skill_names, &app.theme).unwrap_or_else(|| {
                    Line::from(Span::styled(
                        (*line).to_string(),
                        Style::default()
                            .fg(app.theme.input_box_fg)
                            .bg(app.theme.input_box_bg),
                    ))
                })
            })
            .collect();
        Text::from(styled_lines)
    };

    let bottom_title = line_stats
        .map(|(total_lines, total_chars)| format!(" 📝 {total_lines}L · {total_chars}chars "))
        .unwrap_or_default();

    // Determine border color: accent when focused (insert mode), normal otherwise
    let border_color = if app.input_mode == InputMode::Insert {
        app.theme.accent
    } else {
        app.theme.border
    };

    // Left title + centered voice as separate Block titles so the top border
    // stays visible between them (space-padding with a bg would eat the line).
    let mut block = Block::default()
        .borders(Borders::ALL)
        .border_type(app.theme.block_border_type())
        .border_style(Style::default().fg(border_color))
        .title(Span::raw(app.msgs().input_box_title))
        .title_bottom(bottom_title);
    if let Some((voice_label, voice_style)) = voice_title(app) {
        block = block
            .title(Line::from(Span::styled(voice_label, voice_style)).alignment(Alignment::Center));
    }

    let input_para = Paragraph::new(display)
        .style(Style::default().bg(app.theme.input_box_bg))
        .block(block);
    frame.render_widget(Clear, area);
    frame.render_widget(input_para, area);
    update_voice_button_area(app, area);

    let cursor_x =
        (area.x + 1 + caret_col_in_line as u16).min(area.x + area.width.saturating_sub(2));
    let cursor_y = area.y + 1 + (cursor_display_line - app.input_scroll as usize) as u16;
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn voice_title(app: &App) -> Option<(String, Style)> {
    if matches!(app.voice.phase, VoicePhase::Disabled) {
        return None;
    }
    let label = voice_button_label(app);
    if label.is_empty() {
        return None;
    }
    let style = match app.voice.phase {
        VoicePhase::Idle => Style::default().fg(app.theme.accent),
        VoicePhase::Recording { .. } => Style::default().fg(app.theme.warning),
        VoicePhase::Transcribing => Style::default().fg(Color::Rgb(120, 120, 140)),
        VoicePhase::Disabled => Style::default(),
    };
    Some((label, style))
}

fn voice_button_label(app: &App) -> String {
    match app.voice.phase {
        VoicePhase::Disabled => String::new(),
        VoicePhase::Idle => app.msgs().voice_idle.to_string(),
        VoicePhase::Recording { started_at } => {
            let elapsed = started_at.elapsed();
            format!(
                "⏹ {:02}:{:02}",
                elapsed.as_secs() / 60,
                elapsed.as_secs() % 60
            )
        }
        VoicePhase::Transcribing => app.msgs().voice_transcribing.to_string(),
    }
}

fn update_voice_button_area(app: &mut App, area: Rect) {
    if matches!(app.voice.phase, VoicePhase::Disabled) {
        app.voice.set_button_area(Rect::default());
        return;
    }
    let label = voice_button_label(app);
    let width = UnicodeWidthStr::width(label.as_str()) as u16;
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
        caret_in_wrapped, render_input_box, wrap_line,
    };
    use crate::widgets::state::{SkillEntry, VoicePhase, VoiceState};

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
