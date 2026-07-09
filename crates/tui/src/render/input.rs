use crate::widgets::state::{App, InputMode};
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

/// Render command-line input (Search / Palette mode).
pub(crate) fn render_command_line(frame: &mut Frame, area: Rect, app: &App) {
    let prefix = match app.input_mode {
        InputMode::Search => "/",
        _ => "",
    };
    let content = format!("{}{}", prefix, app.cmd_line);
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
    if app.input_mode == InputMode::Search || app.input_mode == InputMode::Palette {
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
    let display_text = if app.input.is_empty() {
        app.msgs().input_box_placeholder.to_string()
    } else {
        lines[start..end].join("\n")
    };
    let placeholder_mode = app.input.is_empty();

    // Determine border color: accent when focused (insert mode), normal otherwise
    let border_color = if app.input_mode == InputMode::Insert {
        app.theme.accent
    } else {
        app.theme.border
    };

    let input_para = Paragraph::new(display_text)
        .style(
            Style::default()
                .fg(if placeholder_mode {
                    Color::Rgb(100, 100, 120) // dim for placeholder
                } else {
                    app.theme.input_box_fg
                })
                .bg(app.theme.input_box_bg),
        )
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
}
