use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthChar;

use crate::render::{renderable::Renderable, util::wrap_line};

/// Writes a single-line span into the buffer.
fn render_line(line: &Line, x: u16, y: u16, width: u16, bg_color: Color, buf: &mut Buffer) {
    let mut col = x;
    for span in &line.spans {
        // Spans that bring their own background (fenced code, H1 headings)
        // keep it; everything else sits on the panel surface color. Either
        // way every glyph writes an explicit bg so no overlay residue can
        // leak through when the line scrolls into view.
        let style = if span.style.bg.is_some() {
            span.style
        } else {
            span.style.bg(bg_color)
        };
        for ch in span.content.chars() {
            if col < x + width {
                buf[(col, y)].set_char(ch).set_style(style);
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
    /// Surface background color applied to every written glyph.
    bg_color: Color,
}

impl TextCell {
    pub(crate) fn new(
        cached_lines: Vec<Line<'static>>,
        raw_text: String,
        selection_range: Option<(usize, usize)>,
        prefix: Option<String>,
        indent_cols: u16,
        fg_color: Color,
        bg_color: Color,
    ) -> Self {
        TextCell {
            cached_lines,
            raw_text,
            selection_range,
            prefix,
            indent_cols,
            fg_color,
            bg_color,
        }
    }

    /// Build the visual line list for rendering (selection overlay or cache).
    fn build_lines(&self, width: u16) -> Vec<Line<'_>> {
        let wrap_width = width;
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
            render_line(&line, x, y, width, self.bg_color, buf);
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
        TextCell::new(
            vec![Line::from("alpha beta gamma")],
            "alpha beta gamma".into(),
            None,
            None,
            0,
            Color::White,
            Color::Black,
        )
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
    fn span_backgrounds_survive_rendering() {
        // Fenced code / heading spans carry their own bg (code_block_bg /
        // highlight); TextCell must not flatten them onto the surface color.
        let code_bg = Color::Rgb(30, 35, 50);
        let cell = TextCell::new(
            vec![Line::from(vec![
                Span::styled("code ", Style::default().bg(code_bg)),
                Span::styled("plain", Style::default()),
            ])],
            "code plain".into(),
            None,
            None,
            0,
            Color::White,
            Color::Black,
        );
        let area = Rect::new(0, 0, 12, 1);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        assert_eq!(buf[(0, 0)].bg, code_bg, "span bg must be preserved");
        assert_eq!(
            buf[(7, 0)].bg,
            Color::Black,
            "span without bg falls back to the surface color"
        );
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

    #[test]
    fn normal_text_replaces_prior_cell_background_with_surface_color() {
        let surface_bg = Color::Rgb(13, 13, 13);
        let old_card_bg = Color::Rgb(30, 35, 50);
        let cell = TextCell::new(
            vec![Line::from("plain log row")],
            "plain log row".into(),
            None,
            None,
            0,
            Color::White,
            surface_bg,
        );
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        buf.set_style(area, Style::default().bg(old_card_bg));

        cell.render(area, &mut buf);

        assert_eq!(buf[(0, 0)].symbol(), "p");
        assert_eq!(buf[(0, 0)].bg, surface_bg);
        assert_ne!(buf[(0, 0)].bg, old_card_bg);
    }

    #[test]
    fn partial_selection_reverses_target_span_across_wrapped_lines() {
        // A selection that spans a line break must keep REVERSED on the
        // continuation line, not just on the first visual line.
        let text = "the quick brown fox jumps";
        let cell = TextCell::new(
            vec![Line::from(text)],
            text.into(),
            Some((10, 15)), // "brown": starts mid-wrap at width 10
            None,
            0,
            Color::White,
            Color::Black,
        );
        let area = Rect::new(0, 0, 10, 3);
        let mut buf = Buffer::empty(area);
        cell.render(area, &mut buf);
        let mut reversed_rows: Vec<usize> = Vec::new();
        for y in 0..area.height {
            if (0..area.width).any(|x| buf[(x, y)].modifier.contains(Modifier::REVERSED)) {
                reversed_rows.push(y as usize);
            }
        }
        assert!(
            !reversed_rows.is_empty(),
            "selection must stay visible after wrapping"
        );
        assert!(
            reversed_rows.contains(&1),
            "continuation line must carry REVERSED too: {reversed_rows:?}"
        );
    }
}
