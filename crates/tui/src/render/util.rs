use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
