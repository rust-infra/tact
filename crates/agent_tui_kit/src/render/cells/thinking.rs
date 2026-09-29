use std::time::Duration;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    i18n::Messages,
    render::{renderable::Renderable, util::LOG_THINKING_INDENT},
    state::{ActiveThinkingBlock, ThinkingBlock},
    theme::Theme,
    widgets::button::{Button, ButtonChrome, ButtonTheme},
};

pub fn thinking_visual_rows(body_lines: usize) -> usize {
    let card_rows = 1 + body_lines.clamp(1, 3) + 1;
    card_rows + 2
}

pub struct ThinkingCell {
    lines: Vec<String>,
    title: String,
    /// Footer text left of the button — see [`Self::footer`].
    bottom: String,
    /// The footer button's label, when the locale's template ends with it.
    bottom_action: Option<&'static str>,
    button_theme: ButtonTheme,
    fg: ratatui::style::Color,
    bg: ratatui::style::Color,
    accent: ratatui::style::Color,
    border_type: ratatui::widgets::BorderType,
}

impl ThinkingCell {
    /// The footer the template fills, split into "everything before the button"
    /// and the button's label.
    ///
    /// The action is the template's **tail**, the rule the collapsed tool card's
    /// meta row follows too, so this is a suffix match rather than a search: a
    /// locale that stopped ending with it draws the readout without a button
    /// instead of a button in the wrong place.
    fn footer(
        msgs: &Messages,
        shown: usize,
        total: usize,
        elapsed: Duration,
    ) -> (String, Option<&'static str>) {
        let label = msgs.thinking_card_action;
        let action = ButtonChrome::Brackets.wrap(label);
        // Filled in template order: elapsed first, then the line count.
        let text = msgs
            .thinking_card_bottom
            .replacen("{}", &format_elapsed(elapsed), 1)
            .replacen("{}", &shown.to_string(), 1)
            .replacen("{}", &total.to_string(), 1)
            .replacen("{}", &action, 1);
        match text.strip_suffix(&action) {
            Some(prefix) => (prefix.to_string(), Some(label)),
            None => (text, None),
        }
    }

    pub fn active(
        block: &ActiveThinkingBlock,
        spinner: char,
        theme: &Theme,
        msgs: &Messages,
    ) -> Self {
        let lines = block.display_tail();
        let visible = lines.len().clamp(1, 3);
        let total = block.content.lines().count().max(1);
        let elapsed = block.started_at.elapsed();
        let (bottom, bottom_action) = Self::footer(msgs, visible, total, elapsed);
        Self {
            lines,
            title: format!(" {spinner}{}", msgs.thinking_card_title),
            bottom,
            bottom_action,
            button_theme: ButtonTheme::from_theme(theme),
            fg: theme.thinking_preview_fg(),
            bg: theme.bg,
            accent: theme.thinking_card_border(),
            border_type: theme.block_border_type(),
        }
    }

    pub fn completed(block: &ThinkingBlock, theme: &Theme, msgs: &Messages) -> Self {
        let total = block.content.lines().count().max(1);
        let (bottom, bottom_action) = Self::footer(msgs, 1, total, block.elapsed);
        Self {
            lines: vec![block.summary.clone()],
            title: msgs.thinking_card_title.to_string(),
            bottom,
            bottom_action,
            button_theme: ButtonTheme::from_theme(theme),
            fg: theme.thinking_preview_fg(),
            bg: theme.bg,
            accent: theme.thinking_card_border(),
            border_type: theme.block_border_type(),
        }
    }

