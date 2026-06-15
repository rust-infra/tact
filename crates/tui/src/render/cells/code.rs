use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use crate::state::App;

/// Render code block card overlay.
pub(crate) fn render_code_cards(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    visual_scroll: usize,
    visible_height: usize,
) {
    let vs_cache = &app.log_scroll.visual_start_cache;
    for block in &app.code_blocks {
        let Some(start_logical) = app.phys_to_logical_fast(block.start_idx) else {
            continue;
        };
        let Some(end_logical) = app.phys_to_logical_fast(block.end_idx.saturating_sub(1)) else {
            continue;
        };
        if start_logical >= vs_cache.len() || end_logical + 1 >= vs_cache.len() {
            continue;
        }
        let vis_top = vs_cache[start_logical];
        let vis_bot = vs_cache[end_logical + 1];
        let vis_range_end = visual_scroll + visible_height;
        if vis_bot <= visual_scroll || vis_top >= vis_range_end {
            continue;
        }
        let y_top = (vis_top.saturating_sub(visual_scroll)) as u16;
        let y_bot =
            (vis_bot.saturating_sub(visual_scroll)).min(visible_height) as u16;
        if y_bot <= y_top {
            continue;
        }

        let lang_label = if block.lang.is_empty() {
            "code".to_string()
        } else {
            block.lang.clone()
        };
        let total_styled = block.styled.len();
        let inner_h = (y_bot.saturating_sub(y_top).saturating_sub(2)) as usize;
        let shown = total_styled.min(inner_h);
        let msgs = app.msgs();
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(100, 120, 180)))
            .style(Style::default().bg(Color::Rgb(20, 24, 38)))
            .title(Span::styled(
                format!(" {} ", lang_label),
                Style::default()
                    .fg(Color::Rgb(160, 180, 240))
                    .add_modifier(Modifier::BOLD),
            ))
            .title_bottom(if total_styled > shown {
                Line::from(Span::styled(
                    format!(" +{} lines | {}", total_styled - shown, msgs.code_card_bottom),
                    Style::default().fg(Color::DarkGray),
                ))
            } else {
                Line::from(Span::styled(
                    msgs.code_card_bottom,
                    Style::default().fg(Color::DarkGray),
                ))
            });

        let card_area = Rect::new(
            area.x + 1,
            area.y + 1 + y_top,
            area.width.saturating_sub(2),
            y_bot - y_top,
        );
        frame.render_widget(Clear, card_area);
        frame.render_widget(card_block, card_area);

        let inner = Rect::new(
            card_area.x + 1,
            card_area.y + 1,
            card_area.width.saturating_sub(2),
            card_area.height.saturating_sub(2),
        );
        if inner.height > 0 && !block.styled.is_empty() {
            let max_rows = inner.height as usize;
            let lines: Vec<Line> = block
                .styled
                .iter()
                .take(max_rows)
                .map(|l| {
                    let text: String = l.spans.iter().map(|s| s.content.as_ref()).collect();
                    let display: String = text.chars().take(inner.width as usize).collect();
                    let base_style = l.spans.first().map(|s| s.style).unwrap_or_default();
                    Line::from(Span::styled(display, base_style))
                })
                .collect();
            frame.render_widget(
                Paragraph::new(lines)
                    .style(Style::default().bg(Color::Rgb(20, 24, 38))),
                inner,
            );
        }
    }
}
