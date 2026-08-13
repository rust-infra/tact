use ratatui::{
    layout::Rect,
    style::Style,
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

/// Left gutter for thinking blocks inside the log panel.
pub(crate) const LOG_THINKING_INDENT: u16 = 2;
/// Left gutter for tool invocations inside the log panel.
pub(crate) const LOG_TOOL_INDENT: u16 = 4;
/// Extra indent for rendered tool blocks (title + meta + detail card).
pub(crate) const LOG_TOOL_BLOCK_INDENT: u16 = LOG_TOOL_INDENT + 4;

/// Truncate by Unicode scalar count, appending `…` when over `max_chars`.
pub(crate) fn truncate_chars_with_ellipsis(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        let keep = max_chars.saturating_sub(3).max(1);
        format!("{}…", s.chars().take(keep).collect::<String>())
    }
}

pub(crate) fn indent_rect(area: Rect, cols: u16) -> Rect {
    if cols == 0 {
        return area;
    }
    let x = area.x.saturating_add(cols);
    let width = area.width.saturating_sub(cols);
    Rect::new(x, area.y, width, area.height)
}

/// Split a single line of text at the specified display width, returning (prefix, remainder).
/// The prefix display width ≤ max_width.
pub(crate) fn split_at_display_width(text: &str, max_width: usize) -> (&str, &str) {
    if text.is_empty() || max_width == 0 {
        return ("", text);
    }
    let mut current_width = 0;
    for (i, c) in text.char_indices() {
        let cw = UnicodeWidthChar::width(c).unwrap_or(0);
        if current_width + cw > max_width {
            return (&text[..i], &text[i..]);
        }
        current_width += cw;
    }
    (text, "")
}

/// Width of the visible prefix before the content of a Markdown list item.
///
/// The returned value includes leading indentation, the marker (`-`, `*`, `+`,
/// `•`, or an ordered marker such as `12.`), and the whitespace after it.
/// Continuation rows can use this as a hanging indent so their text starts
/// below the item text rather than at the panel edge.
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

    UnicodeWidthStr::width(&text[..leading + marker_end + whitespace_len])
}

/// Split a styled Line by display width into multiple Lines not exceeding
/// `max_width`.
pub(crate) fn wrap_line(line: &Line<'_>, max_width: usize) -> Vec<Line<'static>> {
    wrap_line_with_hanging_indent(line, max_width, 0)
}

/// Split a styled line while reserving a hanging indent for continuation rows.
///
/// The first visual row keeps the full width for the list marker and item text.
/// Later rows start with `hanging_indent` spaces and use the remaining width for
/// content. The indent is clamped so even a very narrow terminal cannot emit a
/// row wider than the requested width.
pub(crate) fn wrap_line_with_hanging_indent(
    line: &Line<'_>,
    max_width: usize,
    hanging_indent: usize,
) -> Vec<Line<'static>> {
    let line_style = line.style;
    let text: String = line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .concat();
    let base_style = line_style.patch(line.spans.first().map(|s| s.style).unwrap_or_default());
    let hanging_indent = hanging_indent.min(max_width.saturating_sub(1));
    let continuation_width = max_width.saturating_sub(hanging_indent).max(1);

    if !text.contains('\n') && UnicodeWidthStr::width(text.as_str()) <= max_width {
        let spans: Vec<Span<'static>> = line
            .spans
            .iter()
            .map(|span| {
                Span::styled(
                    span.content.clone().into_owned(),
                    line_style.patch(span.style),
                )
            })
            .collect();
        if !spans.is_empty() {
            return vec![Line {
                style: Style::default(),
                alignment: line.alignment,
                spans,
            }];
        }
    }

    let mut result = Vec::new();
    for text_line in text.lines() {
        if text_line.is_empty() {
            result.push(Line::from(Span::styled("", base_style)));
            continue;
        }

        let mut remaining = text_line;
        let mut first_row = true;
        while !remaining.is_empty() {
            let width = if first_row {
                max_width
            } else {
                continuation_width
            };
            let (seg, rest) = split_at_display_width(remaining, width);
            let segment = if seg.is_empty() {
                if let Some(c) = rest.chars().next() {
                    &rest[..c.len_utf8()]
                } else {
                    break;
                }
            } else {
                seg
            };

            let mut spans = Vec::new();
            if !first_row && hanging_indent > 0 {
                spans.push(Span::styled(" ".repeat(hanging_indent), base_style));
            }
            spans.push(Span::styled(segment.to_string(), base_style));
            result.push(Line::from(spans));

            remaining = &remaining[segment.len()..];
            first_row = false;
        }
    }
    if result.is_empty() {
        result.push(Line::from(Span::styled("", base_style)));
    }
    result
}

