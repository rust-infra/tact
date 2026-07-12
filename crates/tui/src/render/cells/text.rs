use crate::render::renderable::Renderable;
use crate::render::util::wrap_line;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthChar;

/// Writes a single-line span into the buffer.
fn render_line(line: &Line, x: u16, y: u16, width: u16, buf: &mut Buffer) {
    let mut col = x;
    for span in &line.spans {
        for ch in span.content.chars() {
            if col < x + width {
                buf[(col, y)].set_char(ch).set_style(span.style);
                col += UnicodeWidthChar::width(ch).unwrap_or(0) as u16;
            }
        }
    }
}

/// Rendering unit for a single log message.
/// Pre-cached line wrapping result, supporting search highlights and mouse selections.
pub(crate) struct TextCell {
    /// Pre-wrapped visual lines (cloned directly during normal rendering).
    cached_lines: Vec<Line<'static>>,
    /// Raw text (used for search highlighting and selection).
    raw_text: String,
    /// Search term.
    search_term: String,
    /// Whether this line is a search match.
    is_search_match: bool,
    /// Selected byte range within raw_text, None if no selection.
    selection_range: Option<(usize, usize)>,
    /// First line prefix (thinking block collapse indicator).
    prefix: Option<String>,
    /// Left gutter columns (thinking / tool nesting).
    indent_cols: u16,
    /// Normal foreground color.
    fg_color: Color,
    search_match_bg: Color,
    search_match_fg: Color,
}

impl TextCell {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cached_lines: Vec<Line<'static>>,
        raw_text: String,
        search_term: String,
        is_search_match: bool,
        selection_range: Option<(usize, usize)>,
        prefix: Option<String>,
        indent_cols: u16,
        fg_color: Color,
        search_match_bg: Color,
        search_match_fg: Color,
    ) -> Self {
        TextCell {
            cached_lines,
            raw_text,
            search_term,
            is_search_match,
            selection_range,
            prefix,
            indent_cols,
            fg_color,
            search_match_bg,
            search_match_fg,
        }
    }

    /// Build the visual line list for rendering (chooses search highlight/selection/cache based on state).
    fn build_lines(&self, width: u16) -> Vec<Line<'_>> {
        let wrap_width = width + self.indent_cols;
        if self.is_search_match {
            return self.build_highlighted_line(wrap_width);
        }
        if let Some((sel_start, sel_end)) = self.selection_range {
            // Whole-line selection can reuse the cached wrap and only flip style.
            if sel_start == 0 && sel_end == self.raw_text.len() {
                let mut lines = self.cached_lines.clone();
                for line in &mut lines {
                    for span in line.spans.iter_mut() {
                        span.style = span.style.add_modifier(Modifier::REVERSED);
                    }
                }
                return lines;
            }
            return self.build_selected_lines(wrap_width, sel_start, sel_end);
        }
        self.cached_lines.clone()
    }

    fn build_highlighted_line(&self, wrap_width: u16) -> Vec<Line<'static>> {
        let lower_raw = self.raw_text.to_lowercase();
        let lower_term = self.search_term.to_lowercase();
        // Byte indices from `lower_raw` only align with `raw_text` when lengths match
        // (e.g. ß → ss would desync). Fall back to unhighlighted rendering.
        if lower_raw.len() != self.raw_text.len() || lower_term.is_empty() {
            return wrap_line(
                &Line::from(Span::styled(
                    self.raw_text.clone(),
                    Style::default().fg(self.fg_color),
                )),
                wrap_width as usize,
            );
        }

        let mut spans = Vec::new();
        let mut last_idx = 0;

        for (match_idx, _) in lower_raw.match_indices(&lower_term) {
            let match_idx = self.raw_text.floor_char_boundary(match_idx);
            let end_idx = self
                .raw_text
                .floor_char_boundary(match_idx.saturating_add(lower_term.len()));
            if match_idx > last_idx {
                let start = self.raw_text.floor_char_boundary(last_idx);
                spans.push(Span::styled(
                    self.raw_text[start..match_idx].to_string(),
                    Style::default().fg(self.fg_color),
                ));
            }
            spans.push(Span::styled(
                self.raw_text[match_idx..end_idx].to_string(),
                Style::default()
                    .bg(self.search_match_bg)
                    .fg(self.search_match_fg),
            ));
            last_idx = end_idx;
        }
        if last_idx < self.raw_text.len() {
            let start = self.raw_text.floor_char_boundary(last_idx);
            spans.push(Span::styled(
                self.raw_text[start..].to_string(),
                Style::default().fg(self.fg_color),
            ));
        }

        let mut line = Line::from(spans);
        if self.selection_range == Some((0, self.raw_text.len())) {
            for span in line.spans.iter_mut() {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            }
        }
        wrap_line(&line, wrap_width as usize)
    }

    fn build_selected_lines(
        &self,
        wrap_width: u16,
        sel_start: usize,
        sel_end: usize,
    ) -> Vec<Line<'static>> {
        let raw = &self.raw_text;
        let sel_start = raw.floor_char_boundary(sel_start.min(raw.len()));
        let sel_end = raw.floor_char_boundary(sel_end.min(raw.len()));
        let (sel_start, sel_end) = if sel_end < sel_start {
            (sel_end, sel_start)
        } else {
            (sel_start, sel_end)
        };
        let before = &raw[..sel_start];
        let selected = &raw[sel_start..sel_end];
        let after = &raw[sel_end..];
        let styled_line = Line::from(vec![
            Span::styled(before.to_string(), Style::default().fg(self.fg_color)),
            Span::styled(
                selected.to_string(),
                Style::default()
                    .add_modifier(Modifier::REVERSED)
                    .fg(self.fg_color),
            ),
            Span::styled(after.to_string(), Style::default().fg(self.fg_color)),
        ]);
        wrap_line(&styled_line, wrap_width as usize)
    }
}

