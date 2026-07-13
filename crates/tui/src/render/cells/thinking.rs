use crate::render::util::LOG_THINKING_INDENT;
use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

/// Spinner frames for in-progress thinking animation (see `render_loading_spinner` in log.rs).
const THINKING_SPINNER: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
/// Resting frame for collapsed (completed) thinking cards — static, not animated.
const THINKING_DONE_ICON: char = THINKING_SPINNER[THINKING_SPINNER.len() - 1];

/// Render thinking block card overlay.
pub(crate) fn render_thinking_cards(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    visual_scroll: usize,
    visible_height: usize,
) {
    let spinner_char = THINKING_DONE_ICON;
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
        let card_style = Style::default().fg(app.theme.thinking_card_border());
        let visible_count = total_lines.min(3);
        let msgs = app.msgs();
        let elapsed_str = format_elapsed(block.elapsed);

        // Show line count with "Click for full content" hint
        let progress_bar = msgs
            .thinking_card_bottom
            .replacen("{}", &visible_count.to_string(), 1)
            .replacen("{}", &total_lines.to_string(), 1)
            .replacen("{}", &elapsed_str, 1);

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(app.theme.block_border_type())
            .border_style(card_style)
            .style(Style::default().bg(app.theme.bg))
            .title(format!(
                " {} {}",
                spinner_char,
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
            ))
            .title_bottom(progress_bar);

        let card_area = Rect::new(
            area.x + 1 + LOG_THINKING_INDENT,
            area.y + 1 + y_top,
            area.width.saturating_sub(2 + LOG_THINKING_INDENT),
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
                .fg(app.theme.thinking_preview_fg())
                .bg(app.theme.bg);
            let start_preview = block.cached_preview.len().saturating_sub(3);
            let preview_lines: Vec<Line> = block.cached_preview[start_preview..]
                .iter()
                .take(3)
                .map(|s| {
                    let display = if s.len() > inner.width as usize {
                        let max_bytes = (inner.width as usize).saturating_sub(1).min(s.len());
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

fn format_elapsed(d: std::time::Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else {
        let m = secs / 60.0;
        format!("{:.1}min", m)
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use crate::render::test_harness::{buffer_text, make_app};
    use ratatui::{Terminal, backend::TestBackend};
    use tact_protocol::{AgentUpdate, ThinkingChunk};

    #[test]
    fn thinking_card_overlay_renders_collapsed_preview() {
        let mut app = make_app();
        for i in 1..=4 {
            app.handle_agent_update(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
                format!("reason {i}\n"),
            )));
        }
        app.handle_agent_update(AgentUpdate::StreamChunk("answer".into()));
        assert!(!app.thinking.blocks.is_empty());

        // Build visual_start_cache via log panel before isolated overlay draw.
        let _ = crate::render::test_harness::render_log_panel_text(&mut app, 80, 20);

        let backend = TestBackend::new(80, 20);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_thinking_cards(frame, area, &app, 0, area.height as usize);
            })
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("reason 4") || text.contains("Thinking"),
            "thinking overlay should render card preview, got:\n{text}"
        );
    }
}
