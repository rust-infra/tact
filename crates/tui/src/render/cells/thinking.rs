use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    i18n::Messages,
    render::{renderable::Renderable, util::LOG_THINKING_INDENT},
    theme::Theme,
    widgets::state::{ActiveThinkingBlock, ThinkingBlock},
};

pub(crate) const THINKING_HEADER_ROWS: usize = 1;

pub(crate) fn thinking_visual_rows(body_lines: usize) -> usize {
    THINKING_HEADER_ROWS + 1 + body_lines.clamp(1, 3) + 1
}

pub(crate) struct ThinkingCell {
    lines: Vec<String>,
    elapsed: Duration,
    title: String,
    bottom: String,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    accent: ratatui::style::Color,
    border_type: ratatui::widgets::BorderType,
}

impl ThinkingCell {
    pub(crate) fn active(
        block: &ActiveThinkingBlock,
        spinner: char,
        theme: &Theme,
        msgs: &Messages,
    ) -> Self {
        let lines = block.display_tail();
        let visible = lines.len().clamp(1, 3);
        let total = block.content.lines().count().max(1);
        Self {
            lines,
            elapsed: block.started_at.elapsed(),
            title: format!(
                " {spinner}{}",
                msgs.thinking_card_title
                    .replacen("{}", &visible.to_string(), 1)
                    .replacen(
                        "{}",
                        if visible == 1 {
                            ""
                        } else {
                            msgs.thinking_card_title_pl
                        },
                        1
                    )
            ),
            bottom: msgs
                .thinking_card_bottom
                .replacen("{}", &visible.to_string(), 1)
                .replacen("{}", &total.to_string(), 1)
                .replacen("{}", "streaming", 1),
            fg: theme.thinking_preview_fg(),
            bg: theme.bg,
            accent: theme.thinking_card_border(),
            border_type: theme.block_border_type(),
        }
    }

    pub(crate) fn completed(block: &ThinkingBlock, theme: &Theme, msgs: &Messages) -> Self {
        Self {
            lines: vec![block.summary.clone()],
            elapsed: block.elapsed,
            title: msgs
                .thinking_card_title
                .replacen("{}", "1", 1)
                .replacen("{}", "", 1),
            bottom: msgs
                .thinking_card_bottom
                .replacen("{}", "1", 1)
                .replacen("{}", &block.content.lines().count().max(1).to_string(), 1)
                .replacen("{}", &format_elapsed(block.elapsed), 1),
            fg: theme.thinking_preview_fg(),
            bg: theme.bg,
            accent: theme.thinking_card_border(),
            border_type: theme.block_border_type(),
        }
    }

    fn body_lines(&self) -> usize {
        self.lines.len().clamp(1, 3)
    }

    fn truncated_lines(&self, width: u16) -> Vec<Line<'static>> {
        let max = width as usize;
        let style = Style::default().fg(self.fg).bg(self.bg);
        let mut lines = self.lines.clone();
        if lines.is_empty() {
            lines.push(String::new());
        }
        lines
            .into_iter()
            .take(self.body_lines())
            .map(|line| {
                let display = if line.chars().count() > max && max > 0 {
                    let end = line.floor_char_boundary(max.saturating_sub(1));
                    format!("{}…", &line[..end])
                } else {
                    line
                };
                Line::from(Span::styled(display, style))
            })
            .collect()
    }

    fn header(&self) -> Line<'static> {
        Line::from(Span::styled(
            format!("{} | {}", self.title.trim(), format_elapsed(self.elapsed)),
            Style::default()
                .fg(self.accent)
                .bg(self.bg)
                .add_modifier(Modifier::BOLD),
        ))
    }
}

impl Renderable for ThinkingCell {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn height(&self, _width: u16) -> u16 {
        thinking_visual_rows(self.body_lines()) as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        let area = crate::render::util::indent_rect(area, LOG_THINKING_INDENT);
        if area.width == 0 || area.height == 0 {
            return;
        }

        if skip_lines == 0 {
            Paragraph::new(self.header())
                .style(Style::default().bg(self.bg))
                .render(Rect::new(area.x, area.y, area.width, 1), buf);
        }

        let body_lines = self.body_lines();
        let card_total = body_lines + 2;
        let card_skip = skip_lines.saturating_sub(THINKING_HEADER_ROWS);
        if card_skip >= card_total {
            return;
        }
        let y_offset = THINKING_HEADER_ROWS.saturating_sub(skip_lines) as u16;
        let card_area = Rect::new(
            area.x,
            area.y + y_offset,
            area.width,
            area.height.saturating_sub(y_offset),
        );
        if card_area.height == 0 {
            return;
        }

        let mut borders = Borders::LEFT | Borders::RIGHT;
        if card_skip == 0 {
            borders |= Borders::TOP;
        }
        if card_skip + card_area.height as usize >= card_total {
            borders |= Borders::BOTTOM;
        }
        let block = Block::default()
            .borders(borders)
            .border_type(self.border_type)
            .border_style(Style::default().fg(self.accent))
            .style(Style::default().bg(self.bg))
            .title(if card_skip == 0 {
                self.title.clone()
            } else {
                String::new()
            })
            .title_bottom(if borders.contains(Borders::BOTTOM) {
                self.bottom.clone()
            } else {
                String::new()
            });
        block.render(card_area, buf);

        let first_line = card_skip.saturating_sub(1);
        let top_border = usize::from(card_skip == 0);
        let inner = Rect::new(
            card_area.x + 1,
            card_area.y + top_border as u16,
            card_area.width.saturating_sub(2),
            card_area.height.saturating_sub(
                top_border as u16 + usize::from(borders.contains(Borders::BOTTOM)) as u16,
            ),
        );
        if inner.height > 0 && first_line < body_lines {
            Paragraph::new(self.truncated_lines(inner.width)[first_line..].to_vec())
                .style(Style::default().bg(self.bg))
                .render(inner, buf);
        }
    }
}

fn format_elapsed(duration: Duration) -> String {
    if duration.as_secs() == 0 {
        format!("{}ms", duration.as_millis())
    } else {
        format!("{:.1}s", duration.as_secs_f64())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::buffer_text;
    use ratatui::{Terminal, backend::TestBackend};

    fn render_text(cell: &ThinkingCell) -> String {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    #[test]
    fn active_thinking_cell_stops_growing_after_three_lines() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut three = ActiveThinkingBlock::new(0, std::time::Instant::now());
        three.push_delta("one\ntwo\nthree\n");
        let mut four = ActiveThinkingBlock::new(0, std::time::Instant::now());
        four.push_delta("one\ntwo\nthree\nfour\n");
        let three = ThinkingCell::active(&three, 'x', &theme, &msgs);
        let four = ThinkingCell::active(&four, 'x', &theme, &msgs);

        assert_eq!(three.height(80), four.height(80));
        let text = render_text(&four);
        assert!(text.contains("two") && text.contains("four"), "{text}");
        assert!(!text.contains("one"), "{text}");
    }

    #[test]
    fn completed_thinking_cell_renders_only_its_summary() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let block = ThinkingBlock {
            phys_idx: 0,
            content: "first\nlast".into(),
            summary: "last".into(),
            cached_markdown: Vec::new(),
            elapsed: Duration::ZERO,
        };
        let text = render_text(&ThinkingCell::completed(&block, &theme, &msgs));
        assert!(text.contains("last"), "{text}");
        assert!(!text.contains("first"), "{text}");
    }
}
