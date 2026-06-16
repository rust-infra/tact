use crate::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
};

use super::popup_common::{compute_popup_layout, render_popup_scrollbar};

pub(crate) fn render_thinking_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let popup = match &app.thinking.popup {
        Some(p) => p,
        None => return,
    };
    let block = &app.thinking.blocks[popup.block_idx];
    let raw_total = block.end_idx.saturating_sub(block.title_idx);
    if raw_total == 0 {
        return;
    }

    let styled_lines = &block.cached_markdown;
    let total = styled_lines.len();
    if total == 0 {
        return;
    }

    let layout = compute_popup_layout(area, total, popup.scroll, 3);

    frame.render_widget(Clear, layout.area);

    let mut text = Text::default();
    let title_style = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);
    text.push_line(Line::from(Span::styled(
        format!(
            "{} ({} markdown lines, {} raw)",
            popup.title, total, raw_total
        ),
        title_style,
    )));
    text.push_line(Line::from(""));
    for line in &styled_lines[layout.start_line..layout.end_line] {
        text.push_line(line.clone());
    }

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.msgs().thinking_popup_title)
                .title_bottom(Line::from(vec![
                    Span::styled(
                        app.msgs().popup_copy_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                    Span::styled(
                        app.msgs().popup_close_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                    Span::styled(
                        app.msgs().popup_scroll_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                ]))
                .style(Style::default().fg(app.theme.fg).bg(app.theme.bg)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, layout.area);

    render_popup_scrollbar(frame, &layout, total);

    app.mouse.thinking_popup_area = layout.area;
}
