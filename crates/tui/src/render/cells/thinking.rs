use crate::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Render thinking block card overlay.
pub(crate) fn render_thinking_cards(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    visual_scroll: usize,
    visible_height: usize,
) {
    let vs_cache = &app.log_scroll.visual_start_cache;
    for block in &app.thinking.blocks {
        let Some(title_logical) = app.phys_to_logical_fast(block.title_idx) else {
            continue;
        };
        let blank_after_phys = block.end_idx + 1;
        let Some(blank_after_logical) = app.phys_to_logical_fast(blank_after_phys) else {
            continue;
        };
        if title_logical >= vs_cache.len() || blank_after_logical >= vs_cache.len() {
            continue;
        }
        let vis_card_top = vs_cache[title_logical];
        let vis_card_bottom = vs_cache[blank_after_logical];
        let vis_range_end = visual_scroll + visible_height;
        if vis_card_bottom <= visual_scroll || vis_card_top >= vis_range_end {
            continue;
        }
        let y_top = (vis_card_top.saturating_sub(visual_scroll)) as u16;
        let y_bot = (vis_card_bottom.saturating_sub(visual_scroll)).min(visible_height as _) as u16;
        if y_bot <= y_top {
            continue;
        }

        let total_lines = block.end_idx.saturating_sub(block.title_idx);
        let card_style = Style::default().fg(Color::Rgb(140, 140, 220));
        let visible_count = total_lines.min(3);
        let showing_from = total_lines.saturating_sub(visible_count);
        let msgs = app.msgs();
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(card_style)
            .style(Style::default().bg(app.theme.bg))
            .title(
                msgs.thinking_card_title
                    .replacen("{}", &total_lines.to_string(), 1)
                    .replacen(
                        "{}",
                        if total_lines == 1 {
                            ""
                        } else {
                            msgs.thinking_card_title_pl
                        },
                        1,
                    ),
            )
            .title_bottom(
                msgs.thinking_card_bottom
                    .replacen("{}", &(showing_from + 1).to_string(), 1)
                    .replacen("{}", &total_lines.to_string(), 1),
            );

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
        if inner.height > 0 && !block.cached_preview.is_empty() {
            let preview_style = Style::default()
                .fg(Color::Rgb(180, 180, 200))
                .bg(app.theme.bg);
            let start_preview = block.cached_preview.len().saturating_sub(3);
            let preview_lines: Vec<Line> = block.cached_preview[start_preview..]
                .iter()
                .take(3)
                .map(|s| {
                    let display = if s.len() > inner.width as usize {
                        let max_bytes =
                            (inner.width as usize).saturating_sub(1).min(s.len());
                        let safe_end = s.floor_char_boundary(max_bytes);
                        format!("{}…", &s[..safe_end])
                    } else {
                        s.clone()
                    };
                    Line::from(Span::styled(display, preview_style))
                })
                .collect();
            frame.render_widget(Paragraph::new(preview_lines), inner);
        }
    }
}
