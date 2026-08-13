use ratatui::{
    buffer::CellWidth,
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

use crate::widgets::state::{PopupHitRow, PopupTextHit};

#[derive(Debug, Clone, Copy)]
pub(crate) struct SourceLine<'a> {
    pub(crate) text: &'a str,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

#[derive(Debug, Clone)]
struct DisplayGrapheme {
    symbol: String,
    hit: PopupTextHit,
    style: Style,
}

#[derive(Debug, Clone)]
pub(crate) struct DisplayRow {
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    graphemes: Vec<DisplayGrapheme>,
    pub(crate) cells: Vec<PopupTextHit>,
    /// Visual cells reserved before the first source grapheme (for example,
    /// the continuation indent of a wrapped list item). These cells are not
    /// part of the selectable source text.
    prefix_cells: usize,
}

/// Wrapped-layout snapshot reused across frames while the popup content and
/// body width are unchanged. Avoids re-wrapping the whole conversation (which
/// can be tens of thousands of lines for a live subagent) on every render.
#[derive(Debug, Clone)]
pub(crate) struct PopupLayoutCache {
    /// Live output grows over time; markdown vs plain styling differs. Both are
    /// part of the key so a live→completed transition invalidates the cache.
    pub(crate) is_live: bool,
    pub(crate) content_len: usize,
    pub(crate) width: u16,
    pub(crate) raw_text: String,
    pub(crate) display_rows: Vec<DisplayRow>,
    pub(crate) line_count: usize,
}

impl PopupLayoutCache {
    pub(crate) fn is_valid(&self, is_live: bool, content_len: usize, width: u16) -> bool {
        self.is_live == is_live && self.content_len == content_len && self.width == width
    }
}

fn hit_intersects(hit: PopupTextHit, selection: &std::ops::Range<usize>) -> bool {
    hit.start < selection.end && hit.end > selection.start
}

impl DisplayRow {
    pub(crate) fn hit_row(&self, screen_y: u16, text_x: u16) -> PopupHitRow {
        let mut cells = vec![PopupTextHit::empty(self.line_start); self.prefix_cells];
        cells.extend(self.cells.iter().copied());
        PopupHitRow {
            screen_y,
            text_x,
            line_start: self.line_start,
            line_end: self.line_end,
            cells,
        }
    }

    pub(crate) fn spans(&self, selection: Option<&std::ops::Range<usize>>) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        if self.prefix_cells > 0 {
            spans.push(Span::raw(" ".repeat(self.prefix_cells)));
        }
        let mut content = String::new();
        let mut style = None;

        for grapheme in &self.graphemes {
            let grapheme_selected =
                selection.is_some_and(|range| hit_intersects(grapheme.hit, range));
            let grapheme_style = if grapheme_selected {
                grapheme.style.add_modifier(Modifier::REVERSED)
            } else {
                grapheme.style
            };
            if let Some(current) = style.filter(|current| *current != grapheme_style) {
                spans.push(Span::styled(std::mem::take(&mut content), current));
            }
            style = Some(grapheme_style);
            content.push_str(&grapheme.symbol);
        }
        if let Some(style) = style {
            spans.push(Span::styled(content, style));
        }
        spans
    }
}

pub(crate) fn source_lines(content: &str) -> Vec<SourceLine<'_>> {
    if content.is_empty() {
        return vec![SourceLine {
            text: "",
            start: 0,
            end: 0,
        }];
    }

    let mut lines = Vec::new();
    let mut line_start = 0;
    for (newline, ch) in content.char_indices() {
        if ch != '\n' {
            continue;
        }
        let line_end = if newline > line_start && content.as_bytes()[newline - 1] == b'\r' {
            newline - 1
        } else {
            newline
        };
        lines.push(SourceLine {
            text: &content[line_start..line_end],
            start: line_start,
            end: line_end,
        });
        line_start = newline + 1;
    }
    if line_start < content.len() {
        lines.push(SourceLine {
            text: &content[line_start..],
            start: line_start,
            end: content.len(),
        });
    }
    if lines.is_empty() {
        lines.push(SourceLine {
            text: content,
            start: 0,
            end: content.len(),
        });
    }
    lines
}

pub(crate) fn scalar_styles(
    line: Option<&Line<'_>>,
    fallback: Style,
    scalar_count: usize,
) -> Vec<Style> {
    let Some(line) = line else {
        return vec![fallback; scalar_count];
    };
    let mut styles: Vec<_> = line
        .spans
        .iter()
        .flat_map(|span| {
            std::iter::repeat_n(line.style.patch(span.style), span.content.chars().count())
        })
        .collect();
    styles.resize(scalar_count, fallback);
    styles
}

