use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::UnicodeWidthChar;
use ratatui::text::{Line, Span};
use crate::render::renderable::Renderable;
use crate::render::util::wrap_line;

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
    /// Raw text (used for search highlighting).
    raw_text: String,
    /// Search term.
    search_term: String,
    /// Whether this line is a search match.
    is_search_match: bool,
    /// Whether this line is selected by mouse.
    is_selected: bool,
    /// Word-level selection (start_byte, end_byte); None means line-level selection.
    word_selection: Option<(usize, usize)>,
    /// First line prefix (thinking block collapse indicator).
    prefix: Option<String>,
    /// Normal foreground color.
    fg_color: Color,
}

impl TextCell {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        cached_lines: Vec<Line<'static>>,
        raw_text: String,
        search_term: String,
        is_search_match: bool,
        is_selected: bool,
        word_selection: Option<(usize, usize)>,
        prefix: Option<String>,
        fg_color: Color,
    ) -> Self {
        TextCell {
            cached_lines,
            raw_text,
            search_term,
            is_search_match,
            is_selected,
            word_selection,
            prefix,
            fg_color,
        }
    }

    /// Build the visual line list for rendering (chooses search highlight/selection/cache based on state).
    fn build_lines(&self, wrap_width: u16) -> Vec<Line<'_>> {
        if self.is_search_match {
            return self.build_highlighted_line(wrap_width);
        }
        if self.is_selected {
            if let Some((ws, we)) = self.word_selection {
                return self.build_word_selected_lines(wrap_width, ws, we);
            }
            return self.build_line_selected_lines();
        }
        self.cached_lines.clone()
    }

    fn build_highlighted_line(&self, wrap_width: u16) -> Vec<Line<'static>> {
        let lower_raw = self.raw_text.to_lowercase();
        let lower_term = self.search_term.to_lowercase();
        let mut spans = Vec::new();
        let mut last_idx = 0;

        for (match_idx, _) in lower_raw.match_indices(&lower_term) {
            if match_idx > last_idx {
                spans.push(Span::styled(
                    self.raw_text[last_idx..match_idx].to_string(),
                    Style::default().fg(self.fg_color),
                ));
            }
            let end_idx = match_idx + lower_term.len();
            spans.push(Span::styled(
                self.raw_text[match_idx..end_idx].to_string(),
                Style::default().bg(Color::Yellow).fg(Color::Black),
            ));
            last_idx = end_idx;
        }
        if last_idx < self.raw_text.len() {
            spans.push(Span::styled(
                self.raw_text[last_idx..].to_string(),
                Style::default().fg(self.fg_color),
            ));
        }

        let mut line = Line::from(spans);
        if self.is_selected {
            for span in line.spans.iter_mut() {
                span.style = span.style.add_modifier(Modifier::REVERSED);
            }
        }
        wrap_line(&line, wrap_width as usize)
    }

    fn build_word_selected_lines(&self, wrap_width: u16, ws: usize, we: usize) -> Vec<Line<'static>> {
        let raw = &self.raw_text;
        let w_start = raw.floor_char_boundary(ws.min(raw.len()));
        let w_end = raw.floor_char_boundary(we.min(raw.len()));
        let (w_start, w_end) = if w_end < w_start {
            (w_end, w_start)
        } else {
            (w_start, w_end)
        };
        let before = &raw[..w_start];
        let word = &raw[w_start..w_end];
        let after = &raw[w_end..];
        let styled_line = Line::from(vec![
            Span::raw(before.to_string()),
            Span::styled(word.to_string(), Style::default().add_modifier(Modifier::REVERSED)),
            Span::raw(after.to_string()),
        ]);
        wrap_line(&styled_line, wrap_width as usize)
    }

    fn build_line_selected_lines(&self) -> Vec<Line<'static>> {
        self.cached_lines
            .iter()
            .map(|line| {
                let mut line = line.clone();
                for span in line.spans.iter_mut() {
                    span.style = span.style.add_modifier(Modifier::REVERSED);
                }
                line
            })
            .collect()
    }
}

impl Renderable for TextCell {
    fn height(&self, _width: u16) -> u16 {
        self.cached_lines.len() as u16
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        let lines = self.build_lines(area.width);
        let mut y = area.y;
        for (i, line) in lines.iter().enumerate().skip(skip_lines) {
            if y >= area.y + area.height {
                break;
            }
            let mut line = line.clone();
            // Only add prefix on the first line of the cell (i == 0)
            if i == 0 {
                if let Some(ref prefix) = self.prefix {
                    if let Some(first) = line.spans.first_mut() {
                        first.content = format!("{}{}", prefix, first.content).into();
                    }
                }
            }
            render_line(&line, area.x, y, area.width, buf);
            y += 1;
        }
    }

    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }
}
