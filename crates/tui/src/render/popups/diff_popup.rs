use ratatui::{Frame, layout::Rect, style::Style, text::{Line, Span, Text}, widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap}};
use crate::state::App;

pub(crate) fn render_diff_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let file_path = app.diff_popup.as_ref().map(|p| p.file_path.clone());
    let file_path = match file_path {
        Some(p) => p,
        None => return,
    };

    let popup = app.diff_popup.as_mut().unwrap();

    if popup.cached_content.is_none() {
        popup.cached_content = std::fs::read_to_string(&file_path).ok();
    }
    let content = match &popup.cached_content {
        Some(c) => c,
        None => {
            let err = format!("Unable to read file: {}", file_path);
            let para = Paragraph::new(err)
                .block(Block::default()
                    .borders(Borders::ALL)
                    .title(app.msgs().diff_popup_title.replace("{}", &file_path)));
            frame.render_widget(para, area);
            return;
        }
    };

    let lines: Vec<&str> = content.lines().collect();
    let total = lines.len();
    if total == 0 { return; }

    let popup_width = (area.width as f32 * 0.8).max(40.0) as u16;
    let popup_height = (area.height as f32 * 0.8).max(10.0) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let content_height = popup_height.saturating_sub(3) as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);
    let start_line = scroll;
    let end_line = (start_line + content_height).min(total);

    let num_width = (total + 1).to_string().len().max(3);
    let code_width = (popup_width as usize).saturating_sub(4 + num_width);
    let num_style = Style::default().fg(app.theme.border);
    let text_style = Style::default().fg(app.theme.fg);

    let mut text = Text::default();
    for i in start_line..end_line {
        let num = format!("{:>nw$}", i + 1, nw = num_width);
        let trimmed: String = lines[i].chars().take(code_width).collect();
        text.push_line(Line::from(vec![
            Span::styled(format!(" {} ", num), num_style),
            Span::styled(trimmed, text_style),
        ]));
    }

    let para = Paragraph::new(text)
        .block(Block::default()
            .borders(Borders::ALL)
            .title(app.msgs().diff_popup_title.replace("{}", &file_path))
            .title_bottom(Line::from(vec![
                Span::styled(app.msgs().popup_copy_hint, Style::default().fg(app.theme.accent)),
                Span::styled(app.msgs().popup_close_hint, Style::default().fg(app.theme.accent)),
                Span::styled(app.msgs().popup_scroll_hint, Style::default().fg(app.theme.accent)),
            ]))
            .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)))
        .wrap(Wrap { trim: false });

    frame.render_widget(para, popup_area);

    let scrollbar = Scrollbar::default()
        .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total).viewport_content_length(content_height).position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    app.mouse.diff_popup_area = popup_area;
}
