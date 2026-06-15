use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use crate::state::App;

/// Render diff block card overlay.
pub(crate) fn render_diff_cards(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    visual_scroll: usize,
    visible_height: usize,
) {
    let vs_cache = &app.log_scroll.visual_start_cache;
    for block in &app.diff_blocks {
        let Some(start_logical) = app.phys_to_logical_fast(block.start_idx) else {
            continue;
        };
        let Some(end_logical) = app.phys_to_logical_fast(block.end_idx) else {
            continue;
        };
        if start_logical >= vs_cache.len() || end_logical >= vs_cache.len() {
            continue;
        }
        let vis_top = vs_cache[start_logical];
        let vis_bot = vs_cache[end_logical];
        let vis_range_end = visual_scroll + visible_height;
        if vis_bot <= visual_scroll || vis_top >= vis_range_end {
            continue;
        }
        let y_top = (vis_top.saturating_sub(visual_scroll)) as u16;
        let y_bot = (vis_bot.saturating_sub(visual_scroll)).min(visible_height as _) as u16;
        if y_bot <= y_top {
            continue;
        }

        let total_lines = block.line_count;

        let msgs = app.msgs();
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(app.theme.accent))
            .style(Style::default().bg(app.theme.bg))
            .title(
                msgs.diff_card_title
                    .replacen("{}", &total_lines.to_string(), 1)
                    .replacen("{}", &block.file_path, 1),
            )
            .title_bottom(Line::from(Span::styled(
                msgs.diff_card_bottom,
                Style::default().fg(app.theme.accent),
            )));

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
        if inner.height > 0 {
            let max_visible = inner.height as usize;
            let num_width = (total_lines + 1).to_string().len().max(3);
            let code_width = (inner.width as usize).saturating_sub(num_width + 3);
            let num_style = Style::default().fg(Color::Gray).bg(app.theme.bg);
            let text_style = Style::default().fg(app.theme.fg).bg(app.theme.bg);
            let plus_style = Style::default().fg(app.theme.success).bg(app.theme.bg);

            let mut preview_lines: Vec<Line> = block
                .preview_lines
                .iter()
                .take(max_visible)
                .enumerate()
                .map(|(i, line)| {
                    let num = format!("{:>nw$}", i + 1, nw = num_width);
                    let trimmed: String = line.chars().take(code_width).collect();
                    Line::from(vec![
                        Span::styled(format!(" {} ", num), num_style),
                        Span::styled("+ ", plus_style),
                        Span::styled(trimmed, text_style),
                    ])
                })
                .collect();

            if total_lines > max_visible {
                preview_lines.push(Line::from(Span::styled(
                    app.msgs()
                        .diff_overflow_tmpl
                        .replace("{}", &(total_lines - max_visible).to_string()),
                    Style::default().fg(Color::Gray).bg(app.theme.bg),
                )));
            }

            frame.render_widget(Paragraph::new(preview_lines), inner);
        }
    }
}