/// Width of the prefix before the content of a Markdown list item.
///
/// Wrapped list rows use this as a hanging indent so continuation text lines
/// up with the item content instead of the list marker.
pub(crate) fn list_hanging_indent(text: &str) -> usize {
    let leading = text.len() - text.trim_start().len();
    let trimmed = &text[leading..];

    let marker_end = {
        let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
        if digits > 0 && trimmed.as_bytes().get(digits) == Some(&b'.') {
            digits + 1
        } else {
            let Some(marker) = trimmed.chars().next() else {
                return 0;
            };
            if !matches!(marker, '-' | '*' | '+' | '•') {
                return 0;
            }
            marker.len_utf8()
        }
    };

    let whitespace_len = trimmed[marker_end..]
        .char_indices()
        .take_while(|(_, ch)| ch.is_whitespace())
        .last()
        .map_or(0, |(offset, ch)| offset + ch.len_utf8());
    if whitespace_len == 0 {
        return 0;
    }

    unicode_width::UnicodeWidthStr::width(&text[..leading + marker_end + whitespace_len])
}

/// Wrap every source line of `raw_text` into display rows, applying the styles
/// from `styled_lines` (falling back to `fallback` where a line is missing).
pub(crate) fn layout_all_display_rows(
    raw_text: &str,
    styled_lines: &[Line<'_>],
    fallback: Style,
    max_width: usize,
) -> Vec<DisplayRow> {
    let mut display_rows = Vec::new();
    for (index, source_line) in source_lines(raw_text).iter().enumerate() {
        let styles = scalar_styles(
            styled_lines.get(index),
            fallback,
            source_line.text.chars().count(),
        );
        display_rows.extend(layout_display_rows_with_hanging_indent(
            source_line.text,
            source_line.start,
            &styles,
            max_width,
            true,
            list_hanging_indent(source_line.text),
        ));
    }
    display_rows
}

pub(crate) fn layout_display_rows(
    text: &str,
    line_start: usize,
    styles: &[Style],
    max_width: usize,
    wrap: bool,
) -> Vec<DisplayRow> {
    layout_display_rows_with_hanging_indent(text, line_start, styles, max_width, wrap, 0)
}

pub(crate) fn layout_display_rows_with_hanging_indent(
    text: &str,
    line_start: usize,
    styles: &[Style],
    max_width: usize,
    wrap: bool,
    hanging_indent: usize,
) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    let mut graphemes: Vec<DisplayGrapheme> = Vec::new();
    let mut cells: Vec<PopupTextHit> = Vec::new();
    let mut row_start = line_start;
    let mut row_end = line_start;
    let mut row_width = 0;
    let mut row_prefix = 0;
    // Leave at least one cell for content when the caller supplies a narrow
    // width. Normal popup widths are much larger than any list marker.
    let hanging_indent = hanging_indent.min(max_width.saturating_sub(1));

    let push_row = |rows: &mut Vec<DisplayRow>,
                    graphemes: &mut Vec<DisplayGrapheme>,
                    cells: &mut Vec<PopupTextHit>,
                    line_start,
                    line_end,
                    prefix_cells| {
        rows.push(DisplayRow {
            line_start,
            line_end,
            graphemes: std::mem::take(graphemes),
            cells: std::mem::take(cells),
            prefix_cells,
        });
    };

    let grapheme_cells = |g: &DisplayGrapheme| -> usize {
        if g.symbol.contains(char::is_control) {
            0
        } else {
            usize::from(g.symbol.cell_width())
        }
    };

    let mut scalar_index = 0;
    for (relative_start, symbol) in text.grapheme_indices(true) {
        let start = line_start + relative_start;
        let end = start + symbol.len();
        let width = if symbol.contains(char::is_control) {
            0
        } else {
            usize::from(symbol.cell_width())
        };

        if width > 0 && row_width + width > max_width {
            if wrap && !graphemes.is_empty() {
                // Soft-break at the last whitespace when possible so words like
                // "outside" stay intact instead of splitting mid-word.
                let break_at = graphemes
                    .iter()
                    .enumerate()
                    .rev()
                    .find(|(_, g)| g.symbol.chars().all(char::is_whitespace))
                    .map(|(i, _)| i)
                    .filter(|&i| i > 0);

                if let Some(idx) = break_at {
                    let keep_width: usize = graphemes[..idx].iter().map(grapheme_cells).sum();
                    let mut rest = graphemes.split_off(idx);
                    let _space = rest.remove(0); // drop the break whitespace
                    let mut rest_cells = cells.split_off(keep_width);
                    let space_width = grapheme_cells(&_space);
                    if space_width > 0 && rest_cells.len() >= space_width {
                        rest_cells.drain(..space_width);
                    }
                    let kept_end = graphemes.last().map(|g| g.hit.end).unwrap_or(row_start);
                    push_row(
                        &mut rows,
                        &mut graphemes,
                        &mut cells,
                        row_start,
                        kept_end,
                        row_prefix,
                    );
                    graphemes = rest;
                    cells = rest_cells;
                    row_prefix = hanging_indent;
                    row_width = row_prefix + graphemes.iter().map(grapheme_cells).sum::<usize>();
                    row_start = graphemes.first().map(|g| g.hit.start).unwrap_or(start);
                } else {
                    let current_end = graphemes.last().map(|g| g.hit.end).unwrap_or(row_start);
                    push_row(
                        &mut rows,
                        &mut graphemes,
                        &mut cells,
                        row_start,
                        current_end,
                        row_prefix,
                    );
                    row_prefix = hanging_indent;
                    row_start = start;
                    row_width = row_prefix;
                }
            } else if wrap && row_prefix > 0 {
                // A wide grapheme may not fit in the remaining space after the
                // continuation indent; fall back to the left edge rather than
                // emitting a row wider than the viewport.
                row_prefix = 0;
                row_width = 0;
            } else if !wrap {
                break;
            }
        }

        let style = styles.get(scalar_index).copied().unwrap_or_default();
        scalar_index += symbol.chars().count();
        if width == 0 {
            if let Some(previous_hit) = cells.last().copied() {
                for cell in cells.iter_mut().rev() {
                    if *cell != previous_hit {
                        break;
                    }
                    cell.end = end;
                }
                if let Some(previous_grapheme) = graphemes
                    .iter_mut()
                    .rev()
                    .find(|grapheme| grapheme.hit == previous_hit)
                {
                    previous_grapheme.hit.end = end;
                }
            }
            graphemes.push(DisplayGrapheme {
                symbol: symbol.to_owned(),
                hit: PopupTextHit::new(start, end),
                style,
            });
            row_end = end;
            continue;
        }

        let hit_start = if cells.is_empty() {
            graphemes
                .first()
                .map_or(start, |grapheme| grapheme.hit.start)
        } else {
            start
        };
        let hit = PopupTextHit::new(hit_start, end);
        graphemes.push(DisplayGrapheme {
            symbol: symbol.to_owned(),
            hit,
            style,
        });
        cells.extend(std::iter::repeat_n(hit, width));
        row_width += width;
        row_end = end;
    }

    if !graphemes.is_empty() || rows.is_empty() {
        push_row(
            &mut rows,
            &mut graphemes,
            &mut cells,
            row_start,
            row_end,
            row_prefix,
        );
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    fn row_text(row: &DisplayRow) -> String {
        row.spans(None).into_iter().map(|s| s.content).collect()
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        // "If you are outside" is 18 cells; width 14 forces a wrap.
        // Soft-break should keep "outside" intact: "If you are" / "outside".
        let rows = layout_display_rows("If you are outside", 0, &[], 14, true);
        let texts: Vec<String> = rows.iter().map(row_text).collect();
        assert_eq!(texts, vec!["If you are", "outside"]);
    }

    #[test]
    fn wrap_hard_breaks_unbroken_token_when_needed() {
        let rows = layout_display_rows("abcdef", 0, &[], 3, true);
        let texts: Vec<String> = rows.iter().map(row_text).collect();
        assert_eq!(texts, vec!["abc", "def"]);
    }

    #[test]
    fn list_hanging_indent_matches_marker_width() {
        assert_eq!(list_hanging_indent("4. item"), 3);
        assert_eq!(list_hanging_indent("10. item"), 4);
        assert_eq!(list_hanging_indent("- item"), 2);
        assert_eq!(list_hanging_indent("plain text"), 0);
    }

    #[test]
    fn wrapped_list_continuation_starts_under_item_text() {
        let text = "4. one two three";
        let rows = layout_display_rows_with_hanging_indent(
            text,
            0,
            &[],
            12,
            true,
            list_hanging_indent(text),
        );
        let texts: Vec<String> = rows.iter().map(row_text).collect();
        assert_eq!(texts, vec!["4. one two", "   three"]);
        let hit_row = rows[1].hit_row(0, 0);
        assert_eq!(hit_row.cells.len(), 8, "indent cells must be hit-testable");
        assert!(
            hit_row
                .cells
                .iter()
                .take(3)
                .all(|hit| *hit == PopupTextHit::empty(rows[1].line_start))
        );
    }

    #[test]
    fn wrap_preserves_styles_across_soft_break() {
        let styles = vec![Style::default(); 18];
        let rows = layout_display_rows("If you are outside", 0, &styles, 14, true);
        assert_eq!(rows.len(), 2);
        assert_eq!(row_text(&rows[0]), "If you are");
        assert_eq!(row_text(&rows[1]), "outside");
    }
}
