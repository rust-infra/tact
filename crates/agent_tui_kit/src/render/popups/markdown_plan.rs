//! Shared Markdown display planning for scrollable popups.
//!
//! The thinking and subagent popups render completed content through the same
//! width-aware pipeline as the main area ([`crate::render::render_md::render_markdown_with_tables`])
//! and then share one decoration pass: GitHub-style `#` heading markers, a
//! blank row between adjacent ordered-list items, and a code-row tag so the
//! render loop can fill the row tail with the code background (a continuous
//! band instead of per-glyph patches).
//!
//! Main-area rendering is untouched: these helpers are popup-only.

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};

use crate::{
    render::selectable_text::{
        DisplayRow, MarkdownDisplayRow, layout_display_rows, scalar_styles, source_lines,
    },
    theme::Theme,
};

/// GitHub-style `#` marker recovered from the pulldown renderer's heading
/// styles. The main-area paths deliberately render headings without markers;
/// the popups add them so document structure reads at a glance.
pub fn heading_prefix(line: &Line<'_>, theme: &Theme) -> Option<&'static str> {
    let style = line.spans.first()?.style;
    let mods = style.add_modifier;
    let bold = mods.contains(Modifier::BOLD);
    let italic = mods.contains(Modifier::ITALIC);
    let underlined = mods.contains(Modifier::UNDERLINED);
    match (style.fg, bold, italic, underlined) {
        // H1: heading color, bold + underlined.
        (Some(c), true, _, true) if c == theme.heading => Some("# "),
        // H2: heading color, bold only.
        (Some(c), true, false, false) if c == theme.heading => Some("## "),
        // H3: heading color, bold + italic.
        (Some(c), true, true, false) if c == theme.heading => Some("### "),
        // H4: foreground color, bold + italic.
        (Some(c), true, true, false) if c == theme.fg => Some("#### "),
        // H5/H6: foreground color, italic (the renderer does not distinguish).
        (Some(c), false, true, false) if c == theme.fg => Some("##### "),
        _ => None,
    }
}

/// Prefix every heading line with its `#` marker (styled like the heading).
pub fn decorate_headings(lines: &mut [Line<'static>], theme: &Theme) {
    for line in lines.iter_mut() {
        if let Some(prefix) = heading_prefix(line, theme) {
            let style = line.spans.first().map(|s| s.style).unwrap_or_default();
            line.spans
                .insert(0, Span::styled(prefix.to_string(), style));
        }
    }
}

/// True when a rendered line is an ordered-list item (`1. `-style), used to
/// separate adjacent items with a blank row.
pub fn is_ordered_list_item(line: &Line<'_>) -> bool {
    let text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    let trimmed = text.trim_start();
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    digits > 0
        && trimmed
            .as_bytes()
            .get(digits)
            .is_some_and(|byte| *byte == b'.')
        && trimmed
            .as_bytes()
            .get(digits + 1)
            .is_some_and(u8::is_ascii_whitespace)
}

/// Flatten a rendered line back into plain text (the popup's copy/selection
/// text source).
pub fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// Wrap `display_text` into selectable rows, tagging code rows and inserting
/// spacer rows between adjacent ordered-list items.
///
/// `styled_lines` are the renderer's output lines (one per source line);
/// `fallback` styles source lines without a styled counterpart (blank lines),
/// and `code_bg` is the theme's code-block background used to detect fenced
/// code rows.
pub fn plan_markdown_display(
    styled_lines: &[Line<'_>],
    display_text: &str,
    fallback: Style,
    code_bg: Color,
    max_width: usize,
) -> Vec<MarkdownDisplayRow> {
    let mut rows: Vec<MarkdownDisplayRow> = Vec::new();
    for (index, source_line) in source_lines(display_text).iter().enumerate() {
        let styles = scalar_styles(
            styled_lines.get(index),
            fallback,
            source_line.text.chars().count(),
        );
        // A source line is a code row when any span carries the code-block
        // background (only fenced-code lines get one from the renderer).
        let is_code = styled_lines
            .get(index)
            .is_some_and(|line| line.spans.iter().any(|s| s.style.bg == Some(code_bg)));
        let line_rows = layout_display_rows(
            source_line.text,
            source_line.start,
            &styles,
            max_width,
            true,
        );
        rows.extend(line_rows.into_iter().map(|row| {
            if is_code {
                MarkdownDisplayRow::Code(row)
            } else {
                MarkdownDisplayRow::Content(row)
            }
        }));
        if styled_lines.get(index).is_some_and(is_ordered_list_item)
            && styled_lines
                .get(index + 1)
                .is_some_and(is_ordered_list_item)
        {
            rows.push(MarkdownDisplayRow::Spacer);
        }
    }
    rows
}

/// Fill the tail of a code row with the code background so the fence reads as
/// one continuous band. `cells.len()` is the used display width.
pub fn fill_code_row_tail(
    line: &mut Line<'static>,
    display: &DisplayRow,
    body_width: u16,
    code_bg: Color,
) {
    let fill = (body_width as usize).saturating_sub(display.cells.len());
    if fill > 0 {
        line.spans
            .push(Span::styled(" ".repeat(fill), Style::default().bg(code_bg)));
    }
}
