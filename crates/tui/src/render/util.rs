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

/// Byte offsets where `text` (one logical line, no `\n`) wraps into visual
/// lines of display width ≤ `max_width`. Greedy word wrap: break at the last
/// whitespace run that fits on the current line; hard-cut at `max_width` when
/// an unbroken word is longer than the line. Trailing whitespace rides along
/// on the line before the break (it is invisible) so segments stay contiguous
/// and every byte stays hit-testable.
///
/// The first offset is always 0 and each later offset starts a new visual
/// line; offsets sit on char boundaries. `wrap_line` and
/// `visual_pos_to_byte_offset` share this so rendering and mouse hit-testing
/// can never disagree about where a line breaks.
pub(crate) fn wrap_break_offsets(text: &str, max_width: usize) -> Vec<usize> {
    let mut starts = vec![0usize];
    if max_width == 0 {
        // Degenerate width: one char per visual line (matches the old
        // split_at_display_width fallback).
        let mut byte = 0;
        for c in text.chars() {
            byte += c.len_utf8();
            starts.push(byte);
        }
        starts.pop();
        return starts;
    }
    let mut remaining = text;
    while !remaining.is_empty() && UnicodeWidthStr::width(remaining) > max_width {
        // Preferred break: last whitespace char that fits; the rest of its
        // run is skipped so the next visual line starts on a word.
        let mut ws_at = None;
        let mut width = 0usize;
        let mut seen_non_ws = false;
        for (i, c) in remaining.char_indices() {
            let cw = UnicodeWidthChar::width(c).unwrap_or(0);
            if width + cw > max_width {
                break;
            }
            width += cw;
            if c.is_whitespace() {
                if seen_non_ws {
                    ws_at = Some(i);
                }
            } else {
                seen_non_ws = true;
            }
        }
        let cut = match ws_at {
            Some(i) => i,
            None => {
                // Unbroken run longer than the line: hard-cut at max_width,
                // or take the first char alone when it is wider than the line.
                let (seg, _) = split_at_display_width(remaining, max_width);
                if seg.is_empty() {
                    remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(0)
                } else {
                    seg.len()
                }
            }
        };
        remaining = remaining[cut..].trim_start();
        starts.push(text.len() - remaining.len());
    }
    starts
}

/// Clip one wrapped visual segment against the original styled spans so
/// per-span styles (including the REVERSED selection overlay) survive onto
/// every continuation line.
fn styled_segment_line(
    seg: &str,
    abs_start: usize,
    span_ranges: &[(usize, usize, Style)],
    base_style: Style,
) -> Line<'static> {
    let abs_end = abs_start + seg.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    for &(s, e, style) in span_ranges {
        let lo = s.max(abs_start);
        let hi = e.min(abs_end);
        if lo < hi {
            spans.push(Span::styled(
                seg[lo - abs_start..hi - abs_start].to_string(),
                style,
            ));
        }
    }
    if spans.is_empty() {
        spans.push(Span::styled(String::new(), base_style));
    }
    Line::from(spans)
}

