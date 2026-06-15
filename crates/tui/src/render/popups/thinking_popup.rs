use ratatui::{Frame, layout::Rect, style::{Color, Modifier, Style}, text::{Line, Span, Text}, widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap}};
use crate::state::App;

pub(crate) fn render_thinking_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match &app.thinking.popup {
        Some(p) => p,
        None => return,
    };
    let block = &app.thinking.blocks[popup.block_idx];
    let raw_total = block.end_idx.saturating_sub(block.title_idx);
    if raw_total == 0 { return; }

    let styled_lines = &block.cached_markdown;
    let total = styled_lines.len();
    if total == 0 { return; }

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
    let title_style = Style::default().fg(app.theme.accent).add_modifier(Modifier::BOLD);
    text.push_line(Line::from(Span::styled(
        format!("{} ({} markdown lines, {} raw)", popup.title, total, raw_total),
        title_style,
    )));
    text.push_line(Line::from(""));
    for line in &styled_lines[start_line..end_line] {
        text.push_line(line.clone());
    }

    let para = Paragraph::new(text)
        .block(Block::default()
            .borders(Borders::ALL)
            .title(app.msgs().thinking_popup_title)
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

    app.mouse.thinking_popup_area = popup_area;
}

