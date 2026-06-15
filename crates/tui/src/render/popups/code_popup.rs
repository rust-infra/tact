use ratatui::{
    Frame, layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap},
};
use crate::state::App;

pub(crate) fn render_code_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match &app.code_popup {
        Some(p) => p,
        None => return,
    };
    let block = &app.code_blocks[popup.block_idx];
    let lines: Vec<&str> = block.content.lines().collect();
    let total = lines.len();
    if total == 0 {
        return;
    }

    let popup_width = (area.width as f32 * 0.8) as u16;
    let popup_height = (area.height as f32 * 0.8) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);

    let content_height = popup_height.saturating_sub(3) as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);
    let start_line = scroll;
    let end_line = (scroll + content_height).min(total);

    let mut text = Text::default();
    let title_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    let lang = if popup.lang.is_empty() { "code" } else { &popup.lang };
    text.push_line(Line::from(Span::styled(
        format!("```{} ({} lines)", lang, total),
        title_style,
    )));
    text.push_line(Line::from(""));

    // Render code lines, truncating to popup width minus borders/padding
    let max_chars = popup_width.saturating_sub(4) as usize;
    for &line in &lines[start_line..end_line] {
        let display: String = line.chars().take(max_chars).collect();
        text.push_line(Line::from(Span::styled(
            display,
            Style::default().fg(app.theme.fg),
        )));
    }

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", lang))
                .title_bottom(Line::from(vec![
                    Span::styled(" y:copy ", Style::default().fg(app.theme.accent)),
                    Span::styled(" j/k:scroll ", Style::default().fg(app.theme.accent)),
                    Span::styled(" Esc:close ", Style::default().fg(app.theme.accent)),
                ]))
                .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, popup_area);

    let scrollbar = Scrollbar::default()
        .orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    app.mouse.code_popup_area = popup_area;
}