/// Split a styled Line by display width into multiple Lines not exceeding max_width.
pub(crate) fn wrap_line(line: &Line<'_>, max_width: usize) -> Vec<Line<'static>> {
    let line_style = line.style;
    let text: String = line
        .spans
        .iter()
        .map(|s| s.content.as_ref())
        .collect::<Vec<_>>()
        .concat();
    let base_style = line_style.patch(line.spans.first().map(|s| s.style).unwrap_or_default());

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

    // Source span byte ranges within `text`, so wrapped lines re-slice the
    // original styled spans instead of flattening everything to one style.
    let mut span_ranges: Vec<(usize, usize, Style)> = Vec::with_capacity(line.spans.len());
    let mut cursor = 0usize;
    for span in &line.spans {
        let end = cursor + span.content.len();
        span_ranges.push((cursor, end, line_style.patch(span.style)));
        cursor = end;
    }

    let mut result = Vec::new();
    let mut line_start = 0usize;
    for text_line in text.split('\n') {
        let seg_starts = wrap_break_offsets(text_line, max_width);
        for (k, &seg_start) in seg_starts.iter().enumerate() {
            let seg_end = seg_starts.get(k + 1).copied().unwrap_or(text_line.len());
            let abs_start = line_start + seg_start;
            let abs_end = line_start + seg_end;
            result.push(styled_segment_line(
                &text[abs_start..abs_end],
                abs_start,
                &span_ranges,
                base_style,
            ));
        }
        line_start += text_line.len() + 1; // skip the '\n' itself
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
///
/// Uses the same break points as `wrap_line` (`wrap_break_offsets`), so a
/// click on any rendered cell maps back to the byte it visually covers.
pub(crate) fn visual_pos_to_byte_offset(
    raw_text: &str,
    wrap_width: usize,
    target_line: usize,
    target_col: usize,
) -> usize {
    let logical_lines: Vec<&str> = raw_text.split('\n').collect();
    let mut visual = 0usize;
    let mut abs = 0usize;
    for logical in &logical_lines {
        let starts = wrap_break_offsets(logical, wrap_width);
        for (k, &seg_start) in starts.iter().enumerate() {
            let seg_end = starts.get(k + 1).copied().unwrap_or(logical.len());
            if visual == target_line {
                let seg = &logical[seg_start..seg_end];
                return abs + seg_start + col_to_byte_offset(seg, target_col);
            }
            visual += 1;
        }
        // The '\n' itself ends the last visual line of this logical line (it
        // is already reachable as "one column past" that line's last segment).
        abs += logical.len() + 1;
        if visual > target_line {
            return raw_text.len();
        }
    }
    raw_text.len()
}

/// Byte index of the first char in `seg` whose display columns cover
/// `target_col`; `seg.len()` when the column is past the segment.
fn col_to_byte_offset(seg: &str, target_col: usize) -> usize {
    let mut col = 0usize;
    for (i, ch) in seg.char_indices() {
        let width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if col + width > target_col {
            return i;
        }
        col += width;
    }
    seg.len()
}

#[cfg(test)]
mod wrap_tests {
    use ratatui::style::{Color, Modifier, Style};

    use super::*;

    #[test]
    fn visual_pos_to_byte_offset_basic() {
        assert_eq!(visual_pos_to_byte_offset("hello", 10, 0, 2), 2);
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 0, 0), 0);
        // Word wrap skips the space: visual line 1 starts at 'w' (byte 6).
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 0, 4), 4);
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 1, 0), 6);
        assert_eq!(visual_pos_to_byte_offset("hello world", 5, 1, 1), 7);
        assert_eq!(visual_pos_to_byte_offset("hello", 10, 0, 100), 5);
    }

    #[test]
    fn wrap_break_offsets_prefers_word_boundaries() {
        assert_eq!(wrap_break_offsets("hello", 10), vec![0]);
        assert_eq!(wrap_break_offsets("hello world", 10), vec![0, 6]);
        // CJK breaks anywhere: no spaces needed.
        assert_eq!(wrap_break_offsets("你好世界", 4), vec![0, 6]);
        // Long unbroken word hard-cuts at max_width.
        assert_eq!(wrap_break_offsets("abcdefgh", 3), vec![0, 3, 6]);
        // Break sits at the last fitting whitespace, run skipped.
        assert_eq!(wrap_break_offsets("ab  cd", 4), vec![0, 4]);
        // Leading whitespace is not a break candidate.
        assert_eq!(wrap_break_offsets("   abc", 5), vec![0, 5]);
        assert_eq!(wrap_break_offsets("", 10), vec![0]);
    }

    #[test]
    fn wrap_line_keeps_word_intact_and_preserves_span_styles() {
        let line = Line::from(vec![
            Span::styled("hello ", Style::default().fg(Color::Red)),
            Span::styled(
                "world",
                Style::default().fg(Color::Red).add_modifier(Modifier::REVERSED),
            ),
        ]);
        let wrapped = wrap_line(&line, 5);
        // "hello " / "world": no mid-word split.
        assert_eq!(wrapped.len(), 2);
        let joined: String = wrapped
            .iter()
            .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
            .collect();
        assert_eq!(joined, "hello world");
        // The REVERSED style follows its span onto the second line.
        let second = &wrapped[1];
        assert!(
            second
                .spans
                .iter()
                .any(|s| s.content.as_ref() == "world"
                    && s.style.add_modifier.contains(Modifier::REVERSED)),
            "selection overlay must survive wrapping: {second:?}"
        );
    }

    #[test]
    fn wrap_break_offsets_agree_with_byte_offset_hit_testing() {
        // Every rendered visual line must start exactly where the hit-test
        // maps (line, col 0) back to.
        let text = "the quick brown fox jumps over the lazy dog";
        let starts = wrap_break_offsets(text, 10);
        for (line, &start) in starts.iter().enumerate() {
            assert_eq!(visual_pos_to_byte_offset(text, 10, line, 0), start);
        }
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