/// Convert a visual position within a raw text line to a byte offset.
///
/// `target_line`: 0-based visual line within the logical row (accounting for wrapping).
/// `target_col`: 0-based display column after accounting for prefix/indent.
/// Returns the byte index of the character whose display column covers `target_col`.
/// If the position is past the end of the text, returns `raw_text.len()`.
pub(crate) fn visual_pos_to_byte_offset(
    raw_text: &str,
    wrap_width: usize,
    target_line: usize,
    target_col: usize,
) -> usize {
    let mut line = 0usize;
    let mut col = 0usize;
    for (idx, ch) in raw_text.char_indices() {
        if ch == '\n' {
            if line == target_line && target_col >= col {
                return idx;
            }
            line += 1;
            col = 0;
            if line > target_line {
                return idx;
            }
            continue;
        }
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if wrap_width > 0 && col + width > wrap_width {
            if line == target_line && target_col >= col {
                return idx;
            }
            line += 1;
            col = 0;
            if line > target_line {
                return idx;
            }
            if line == target_line && width > target_col {
                return idx;
            }
        }
        if line == target_line && col + width > target_col {
            return idx;
        }
        col += width;
    }
    raw_text.len()
}

#[cfg(test)]
mod wrap_tests {
    use ratatui::style::{Color, Modifier, Style};

    use super::*;

    #[test]
    fn visual_pos_to_byte_offset_basic() {
        assert_eq!(visual_pos_to_byte_offset("hello", 10, 0, 2), 2);
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 0, 0), 0);
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 1, 0), 5);
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 1, 1), 6);
        assert_eq!(visual_pos_to_byte_offset("hello", 10, 0, 100), 5);
    }

    #[test]
    fn visual_pos_to_byte_offset_multibyte() {
        let text = "こんにちは"; // each char is width 2 and 3 bytes
        assert_eq!(visual_pos_to_byte_offset(text, 10, 0, 0), 0);
        assert_eq!(visual_pos_to_byte_offset(text, 10, 0, 1), 0); // middle of first char
        assert_eq!(visual_pos_to_byte_offset(text, 10, 0, 2), 3); // start of second char
    }

    #[test]
    fn visual_pos_to_byte_offset_newline() {
        assert_eq!(visual_pos_to_byte_offset("ab\ncd", 10, 0, 1), 1);
        assert_eq!(visual_pos_to_byte_offset("ab\ncd", 10, 1, 0), 3);
        assert_eq!(visual_pos_to_byte_offset("ab\ncd", 10, 1, 1), 4);
    }

    #[test]
    fn line_style_after_wrap() {
        let line = Line::from(vec![
            Span::styled("### ", Style::default()),
            Span::styled("Heading", Style::default()),
        ])
        .style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        );
        let wrapped = wrap_line(&line, 80);
        assert_eq!(wrapped.len(), 1);
        assert_eq!(wrapped[0].spans[0].style.fg, Some(Color::Cyan));
        assert!(
            wrapped[0].spans[0]
                .style
                .add_modifier
                .contains(Modifier::BOLD)
        );
    }
}