    /// The footer as one line: the readout (elapsed, line count) in the border
    /// color, then the button that opens the full content — drawn by the shared
    /// [`Button`] widget with the same chrome as a collapsed tool card's, so the
    /// two affordances read the same wherever they appear.
    fn bottom_line(&self) -> Line<'static> {
        let style = Style::default().fg(self.accent).bg(self.bg);
        let Some(label) = self.bottom_action else {
            return Line::from(Span::styled(self.bottom.clone(), style));
        };
        let mut spans = vec![Span::styled(self.bottom.clone(), style)];
        spans.extend(
            Button::new(label, self.button_theme)
                .chrome(ButtonChrome::Brackets)
                .line()
                .spans,
        );
        // Keep the closing bracket off the border dashes that follow it — the
        // spacing the other footers carry as a trailing space in their template.
        spans.push(Span::styled(" ", style));
        Line::from(spans)
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

        let body_lines = self.body_lines();
        let card_total = body_lines + 2;
        let card_skip = skip_lines.saturating_sub(1);
        if card_skip >= card_total {
            return;
        }
        let card_offset = usize::from(skip_lines == 0);
        let remaining_height = area.height.saturating_sub(card_offset as u16);
        if remaining_height == 0 {
            return;
        }
        let card_area = Rect::new(
            area.x,
            area.y + card_offset as u16,
            area.width,
            remaining_height.min((card_total - card_skip) as u16),
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
                self.bottom_line()
            } else {
                Line::default()
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
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;

    fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    fn render_text(cell: &ThinkingCell) -> String {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn render_in_viewport(cell: &ThinkingCell, height: u16) -> String {
        let backend = TestBackend::new(80, height);
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        buffer_text(terminal.backend().buffer())
    }

    fn render_buffer(cell: &ThinkingCell) -> ratatui::buffer::Buffer {
        let backend = TestBackend::new(80, cell.height(80));
        let mut terminal = Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| cell.render(frame.area(), frame.buffer_mut()))
            .expect("draw");
        terminal.backend().buffer().clone()
    }

    /// `(row, columns)` of the first run of cells spelling `needle` — every
    /// glyph here is one column wide, so a cell window is a fair match.
    fn find_run(buf: &ratatui::buffer::Buffer, needle: &str) -> Option<(u16, Vec<u16>)> {
        let cells: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        for y in 0..buf.area.height {
            let row: Vec<String> = (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            if let Some(start) = row.windows(cells.len()).position(|w| w == cells) {
                let cols = (start as u16..start as u16 + cells.len() as u16).collect();
                return Some((y, cols));
            }
        }
        None
    }

    fn completed_cell(msgs: &crate::i18n::Messages, theme: &Theme) -> ThinkingCell {
        let block = ThinkingBlock {
            phys_idx: 0,
            content: "first\nlast".into(),
            summary: "last".into(),
            cached_markdown: Vec::new(),
            elapsed: std::time::Duration::from_millis(1500),
        };
        ThinkingCell::completed(&block, theme, msgs)
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
        assert!(text.contains("Thinking"), "{text}");
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
            elapsed: std::time::Duration::ZERO,
        };
        let text = render_text(&ThinkingCell::completed(&block, &theme, &msgs));
        assert!(text.contains("last"), "{text}");
        assert!(!text.contains("first"), "{text}");
        assert!(text.contains("Thinking"), "{text}");
    }

    #[test]
    fn thinking_cell_is_one_card_and_does_not_extend_to_viewport_bottom() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
        active.push_delta("reasoning\n");
        let cell = ThinkingCell::active(&active, 'x', &theme, &msgs);

        let text = render_in_viewport(&cell, 12);
        assert_eq!(text.matches("Thinking").count(), 1, "{text}");
        assert!(
            text.lines()
                .nth(cell.height(80) as usize)
                .is_some_and(|line| line.trim().is_empty()),
            "thinking card must stop at its own height, got:\n{text}"
        );
    }

    #[test]
    fn thinking_cell_leaves_blank_rows_before_and_after_the_card() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let mut active = ActiveThinkingBlock::new(0, std::time::Instant::now());
        active.push_delta("reasoning\n");
        let cell = ThinkingCell::active(&active, 'x', &theme, &msgs);

