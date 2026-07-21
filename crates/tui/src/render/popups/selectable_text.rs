use crate::widgets::state::{PopupHitRow, PopupTextHit};
use ratatui::{
    buffer::CellWidth,
    style::{Modifier, Style},
    text::{Line, Span},
};
use unicode_segmentation::UnicodeSegmentation;

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

#[derive(Debug)]
pub(crate) struct DisplayRow {
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    graphemes: Vec<DisplayGrapheme>,
    pub(crate) cells: Vec<PopupTextHit>,
}

fn hit_intersects(hit: PopupTextHit, selection: &std::ops::Range<usize>) -> bool {
    hit.start < selection.end && hit.end > selection.start
}

impl DisplayRow {
    pub(crate) fn hit_row(&self, screen_y: u16, text_x: u16) -> PopupHitRow {
        PopupHitRow {
            screen_y,
            text_x,
            line_start: self.line_start,
            line_end: self.line_end,
            cells: self.cells.clone(),
        }
    }

    pub(crate) fn spans(&self, selection: Option<&std::ops::Range<usize>>) -> Vec<Span<'static>> {
        let mut spans = Vec::new();
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

pub(crate) fn layout_display_rows(
    text: &str,
    line_start: usize,
    styles: &[Style],
    max_width: usize,
    wrap: bool,
) -> Vec<DisplayRow> {
    let mut rows = Vec::new();
    let mut graphemes = Vec::new();
    let mut cells = Vec::new();
    let mut row_start = line_start;
    let mut row_end = line_start;
    let mut row_width = 0;

    let push_row = |rows: &mut Vec<DisplayRow>,
                    graphemes: &mut Vec<DisplayGrapheme>,
                    cells: &mut Vec<PopupTextHit>,
                    line_start,
                    line_end| {
        rows.push(DisplayRow {
            line_start,
            line_end,
            graphemes: std::mem::take(graphemes),
            cells: std::mem::take(cells),
        });
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
                push_row(&mut rows, &mut graphemes, &mut cells, row_start, row_end);
                row_start = start;
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
        push_row(&mut rows, &mut graphemes, &mut cells, row_start, row_end);
    }
    rows
}
