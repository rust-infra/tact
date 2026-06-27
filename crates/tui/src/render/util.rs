use ratatui::layout::Rect;
use ratatui::text::{Line, Span};
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
fn split_at_display_width(text: &str, max_width: usize) -> (&str, &str) {
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

/// Split a styled Line by display width into multiple Lines not exceeding max_width.
/// Child lines inherit the first span's style; dominant style preserved for multi-span lines.
pub(crate) fn wrap_line(line: &Line<'_>, max_width: usize) -> Vec<Line<'static>> {
    let text: String = line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .concat();
    let base_style = line.spans.first().map(|s| s.style).unwrap_or_default();

    let mut result = Vec::new();
    for text_line in text.lines() {
        if text_line.is_empty() {
            result.push(Line::from(Span::styled("", base_style)));
            continue;
        }
        let w = UnicodeWidthStr::width(text_line);
        if w <= max_width {
            result.push(Line::from(Span::styled(text_line.to_string(), base_style)));
            continue;
        }
        let mut remaining = text_line;
        while !remaining.is_empty() {
            let (seg, rest) = split_at_display_width(remaining, max_width);
            if seg.is_empty() {
                if let Some(c) = rest.chars().next() {
                    let mut s = String::new();
                    s.push(c);
                    result.push(Line::from(Span::styled(s, base_style)));
                    remaining = &rest[c.len_utf8()..];
                } else {
                    break;
                }
            } else {
                result.push(Line::from(Span::styled(seg.to_string(), base_style)));
                remaining = rest;
            }
        }
    }
    if result.is_empty() {
        result.push(Line::from(Span::styled("", base_style)));
    }
    result
}