        let text = render_text(&cell);
        let rows: Vec<_> = text.lines().collect();
        assert!(
            rows.first().is_some_and(|line| line.trim().is_empty()),
            "{text}"
        );
        assert!(
            rows.last().is_some_and(|line| line.trim().is_empty()),
            "{text}"
        );
        assert!(rows.iter().any(|line| line.contains("Thinking")), "{text}");
    }

    /// The footer's action is not more prose: it is the kit's button (brackets
    /// from the chrome, label from the locale) drawn in the button color, with
    /// the readout around it in the border color — the same shape a collapsed
    /// tool card draws.
    #[test]
    fn the_footer_ends_in_the_shared_button() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let en = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let zh = crate::i18n::Messages::by_language(crate::i18n::Language::Chinese);

        for (msgs, expected) in [(&en, "[󰜼 Open]"), (&zh, "[󰜼 打开]")] {
            let cell = completed_cell(msgs, &theme);
            let line = cell.bottom_line();
            let drawn: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
            assert!(drawn.trim_end().ends_with(expected), "{drawn:?}");
            assert!(
                drawn.contains("↕ 1/2 lines") || drawn.contains("↕ 1/2 行"),
                "{drawn:?}"
            );
            // Order is a contract too: the elapsed time leads, the line count
            // follows, each label staying on the readout it names.
            let elapsed_at = drawn
                .find('⏱')
                .unwrap_or_else(|| panic!("no elapsed readout: {drawn:?}"));
            let count_at = drawn
                .find('↕')
                .unwrap_or_else(|| panic!("no line count: {drawn:?}"));
            assert!(
                elapsed_at < count_at && drawn.contains("⏱ 1.5s | ↕ 1/2"),
                "the elapsed time must lead the readout: {drawn:?}"
            );
            assert_eq!(
                line.spans[0].style.fg,
                Some(theme.thinking_card_border()),
                "the readout keeps the border color: {drawn:?}"
            );
            assert_eq!(
                line.spans[1].style.fg,
                Some(theme.muted),
                "the button keeps its own color: {drawn:?}"
            );
        }
    }

    /// The footer reaches the frame as a `title_bottom` line, so pin the drawn
    /// glyphs *and their color* in a real buffer: a title that dropped the
    /// button's spans would still read as text.
    #[test]
    fn the_footer_button_is_drawn_in_the_button_color() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        let msgs = crate::i18n::Messages::by_language(crate::i18n::Language::English);
        let cell = completed_cell(&msgs, &theme);

        let buf = render_buffer(&cell);
        let glyphs = ButtonChrome::Brackets.wrap(msgs.thinking_card_action);
        let (row, cols) = find_run(&buf, &glyphs).unwrap_or_else(|| {
            panic!(
                "the footer button {glyphs:?} must be drawn, got:\n{}",
                buffer_text(&buf)
            )
        });
        for x in &cols {
            assert_eq!(
                buf[(*x, row)].fg,
                theme.muted,
                "column {x} of the button carries the button color"
            );
            assert_eq!(buf[(*x, row)].bg, theme.bg, "painted on the card surface");
        }
        // One column left of it is the separator the readout ends with, drawn
        // in the border color — the button is a patch inside the sentence.
        assert_eq!(buf[(cols[0] - 1, row)].fg, theme.thinking_card_border());
    }

    /// The footer's tail is a contract, not an accident: both locales end the
    /// template with the action, so the split is a suffix match and neither can
    /// silently lose its button.
    #[test]
    fn every_locale_ends_its_footer_with_the_action() {
        let theme = Theme::from(crate::theme::ThemeName::Dark);
        for lang in [
            crate::i18n::Language::English,
            crate::i18n::Language::Chinese,
        ] {
            let msgs = crate::i18n::Messages::by_language(lang);
            let cell = completed_cell(&msgs, &theme);
            assert_eq!(
                cell.bottom_action,
                Some(msgs.thinking_card_action),
                "{lang:?}: the template must end with {}",
                msgs.thinking_card_action
            );
        }
    }
}