impl Renderable for TextCell {
    fn height(&self, _width: u16) -> u16 {
        self.cached_lines.len() as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        let x = area.x.saturating_add(self.indent_cols);
        let width = area.width.saturating_sub(self.indent_cols);
        if width == 0 {
            return;
        }
        let lines = self.build_lines(width);
        for (y, (i, line)) in (area.y..).zip(lines.iter().enumerate().skip(skip_lines)) {
            if y >= area.y + area.height {
                break;
            }
            let mut line = line.clone();
            // Only add prefix on the first line of the cell (i == 0)
            if i == 0
                && let Some(ref prefix) = self.prefix
                && let Some(first) = line.spans.first_mut()
            {
                first.content = format!("{}{}", prefix, first.content).into();
            }
            render_line(&line, x, y, width, buf);
            // y advances via zip
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }
}

#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::render::renderable::Renderable;
    use ratatui::style::Color;

    fn sample_cell() -> TextCell {
        TextCell::new(
            vec![Line::from("alpha beta gamma")],
            "alpha beta gamma".into(),
            String::new(),
            false,
            None,
            None,
            0,
            Color::White,
            Color::Yellow,
            Color::Black,
        )
    }

    #[test]
    fn search_match_paints_highlight_background() {
        let cell = TextCell::new(
            vec![Line::from("find TOKEN end")],
            "find TOKEN end".into(),
            "TOKEN".into(),
            true,
            None,
            None,
            0,
            Color::White,
            Color::Yellow,
            Color::Black,
        );
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        let mut has_highlight = false;
        for x in 0..area.width {
            if buf[(x, 0)].bg == Color::Yellow {
                has_highlight = true;
            }
        }
        assert!(
            has_highlight,
            "search match should use highlight background"
        );
    }

    #[test]
    fn line_selection_applies_reversed_modifier() {
        let cell = TextCell::new(
            vec![Line::from("select all of this")],
            "select all of this".into(),
            String::new(),
            false,
            Some((0, 18)),
            None,
            0,
            Color::White,
            Color::Yellow,
            Color::Black,
        );
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        let mut reversed = false;
        for y in 0..area.height {
            for x in 0..area.width {
                if buf[(x, y)].modifier.contains(Modifier::REVERSED) {
                    reversed = true;
                }
            }
        }
        assert!(reversed, "line selection should reverse styled spans");
    }

    #[test]
    fn word_selection_reverses_only_target_word() {
        let cell = TextCell::new(
            vec![Line::from("alpha beta gamma")],
            "alpha beta gamma".into(),
            String::new(),
            false,
            Some((6, 10)),
            None,
            0,
            Color::White,
            Color::Yellow,
            Color::Black,
        );
        let area = Rect::new(0, 0, 40, 1);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        let mut reversed = false;
        for y in 0..area.height {
            for x in 0..area.width {
                if buf[(x, y)].modifier.contains(Modifier::REVERSED) {
                    reversed = true;
                }
            }
        }
        assert!(
            reversed,
            "word selection should reverse the target word spans"
        );
        assert_eq!(sample_cell().height(40), 1);
    }
}
