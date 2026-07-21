use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::render::{renderable::Renderable, util::wrap_line};

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
/// Pre-cached line wrapping result, supporting mouse selections.
pub(crate) struct TextCell {
    /// Pre-wrapped visual lines (cloned directly during normal rendering).
    cached_lines: Vec<Line<'static>>,
    /// Raw text (used for selection).
    raw_text: String,
    /// Selected byte range within raw_text, None if no selection.
    selection_range: Option<(usize, usize)>,
    /// First line prefix (thinking block collapse indicator).
    prefix: Option<String>,
    /// Left gutter columns (thinking / tool nesting).
    indent_cols: u16,
    /// Normal foreground color.
    fg_color: Color,
}

impl TextCell {
    pub(crate) fn new(
        cached_lines: Vec<Line<'static>>,
        raw_text: String,
        selection_range: Option<(usize, usize)>,
        prefix: Option<String>,
        indent_cols: u16,
        fg_color: Color,
    ) -> Self {
        TextCell { cached_lines, raw_text, selection_range, prefix, indent_cols, fg_color }
    }

    /// Build the visual line list for rendering (selection overlay or cache).
    fn build_lines(&self, width: u16) -> Vec<Line<'_>> {
        let wrap_width = width + self.indent_cols;
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

    fn build_selected_lines(&self, wrap_width: u16, sel_start: usize, sel_end: usize) -> Vec<Line<'static>> {
        let raw = &self.raw_text;
        let sel_start = raw.floor_char_boundary(sel_start.min(raw.len()));
        let sel_end = raw.floor_char_boundary(sel_end.min(raw.len()));
        let (sel_start, sel_end) = if sel_end < sel_start { (sel_end, sel_start) } else { (sel_start, sel_end) };
        let before = &raw[..sel_start];
        let selected = &raw[sel_start..sel_end];
        let after = &raw[sel_end..];
        let styled_line = Line::from(vec![
            Span::styled(before.to_string(), Style::default().fg(self.fg_color)),
            Span::styled(selected.to_string(), Style::default().add_modifier(Modifier::REVERSED).fg(self.fg_color)),
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
    use ratatui::style::Color;

    use super::*;
    use crate::render::renderable::Renderable;

    fn sample_cell() -> TextCell {
        TextCell::new(vec![Line::from("alpha beta gamma")], "alpha beta gamma".into(), None, None, 0, Color::White)
    }

    #[test]
    fn line_selection_applies_reversed_modifier() {
        let cell = TextCell::new(
            vec![Line::from("select all of this")],
            "select all of this".into(),
            Some((0, 18)),
            None,
            0,
            Color::White,
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
            Some((6, 10)),
            None,
            0,
            Color::White,
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
        assert!(reversed, "word selection should reverse the target word spans");
        assert_eq!(sample_cell().height(40), 1);
    }
}
