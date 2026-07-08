use crate::widgets::state::App;
use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

/// Map language names to plain labels for code card titles.
fn lang_label(lang: &str) -> String {
    match lang.to_ascii_lowercase().as_str() {
        "rust" | "rs" => "rust".to_string(),
        "python" | "py" => "python".to_string(),
        "javascript" | "js" => "js".to_string(),
        "typescript" | "ts" => "ts".to_string(),
        "html" => "html".to_string(),
        "css" => "css".to_string(),
        "json" => "json".to_string(),
        "yaml" | "yml" => "yaml".to_string(),
        "toml" => "toml".to_string(),
        "markdown" | "md" => "md".to_string(),
        "shell" | "sh" | "bash" | "zsh" => "bash".to_string(),
        "go" => "go".to_string(),
        "ruby" | "rb" => "ruby".to_string(),
        "java" => "java".to_string(),
        "c" => "c".to_string(),
        "cpp" | "c++" => "cpp".to_string(),
        "sql" => "sql".to_string(),
        "dockerfile" | "docker" => "docker".to_string(),
        "makefile" | "make" => "make".to_string(),
        "diff" => "diff".to_string(),
        "text" | "txt" => "text".to_string(),
        "xml" => "xml".to_string(),
        _ => lang.to_string(),
    }
}

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
            lang_label(&block.lang)
        };
        let total_styled = block.styled.len();
        let inner_h = (y_bot.saturating_sub(y_top).saturating_sub(2)) as usize;
        let shown = total_styled.min(inner_h);
        let msgs = app.msgs();
        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_type(app.theme.block_border_type())
            .border_style(Style::default().fg(app.theme.code_card_border()))
            .style(Style::default().bg(app.theme.code_card_bg()))
            .title(Span::styled(
                format!(" {} ", lang_label),
                Style::default()
                    .fg(app.theme.code_card_title_fg())
                    .add_modifier(Modifier::BOLD),
            ))
            .title_bottom(if total_styled > shown {
                Line::from(Span::styled(
                    format!(" +{} lines | {}", total_styled - shown, msgs.code_card_bottom),
                    Style::default().fg(app.theme.muted_fg()),
                ))
            } else {
                Line::from(Span::styled(
                    msgs.code_card_bottom,
                    Style::default().fg(app.theme.muted_fg()),
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
                    .style(Style::default().bg(app.theme.code_card_bg())),
                inner,
            );
        }
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use crate::render::test_harness::{buffer_text, make_app};
    use ratatui::{Terminal, backend::TestBackend};
    use tact_protocol::AgentUpdate;

    #[test]
    fn code_card_overlay_renders_language_and_body() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::StreamChunk(
            "```rust\nfn overlay_test() {}\n```\n".into(),
        ));
        assert!(!app.code_blocks.is_empty());

        let _ = crate::render::test_harness::render_log_panel_text(&mut app, 80, 18);

        let backend = TestBackend::new(80, 18);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| {
                let area = frame.area();
                render_code_cards(frame, area, &app, 0, area.height as usize);
            })
            .expect("draw");

        let text = buffer_text(terminal.backend().buffer());
        assert!(
            text.contains("overlay_test"),
            "code overlay should render code body text, got:\n{text}"
        );
    }
}
