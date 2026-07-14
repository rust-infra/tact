use super::slash_style::{skill_name_set, style_input_skill_line};
use crate::widgets::state::{App, InputMode};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

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
            cursor_col += unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        }
    }

    let visible_lines = area.height.saturating_sub(2) as usize;

    if cursor_line < app.input_scroll as usize {
        app.input_scroll = cursor_line as u16;
    } else if cursor_line >= app.input_scroll as usize + visible_lines {
        app.input_scroll = (cursor_line - visible_lines + 1) as u16;
    }

    let lines: Vec<&str> = app.input.split('\n').collect();
    let start = app.input_scroll as usize;
    let end = (start + visible_lines).min(lines.len());
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
        let styled_lines: Vec<Line<'static>> = lines[start..end]
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

    // Determine border color: accent when focused (insert mode), normal otherwise
    let border_color = if app.input_mode == InputMode::Insert {
        app.theme.accent
    } else {
        app.theme.border
    };

    let input_para = Paragraph::new(display)
        .style(Style::default().bg(app.theme.input_box_bg))
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(app.theme.block_border_type())
                .border_style(Style::default().fg(border_color))
                .title(app.msgs().input_box_title)
                .title_bottom(if !app.input.is_empty() {
                    let total_lines = lines.len();
                    let total_chars = app.input.chars().count();
                    format!(" 📝 {}L · {}chars ", total_lines, total_chars)
                } else {
                    String::new()
                }),
        );
    frame.render_widget(Clear, area);
    frame.render_widget(input_para, area);

    let cursor_x = area.x + 1 + cursor_col as u16;
    let cursor_y = area.y + 1 + (cursor_line - app.input_scroll as usize) as u16;
    frame.set_cursor_position((cursor_x, cursor_y));
}

#[cfg(test)]
mod render_tests {
    use super::super::test_harness::{buffer_text, make_app};
    use super::render_input_box;
    use crate::widgets::state::SkillEntry;
    use ratatui::{Terminal, backend::TestBackend, layout::Rect};

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
}
