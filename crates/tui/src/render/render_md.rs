use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use ratatui_markdown::{
    markdown::MarkdownRenderer,
    mermaid::{render_mermaid, theme::MermaidTheme},
    theme::{CodeColors, Generation, RichTextTheme},
};
use unicode_width::UnicodeWidthStr;

use crate::{render::util::split_at_display_width, theme::Theme};

/// Maps the app [`Theme`] into ratatui-markdown's `RichTextTheme` for the
/// Mermaid renderer.
#[derive(Clone, Copy)]
struct TuiRichTextTheme<'a> {
    theme: &'a Theme,
}

impl RichTextTheme for TuiRichTextTheme<'_> {
    fn generation(&self) -> Generation {
        Generation(0)
    }

    fn get_text_color(&self) -> Color {
        self.theme.fg
    }

    fn get_muted_text_color(&self) -> Color {
        self.theme.muted_fg()
    }

    fn get_primary_color(&self) -> Color {
        self.theme.accent
    }

    fn get_popup_selected_background(&self) -> Color {
        self.theme.highlight
    }

    fn get_border_color(&self) -> Color {
        self.theme.border
    }

    fn get_focused_border_color(&self) -> Color {
        self.theme.accent
    }

    fn get_secondary_color(&self) -> Color {
        self.theme.success
    }

    fn get_info_color(&self) -> Color {
        self.theme.heading
    }

    fn get_json_key_color(&self) -> Color {
        self.theme.heading
    }

    fn get_json_string_color(&self) -> Color {
        self.theme.success
    }

    fn get_json_number_color(&self) -> Color {
        self.theme.accent
    }

    fn get_json_bool_color(&self) -> Color {
        self.theme.warning
    }

    fn get_json_null_color(&self) -> Color {
        self.theme.muted
    }

    fn get_accent_yellow(&self) -> Color {
        self.theme.warning
    }

    fn get_code_colors(&self) -> CodeColors {
        CodeColors::default()
    }

    fn get_mermaid_theme(&self) -> MermaidTheme {
        MermaidTheme::for_background(self.theme.bg)
    }
}

/// Render a Mermaid diagram through ratatui-markdown with the app theme.
///
/// Sequence diagrams are handled by Tact's own renderer (see
/// [`mermaid_sequence`]) because the upstream sequence renderer mishandles
/// `participant X as 名称` aliases, the `+`/`-` activation shorthand, and
/// CJK arrow labels.
///
/// Returns `None` when the source cannot be parsed or rendered so callers can
/// fall back to ordinary code rendering.
pub(crate) fn render_mermaid_block(
    source: &str,
    theme: &Theme,
    width: usize,
) -> Option<Vec<Line<'static>>> {
    let width = width.max(1);
    if source
        .lines()
        .next()
        .map(str::trim)
        .is_some_and(|l| l.starts_with("sequenceDiagram"))
    {
        return super::mermaid_sequence::render_sequence_diagram(source, width, theme);
    }
    render_mermaid(source, width, None, &TuiRichTextTheme { theme })
}

/// Layout width used when a caller has no explicit width to pass.
///
/// `render_markdown_tui` has no width parameter; Mermaid fences inside it are
/// laid out at a nominal 80 columns.
const DEFAULT_RENDER_WIDTH: usize = 80;

/// Renderer for one non-Mermaid chunk of Markdown (prose, tables, code fences).
type SegmentRenderer = fn(&str, &Theme, Option<usize>) -> (Vec<Line<'static>>, Vec<String>);

/// Route complete, top-level `mermaid` fences out of `text`.
///
/// A closed ` ```mermaid ` fence is rendered by `render_mermaid_block` at
/// `width` (bounded to [`DEFAULT_RENDER_WIDTH`] when no width is given);
/// every other chunk (prose, pipe tables, non-Mermaid code fences) is
/// delegated to `render_segment` with the caller's width passed through
/// unchanged — `None` keeps meaning "unlimited" for table layout. Only
/// *top-level* Mermaid fences are routed: while scanning, non-Mermaid fenced
/// blocks ( ` ```rust `, ` ```text `, bare ` ``` `, `~~~text`, ` ````text `,
/// …) are tracked with their opener marker (`` ` `` or `~`) and run length so
/// a literal ` ```mermaid ` line inside ordinary code is passed through
/// unchanged until a matching closing fence — it is code content, never a
/// diagram. A fence closes only on a line of the same marker with at least
/// the opener's run length and a whitespace-only suffix, so an inner ` ``` `
/// inside a ` ````text ` block is content, not the close. When a Mermaid
/// fence cannot be rendered (or is never closed) the original fence text goes
/// through the existing code renderer instead, so no source is ever dropped.
/// Returns styled lines plus their raw text, keeping the same shape as the
/// existing renderers.
fn route_mermaid_fences(
    text: &str,
    theme: &Theme,
    width: Option<usize>,
    render_segment: SegmentRenderer,
) -> (Vec<Line<'static>>, Vec<String>) {
    let mut styled_lines = Vec::new();
    let mut raw_lines = Vec::new();
    let mut chunk = String::new();

    // Mermaid layout needs a concrete width; tables/prose keep the caller's
    // optional width untouched (None = unlimited).
    let mermaid_width = width.unwrap_or(DEFAULT_RENDER_WIDTH).max(1);

    let mut lines = text.lines();
    let mut fence: Option<Fence> = None;
    while let Some(line) = lines.next() {
        if let Some(opener) = fence {
            // Inside an ordinary fenced block: pass everything through until a
            // matching closing fence (same marker, at least the opener's run
            // length, whitespace-only suffix), so a literal ```mermaid inside
            // e.g. ~~~text or ````text is never mistaken for a top-level
            // diagram.
            chunk.push_str(line);
            chunk.push('\n');
            if is_fence_closer(line, opener) {
                fence = None;
            }
            continue;
        }

        if mermaid_fence_opener(line).is_some() {
            // Collect source lines until a trimmed closing fence.
            let mut source = String::new();
            let mut closed = false;
            for src in lines.by_ref() {
                if src.trim() == "```" {
                    closed = true;
                    break;
                }
                source.push_str(src);
                source.push('\n');
            }

            // Flush preceding prose so the diagram keeps its position in the doc.
            flush_segment(
                &mut styled_lines,
                &mut raw_lines,
                &mut chunk,
                theme,
                width,
                render_segment,
            );

            if !closed {
                // Unclosed fence: keep it as ordinary code content, never drop it.
                let block = format!("```mermaid\n{source}");
                let (s, r) = render_plain_markdown(&block, theme, width);
                styled_lines.extend(s);
                raw_lines.extend(r);
                continue;
            }

            match render_mermaid_block(&source, theme, mermaid_width) {
                Some(rendered) => {
                    for line in rendered {
                        let raw = line.to_string();
                        raw_lines.push(raw);
                        styled_lines.push(line);
                    }
                }
                None => {
                    // Invalid diagram: send the original fence through the code renderer.
                    let block = format!("```mermaid\n{source}```");
                    let (s, r) = render_plain_markdown(&block, theme, width);
                    styled_lines.extend(s);
                    raw_lines.extend(r);
                }
            }
            continue;
        }

        if let Some(opener) = parse_fence_opener(line) {
            // Ordinary (non-Mermaid) fence opener — backtick or tilde, run
            // length ≥ 3: track the block so any Mermaid-looking line inside
            // it stays literal code content.
            fence = Some(opener);
            chunk.push_str(line);
            chunk.push('\n');
            continue;
        }

        chunk.push_str(line);
        chunk.push('\n');
    }

    flush_segment(
        &mut styled_lines,
        &mut raw_lines,
        &mut chunk,
        theme,
        width,
        render_segment,
    );
    (styled_lines, raw_lines)
}

/// Render one pending non-Mermaid chunk through the segment renderer.
fn flush_segment(
    styled_lines: &mut Vec<Line<'static>>,
    raw_lines: &mut Vec<String>,
    chunk: &mut String,
    theme: &Theme,
    width: Option<usize>,
    render_segment: SegmentRenderer,
) {
    if !chunk.trim().is_empty() {
        let (s, r) = render_segment(chunk, theme, width);
        styled_lines.extend(s);
        raw_lines.extend(r);
        chunk.clear();
    }
}

/// Detect a top-level `mermaid` fence opener.
///
/// A line whose trimmed form starts with three backticks opens a fence; the
/// info string after the backticks must be `mermaid` (case-insensitive).
fn mermaid_fence_opener(line: &str) -> Option<()> {
    let trimmed = line.trim();
    let info = trimmed.strip_prefix("```")?;
    let lang = info.split_whitespace().next().unwrap_or("");
    lang.eq_ignore_ascii_case("mermaid").then_some(())
}

/// A Markdown fenced-block descriptor: the fence marker character (`` ` `` or
/// `~`) and the length of its opening run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fence {
    marker: char,
    run_len: usize,
}

/// Minimum fence run length required by CommonMark (``` / ~~~).
const MIN_FENCE_RUN: usize = 3;

/// Parse a valid CommonMark-style fence opener.
///
/// A line whose trimmed form starts with at least three backticks or tildes
/// opens a fence; the remainder is the info string. Backtick fences reject an
/// info string containing a backtick (CommonMark); tilde fences accept any
/// info string.
fn parse_fence_opener(line: &str) -> Option<Fence> {
    let trimmed = line.trim();
    let marker = trimmed.chars().next()?;
    if marker != '`' && marker != '~' {
        return None;
    }
    let run_len = trimmed.chars().take_while(|&c| c == marker).count();
    if run_len < MIN_FENCE_RUN {
        return None;
    }
    if marker == '`' && trimmed[run_len..].contains('`') {
        return None;
    }
    Some(Fence { marker, run_len })
}

/// Recognize a closing fence for `opener`.
///
/// The closing line must start (after trimming) with the same marker, have a
/// run at least as long as the opener's, and contain only whitespace after the
/// run — a shorter run, a different marker, or any non-whitespace suffix is
/// content, not a close. (Marker runs are ASCII, so byte indices equal char
/// indices.)
fn is_fence_closer(line: &str, opener: Fence) -> bool {
    let trimmed = line.trim();
    let run_len = trimmed.chars().take_while(|&c| c == opener.marker).count();
    run_len >= opener.run_len && trimmed[run_len..].chars().all(char::is_whitespace)
}

/// Renders Markdown text into ratatui `Line`s and raw text via pulldown-cmark.
///
/// Complete top-level ` ```mermaid ` fences are rendered as terminal diagrams
/// at a nominal [`DEFAULT_RENDER_WIDTH`]; all other content keeps the plain
/// pulldown-cmark pipeline.
pub(crate) fn render_markdown_tui(text: &str, theme: &Theme) -> (Vec<Line<'static>>, Vec<String>) {
    route_mermaid_fences(
        text,
        theme,
        Some(DEFAULT_RENDER_WIDTH),
        render_plain_markdown,
    )
}

/// Direct chunk renderer (no Mermaid routing, no width-aware tables).
///
/// Fallback code-card previews go through this path directly so a
/// reconstructed fallback fence is never re-routed through the Mermaid
/// renderer. Tables render at unlimited width: the log panel wraps lines
/// itself via `wrap_line`.
pub(crate) fn render_plain_markdown(
    text: &str,
    theme: &Theme,
    _width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    render_prose_and_tables(text, theme, None)
}

/// Render markdown with width-aware pipe tables.
///
/// Complete top-level ` ```mermaid ` fences are routed to the Mermaid
/// renderer before the chunk renderer runs, so `|` lines inside a diagram are
/// never mistaken for table rows. Everything else (prose, headings, lists,
/// tables, code fences, blockquotes) goes through [`super::pulldown`] with the
/// caller's width forwarded to the pipe-table layout.
pub(crate) fn render_markdown_with_tables(
    text: &str,
    theme: &Theme,
    available_width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    route_mermaid_fences(text, theme, available_width, render_prose_and_tables)
}

/// The width-aware chunk renderer (pulldown-cmark + the `▎` gutter pass).
fn render_prose_and_tables(
    text: &str,
    theme: &Theme,
    available_width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    let (mut styled_lines, raw_lines) =
        super::pulldown::render_markdown(text, theme, available_width);
    apply_blockquote_indicator(&mut styled_lines, theme);
    (styled_lines, raw_lines)
}

/// Render Markdown through ratatui-markdown's own renderer.
///
/// Used by popups (e.g. the `/stats` session-stats popup) where a quick
/// default layout is enough — no width-aware pipe-table pass, no Mermaid
/// routing. `width` bounds the renderer's max line width.
pub(crate) fn render_markdown_ratatui(
    text: &str,
    theme: &Theme,
    width: usize,
) -> Vec<Line<'static>> {
    let renderer = MarkdownRenderer::new(width.max(1));
    let blocks = renderer.parse(text);
    renderer.render(&blocks, &TuiRichTextTheme { theme })
}

/// Adapt the pulldown renderer's blockquotes to the log's `▎` gutter look.
///
/// The pulldown renderer emits a muted `│ ` gutter per level; the log's
/// established look is a success-colored `▎ ` gutter with the quote text in
/// the same color. Gutter-only spans are dropped, nested `│ ` runs collapse
/// into one `▎ `, and every remaining span is recolored to `theme.success`
/// while keeping its modifiers.
fn apply_blockquote_indicator(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    let gutter_fg = theme.muted_fg();
    for line in lines.iter_mut() {
        let is_quote = line
            .spans
            .first()
            .is_some_and(|s| s.content.starts_with('│') && s.style.fg == Some(gutter_fg));
        if !is_quote {
            continue;
        }
        let mut out: Vec<Span<'static>> = Vec::new();
        let mut gutter_open = true;
        for sp in std::mem::take(&mut line.spans) {
            if gutter_open && sp.style.fg == Some(gutter_fg) {
                let trimmed = sp.content.trim_start_matches(['│', ' ']);
                if trimmed.is_empty() {
                    // Pure gutter span (e.g. "│ "): drop it.
                    continue;
                }
                gutter_open = false;
                out.push(Span::styled("▎ ", Style::default().fg(theme.success)));
                out.push(Span::styled(
                    trimmed.to_string(),
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(sp.style.add_modifier),
                ));
            } else {
                if gutter_open {
                    gutter_open = false;
                    out.push(Span::styled("▎ ", Style::default().fg(theme.success)));
                }
                out.push(Span::styled(
                    sp.content.to_string(),
                    Style::default()
                        .fg(theme.success)
                        .add_modifier(sp.style.add_modifier),
                ));
            }
        }
        if out.is_empty() {
            out.push(Span::styled("▎ ", Style::default().fg(theme.success)));
        }
        line.spans = out;
    }
}

/// Checks whether a line is a Markdown horizontal rule (---, ***, ___, spaces allowed).
pub(crate) fn is_horizontal_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.len() < 3 {
        return false;
    }
    let marks: Vec<char> = trimmed.chars().filter(|c| !c.is_whitespace()).collect();
    if marks.len() < 3 {
        return false;
    }
    let first = marks[0];
    if first != '-' && first != '*' && first != '_' {
        return false;
    }
    marks.iter().all(|&c| c == first)
}

/// Display width of a cell (CJK counts as 2 columns in terminal).
fn cell_display_width(cell: &str) -> usize {
    UnicodeWidthStr::width(cell)
}

/// Pad a cell to `width` terminal columns (left content, right spaces).
fn pad_cell(cell: &str, width: usize) -> String {
    let pad = width.saturating_sub(cell_display_width(cell));
    format!(" {cell}{:pad$} ", "", pad = pad)
}

/// Parses Markdown table raw lines into column-aligned ratatui Lines.
///
/// Column widths use Unicode display width so CJK headers/cells align with ASCII.
/// When `available_width` is given, columns are shrunk to fit and long cells are
/// wrapped *inside* the table layout (every wrapped sub-row keeps its pipes and
/// padding), so the log panel's line wrapper never breaks a row and misaligns
/// the columns.
pub(crate) fn format_table(
    lines: &[String],
    theme: &Theme,
    available_width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    let rows: Vec<Vec<String>> = lines
        .iter()
        .map(|line| {
            let mut cells: Vec<String> = line.split('|').map(|s| s.trim().to_string()).collect();
            if cells.first().map(|s| s.is_empty()).unwrap_or(false) {
                cells.remove(0);
            }
            if cells.last().map(|s| s.is_empty()).unwrap_or(false) {
                cells.pop();
            }
            cells
        })
        .collect();

    if rows.is_empty() {
        return (Vec::new(), Vec::new());
    }

    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let mut col_widths = vec![0; col_count];
    for row in &rows {
        // Skip separator rows when measuring — dashes shouldn't widen columns.
        let is_sep = row.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        });
        if is_sep {
            continue;
        }
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(cell_display_width(cell));
            }
        }
    }

    // Shrink the widest columns until the whole table (padding + pipes) fits
    // the available width — a single long cell must not blow up every column.
    if let Some(avail) = available_width {
        fit_columns_to_width(&mut col_widths, avail);
    }

    let mut styled_lines = Vec::new();
    let mut raw_lines = Vec::new();

    for (row_idx, row) in rows.iter().enumerate() {
        let is_sep = row.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        });

        if is_sep {
            // Render a visual separator that matches column widths.
            let sep_cells: Vec<String> = (0..col_count)
                .map(|i| {
                    let w = col_widths.get(i).copied().unwrap_or(0).max(1);
                    format!(" {} ", "-".repeat(w))
                })
                .collect();
            let line_text = format!("|{}|", sep_cells.join("|"));
            styled_lines.push(Line::from(Span::styled(
                line_text.clone(),
                Style::default().fg(theme.accent),
            )));
            raw_lines.push(line_text);
            continue;
        }

        // Wrap each cell to its column width; a row becomes several visual
        // sub-rows, each still padded and pipe-separated so columns stay
        // aligned across the wrap.
        let wrapped: Vec<Vec<String>> = (0..col_count)
            .map(|i| {
                let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
                let width = col_widths.get(i).copied().unwrap_or(0);
                wrap_cell(cell, width)
            })
            .collect();
        let sub_rows = wrapped.iter().map(Vec::len).max().unwrap_or(1);

        for sub in 0..sub_rows {
            let mut cells = Vec::new();
            for (i, segs) in wrapped.iter().enumerate() {
                let seg = segs.get(sub).map(|s| s.as_str()).unwrap_or("");
                let width = col_widths.get(i).copied().unwrap_or(0);
                cells.push(pad_cell(seg, width));
            }
            let line_text = format!("|{}|", cells.join("|"));

            let styled = if row_idx == 0 && sub == 0 {
                // Header: bold accent cells, dim pipes — keeps `#` / titles visually distinct.
                styled_table_row(&cells, theme.accent, true, theme)
            } else {
                Line::from(Span::styled(
                    line_text.clone(),
                    Style::default().fg(theme.fg),
                ))
            };

            styled_lines.push(styled);
            raw_lines.push(line_text);
        }
    }

    (styled_lines, raw_lines)
}

/// Minimum content width a column keeps when shrinking to fit.
const MIN_COL_WIDTH: usize = 8;

/// Shrink the widest columns until the rendered table fits `available_width`.
///
/// A rendered row costs `sum(widths) + 3 * col_count + 1` display columns:
/// two padding spaces per cell plus one pipe per column and a leading pipe.
fn fit_columns_to_width(col_widths: &mut [usize], available_width: usize) {
    let row_width = |widths: &[usize]| widths.iter().sum::<usize>() + 3 * widths.len() + 1;
    let mut overflow = row_width(col_widths).saturating_sub(available_width);
    while overflow > 0 {
        // Widest column still above the floor.
        let mut target = None;
        let mut widest = MIN_COL_WIDTH;
        for (i, &w) in col_widths.iter().enumerate() {
            if w > widest {
                widest = w;
                target = Some(i);
            }
        }
        let Some(idx) = target else { break };
        let reduce = (col_widths[idx] - MIN_COL_WIDTH).min(overflow);
        col_widths[idx] -= reduce;
        overflow = overflow.saturating_sub(reduce);
    }
}

/// Wrap a cell into segments of at most `width` display columns.
///
/// Prefers breaking at whitespace (word wrap); tokens that still exceed the
/// width (URLs, CJK runs without spaces) fall back to display-width splits so
/// CJK text never panics or overflows.
fn wrap_cell(cell: &str, width: usize) -> Vec<String> {
    if cell.is_empty() || width == 0 || cell_display_width(cell) <= width {
        return vec![cell.to_string()];
    }
    let mut segs = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;
    for word in cell.split_whitespace() {
        let word_w = cell_display_width(word);
        if cur_w > 0 && cur_w + 1 + word_w > width {
            segs.push(std::mem::take(&mut cur));
            cur_w = 0;
        }
        if word_w > width {
            // Long token: hard-split by display width.
            if cur_w > 0 {
                segs.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            let mut rest = word;
            while cell_display_width(rest) > width {
                let (seg, rem) = split_at_display_width(rest, width);
                segs.push(seg.to_string());
                rest = rem;
            }
            if !rest.is_empty() {
                cur = rest.to_string();
                cur_w = cell_display_width(rest);
            }
        } else {
            if cur_w > 0 {
                cur.push(' ');
                cur_w += 1;
            }
            cur.push_str(word);
            cur_w += word_w;
        }
    }
    if !cur.is_empty() {
        segs.push(cur);
    }
    segs
}

/// Build a table row as alternating pipe + cell spans.
fn styled_table_row(
    cells: &[String],
    cell_fg: ratatui::style::Color,
    bold: bool,
    theme: &Theme,
) -> Line<'static> {
    let pipe = Style::default().fg(theme.accent);
    let mut cell_style = Style::default().fg(cell_fg);
    if bold {
        cell_style = cell_style.add_modifier(Modifier::BOLD);
    }
    let mut spans = Vec::with_capacity(cells.len() * 2 + 1);
    spans.push(Span::styled("|".to_string(), pipe));
    for (i, cell) in cells.iter().enumerate() {
        spans.push(Span::styled(cell.clone(), cell_style));
        if i + 1 < cells.len() {
            spans.push(Span::styled("|".to_string(), pipe));
        }
    }
    spans.push(Span::styled("|".to_string(), pipe));
    Line::from(spans)
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;
    use crate::theme::{Theme, ThemeName};

    fn theme() -> Theme {
        Theme::from(ThemeName::from_str("retro").unwrap())
    }

    #[test]
    fn render_mermaid_sequence_returns_terminal_lines() {
        let source = "sequenceDiagram\n    Alice->>Bob: Hello\n    Bob-->>Alice: Hi";
        let lines = render_mermaid_block(source, &theme(), 80).expect("valid sequence diagram");
        let text = lines
            .iter()
            .map(Line::to_string)
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Alice"), "participant missing: {text}");
        assert!(text.contains("Bob"), "participant missing: {text}");
        assert!(text.contains("Hello"), "message missing: {text}");
        assert!(
            text.contains('─') || text.contains('>'),
            "diagram art missing: {text}"
        );
        assert!(
            !text.contains("sequenceDiagram"),
            "raw source leaked: {text}"
        );
    }

    #[test]
    fn render_markdown_mermaid_flowchart_uses_box_art() {
        let md = "```mermaid\nflowchart TD\n  A[Start] --> B[End]\n```";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(
            lines
                .iter()
                .any(|line| line.to_string().contains('─') || line.to_string().contains('│')),
            "expected flowchart box art: {text}"
        );
        assert!(!text.contains("flowchart TD"), "raw Mermaid leaked: {text}");
    }

    #[test]
    fn render_markdown_invalid_mermaid_falls_back_to_code() {
        let md = "```mermaid\nnot a valid diagram\n```";
        let (_lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(
            text.contains("not a valid diagram"),
            "fallback lost source: {text}"
        );
    }

    #[test]
    fn render_markdown_mermaid_keeps_surrounding_prose() {
        let md = "before\n\n```mermaid\nflowchart TD\n  A --> B\n```\n\nafter";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(lines.iter().any(|l| l.to_string().contains('─')), "{text}");
        assert!(text.contains("before"), "leading prose missing: {text}");
        assert!(text.contains("after"), "trailing prose missing: {text}");
    }

    #[test]
    fn render_markdown_unclosed_mermaid_fence_keeps_source() {
        let md = "```mermaid\nflowchart TD\n  A --> B\n";
        let (_lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(
            text.contains("flowchart TD") && text.contains("A --> B"),
            "unclosed fence must keep its source: {text}"
        );
    }

    #[test]
    fn render_markdown_mermaid_opener_is_case_insensitive() {
        let md = "```MERMAID\npie title T\n  \"A\" : 1\n```";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(
            lines
                .iter()
                .any(|l| l.to_string().contains('█') || l.to_string().contains('%')),
            "uppercase opener should still render: {text}"
        );
        assert!(!text.contains("pie title T"), "raw Mermaid leaked: {text}");
    }

    #[test]
    fn render_markdown_non_mermaid_fence_stays_code() {
        let md = "```rust\nfn kept() {}\n```";
        let (_lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(text.contains("fn kept() {}"), "code source missing: {text}");
    }

    #[test]
    fn render_markdown_mermaid_fence_inside_non_mermaid_fence_is_literal() {
        // A literal ```mermaid line inside an ordinary (non-Mermaid) fenced
        // block is code content, not a diagram: only top-level Mermaid fences
        // may be routed. The nested fence must survive unchanged and no box-art
        // may appear.
        for lang in ["text", "rust"] {
            let md = format!("```{lang}\n```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n");
            let (lines, raw) = render_markdown_tui(&md, &theme());
            let text = raw.join("\n");

            assert!(
                text.contains("sequenceDiagram") && text.contains("Alice->>Bob: Hello"),
                "literal Mermaid fence inside ```{lang} must stay code content: {text}"
            );
            assert!(
                !lines
                    .iter()
                    .any(|l| l.to_string().contains('─') || l.to_string().contains('│')),
                "nested Mermaid fence must not render a diagram (```{lang}): {text}"
            );
        }
    }

    #[test]
    fn render_markdown_mermaid_fence_inside_tilde_fence_is_literal() {
        // A literal ```mermaid block inside a non-Mermaid tilde fence
        // (~~~text) is code content, never a diagram: the router must track
        // tilde fences just like backtick fences.
        let md = "~~~text\n```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n~~~\n";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(
            text.contains("sequenceDiagram") && text.contains("Alice->>Bob: Hello"),
            "literal Mermaid fence inside ~~~text must stay code content: {text}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.to_string().contains('─') || l.to_string().contains('│')),
            "literal Mermaid fence inside ~~~text must not render a diagram: {text}"
        );
    }

    #[test]
    fn render_markdown_mermaid_fence_inside_longer_backtick_fence_is_literal() {
        // A four-backtick ```text fence containing a literal ```mermaid line
        // and an inner ``` line must stay literal code until the four-backtick
        // close: the inner three-backtick line is content, not the closing
        // fence, and no diagram may be routed.
        let md = "````text\n```mermaid\nsequenceDiagram\n  Alice->>Bob: Hello\n```\n````\n";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let text = raw.join("\n");

        assert!(
            text.contains("sequenceDiagram") && text.contains("Alice->>Bob: Hello"),
            "literal Mermaid fence inside ````text must stay code content: {text}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.to_string().contains('─') || l.to_string().contains('│')),
            "literal Mermaid fence inside ````text must not render a diagram: {text}"
        );
    }

    #[test]
    fn render_markdown_with_tables_mermaid_fence_inside_non_mermaid_fence_is_literal() {
        // Same guarantee through the width-aware tables pipeline: the router
        // must pass a non-Mermaid fenced block through before the table
        // scanner (or the inner markdown pass) ever sees its lines.
        let md = "\
prose

```text
```mermaid
sequenceDiagram
  Alice->>Bob: Hello
```
";
        let (lines, raw) = render_markdown_with_tables(md, &theme(), Some(40));
        let text = raw.join("\n");

        assert!(
            text.contains("prose"),
            "surrounding prose must survive: {text}"
        );
        assert!(
            text.contains("sequenceDiagram") && text.contains("Alice->>Bob: Hello"),
            "literal Mermaid fence inside ```text must stay code content: {text}"
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.to_string().contains('─') || l.to_string().contains('│')),
            "nested Mermaid fence must not render a diagram: {text}"
        );
    }

    #[test]
    fn render_markdown_with_tables_routes_mermaid_before_table_scan() {
        // The diagram source contains a valid `|` inside a node label that
        // must be rendered as diagram art, never parsed as a pipe table.
        // Prose tables around it must survive.
        let md = "\
| A | B |
| --- | --- |
| 1 | 2 |

```mermaid
flowchart LR
  A[Start] --> B[Step | done]
```

Trailing prose.
";
        let (styled, raw) = render_markdown_with_tables(md, &theme(), Some(40));
        let text = raw.join("\n");

        assert!(
            styled.iter().any(|l| l.to_string().contains('─')),
            "expected diagram box art: {text}"
        );
        assert!(
            text.contains("Step | done"),
            "node label with `|` must render inside the diagram: {text}"
        );
        assert!(
            text.contains("| A | B |") && text.contains("| 1 | 2 |"),
            "pipe table must survive: {text}"
        );
        assert!(
            text.contains("Trailing prose."),
            "trailing prose must survive: {text}"
        );
        assert!(!text.contains("flowchart LR"), "raw Mermaid leaked: {text}");
    }

    #[test]
    fn render_markdown_with_tables_invalid_mermaid_falls_back() {
        let md = "```mermaid\nnot a valid diagram\n```";
        let (_lines, raw) = render_markdown_with_tables(md, &theme(), Some(40));
        let text = raw.join("\n");

        assert!(
            text.contains("not a valid diagram"),
            "fallback lost source: {text}"
        );
    }

    #[test]
    fn render_markdown_heading_and_list() {
        let md = "# Title\n\n- item one\n- item two";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let joined = raw.join("\n");
        assert!(joined.contains("Title"), "heading: {joined}");
        assert!(joined.contains("item one"), "list: {joined}");
        assert!(!lines.is_empty());
    }

    #[test]
    fn render_markdown_task_lists_without_panicking() {
        let md = "- context\n\n- [ ] pending\n- [x] complete\n1. [X] ordered";

        let (_lines, raw) = render_markdown_tui(md, &theme());
        let joined = raw.join("\n");

        assert!(joined.contains("☐ pending"), "{joined}");
        assert!(joined.contains("☑ complete"), "{joined}");
        // GFM task lists apply to ordered lists too: `1. [X]` becomes the
        // numbered checkbox `1. ☑` (pulldown-cmark behaviour).
        assert!(joined.contains("1. ☑ ordered"), "{joined}");
    }

    #[test]
    fn render_markdown_preserves_task_marker_inside_fenced_code() {
        let md = "```markdown\n- [ ] literal example\n```";

        let (_lines, raw) = render_markdown_tui(md, &theme());

        assert!(raw.join("\n").contains("- [ ] literal example"));
    }

    #[test]
    fn render_markdown_list_then_fenced_code_then_list_tail() {
        let md = "- 例如：\n  - 为什么不是远端 compact\n  - 为什么不能先 push 当前 turn 再 compact\n```\n - 为什么 assistant history 要规范化成 completed output message\n```";

        let (_lines, raw) = render_markdown_tui(md, &theme());
        let joined = raw.join("\n");

        assert!(
            joined.contains("为什么不是远端 compact"),
            "first nested list item missing: {joined}"
        );
        assert!(
            joined.contains("为什么不能先 push 当前 turn 再 compact"),
            "second nested list item missing: {joined}"
        );
        assert!(
            joined.contains("为什么 assistant history 要规范化成 completed output message"),
            "tail line after fenced code missing or swallowed: {joined}"
        );
    }

    #[test]
    fn render_markdown_fenced_code_block() {
        let md = "```rust\nfn md_test() {}\n```";
        let (lines, raw) = render_markdown_tui(md, &theme());
        // The renderer consumes the fence markers at parse time; raw rows
        // (copy / hit-testing) and styled rows both carry only the code text.
        let joined_raw = raw.join("\n");
        assert!(
            joined_raw.contains("fn md_test() {}"),
            "raw keeps code content: {joined_raw}"
        );
        assert!(
            !joined_raw.contains("```"),
            "fence markers are consumed: {joined_raw}"
        );
        let styled = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            styled.contains("fn md_test() {}"),
            "code block content: {styled}"
        );
        assert!(
            !styled.contains("```"),
            "fence markers must be hidden in the styled output: {styled}"
        );
        assert!(
            lines.iter().any(|l| l
                .spans
                .iter()
                .any(|s| s.style.bg == Some(theme().code_block_bg()))),
            "code rows carry the code background"
        );
    }

    #[test]
    fn render_markdown_blockquote() {
        let md = "> quoted wisdom";
        let (lines, _raw) = render_markdown_tui(md, &theme());
        let styled = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            styled.contains("quoted wisdom"),
            "blockquote text: {styled}"
        );
        assert!(
            styled.contains("▎") && !styled.contains('>'),
            "the literal > marker should be replaced by the ▎ gutter: {styled}"
        );
    }

    #[test]
    fn render_markdown_heading_markers_are_stripped() {
        let md = "## Sub heading\nplain";
        let (lines, raw) = render_markdown_tui(md, &theme());
        // Markers are consumed at parse time — neither raw nor styled rows
        // keep the `## ` prefix.
        let joined_raw = raw.join("\n");
        assert!(
            joined_raw.contains("Sub heading"),
            "raw keeps heading text: {joined_raw}"
        );
        assert!(
            !joined_raw.contains("##"),
            "heading marker is consumed: {joined_raw}"
        );
        let styled = lines
            .iter()
            .map(|l| l.to_string())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            styled.contains("Sub heading"),
            "heading text must stay: {styled}"
        );
        assert!(
            !styled.contains("##"),
            "the heading marker should be stripped: {styled}"
        );
    }

    #[test]
    fn is_horizontal_rule_detects_dashes() {
        assert!(is_horizontal_rule("---"));
        assert!(is_horizontal_rule("  ***  "));
        assert!(!is_horizontal_rule("not a rule"));
    }

    #[test]
    fn format_table_aligns_columns() {
        let rows = vec![
            "| Name | Val |".to_string(),
            "| --- | --- |".to_string(),
            "| foo | 1 |".to_string(),
        ];
        let (styled, raw) = format_table(&rows, &theme(), None);
        assert!(!styled.is_empty());
        assert!(raw.iter().any(|r| r.contains("foo")));
        // Header + separator + body
        assert_eq!(raw.len(), 3);
        let pipe_cols: Vec<Vec<usize>> = raw
            .iter()
            .map(|r| {
                r.char_indices()
                    .filter(|(_, c)| *c == '|')
                    .map(|(i, _)| i)
                    .collect()
            })
            .collect();
        assert!(
            pipe_cols.windows(2).all(|w| w[0] == w[1]),
            "pipe columns should align:\n{}",
            raw.join("\n")
        );
    }

    #[test]
    fn format_table_aligns_cjk_and_ascii() {
        let rows = vec![
            "| # | 文件名 | 类型 | 内容 |".to_string(),
            "|---|--------|------|------|".to_string(),
            "| 1 | 'alpha_27c4.txt' | 文本 | 随机问候 + 时间戳 |".to_string(),
            "| 3 | 'gamma_a1b2.json' | JSON | {\"name\":\"gamma\"} |".to_string(),
            "| 5 | 'epsilon.env' | 环境变量 | 测试配置 |".to_string(),
        ];
        let (_styled, raw) = format_table(&rows, &theme(), None);
        assert_eq!(raw.len(), 5, "header + sep + 3 data rows");

        // All rows must have the same display width and pipe positions.
        let widths: Vec<usize> = raw
            .iter()
            .map(|r| UnicodeWidthStr::width(r.as_str()))
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "row display widths differ: {widths:?}\n{}",
            raw.join("\n")
        );

        let pipe_display_cols = |s: &str| -> Vec<usize> {
            let mut cols = Vec::new();
            let mut col = 0;
            for ch in s.chars() {
                if ch == '|' {
                    cols.push(col);
                }
                col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            }
            cols
        };
        let cols: Vec<Vec<usize>> = raw.iter().map(|r| pipe_display_cols(r)).collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "pipe display columns misaligned:\n{}",
            raw.join("\n")
        );
    }

    #[test]
    fn format_table_header_is_bold() {
        let rows = vec![
            "| # | 文件 | 内容 |".to_string(),
            "|---|------|------|".to_string(),
            "| 1 | a.txt | hello |".to_string(),
        ];
        let (styled, _raw) = format_table(&rows, &theme(), None);
        let header = &styled[0];
        assert!(
            header
                .spans
                .iter()
                .any(|s| s.style.add_modifier.contains(Modifier::BOLD) && s.content.contains('#')),
            "header cell with # should be bold: {header:?}"
        );
        let body = &styled[2];
        assert!(
            body.spans
                .iter()
                .all(|s| !s.style.add_modifier.contains(Modifier::BOLD)),
            "body row should not be bold"
        );
    }

    #[test]
    fn format_table_wraps_long_cells_without_breaking_alignment() {
        // A long description would previously widen the column past the panel
        // and the log wrapper would char-split the row, dropping the pipes.
        let rows = vec![
            "| Name | Description |".to_string(),
            "| --- | --- |".to_string(),
            "| alpha | a very long description that definitely exceeds the available width and must be wrapped |".to_string(),
        ];
        let (styled, raw) = format_table(&rows, &theme(), Some(40));
        assert!(
            raw.len() >= 4,
            "long cell should wrap into sub-rows:\n{}",
            raw.join("\n")
        );

        // Every emitted line (including wrapped sub-rows) stays within the
        // panel width and keeps the same pipe positions.
        let pipe_display_cols = |s: &str| -> Vec<usize> {
            let mut cols = Vec::new();
            let mut col = 0;
            for ch in s.chars() {
                if ch == '|' {
                    cols.push(col);
                }
                col += unicode_width::UnicodeWidthChar::width(ch).unwrap_or(0);
            }
            cols
        };
        let cols: Vec<Vec<usize>> = raw.iter().map(|r| pipe_display_cols(r)).collect();
        assert!(
            cols.windows(2).all(|w| w[0] == w[1]),
            "pipe columns misaligned after wrap:\n{}",
            raw.join("\n")
        );
        assert_eq!(
            cols[0].len(),
            3,
            "every wrapped row keeps leading + inner + trailing pipe"
        );
        assert!(
            raw.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 40),
            "no line may exceed the available width:\n{}",
            raw.join("\n")
        );
        assert_eq!(styled.len(), raw.len());
        // First body sub-row stays in the stream; the wrapped continuation is
        // a normal line, never a second bold header.
        assert!(raw[3].contains('|'), "continuation keeps pipes");
    }

    #[test]
    fn format_table_wraps_long_cjk_cells() {
        // CJK runs have no spaces — wrapping must fall back to display-width
        // splits without panicking or overflowing the column.
        let rows = vec![
            "| 列名 | 描述 |".to_string(),
            "| --- | --- |".to_string(),
            "| x | 这是一段非常长的中文描述内容没有任何空格只能按显示宽度拆分成多行显示 |"
                .to_string(),
        ];
        let (_styled, raw) = format_table(&rows, &theme(), Some(30));
        assert!(
            raw.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 30),
            "CJK wrap must stay within width:\n{}",
            raw.join("\n")
        );
        assert!(raw.len() > 3, "CJK cell should wrap:\n{}", raw.join("\n"));
    }

    #[test]
    fn format_table_fits_available_width() {
        // One pathological cell must not widen other columns beyond the panel.
        let rows = vec![
            "| A | B |".to_string(),
            "| --- | --- |".to_string(),
            "| short | https://example.com/very/long/url/that/should/be/wrapped |".to_string(),
        ];
        let (_styled, raw) = format_table(&rows, &theme(), Some(50));
        assert!(
            raw.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 50),
            "table must fit available width:\n{}",
            raw.join("\n")
        );
        // Both columns were shrunk (the long URL column can no longer be 60+).
        let sep = &raw[1];
        let dash_run = sep.split('|').nth(2).unwrap_or("").trim();
        assert!(
            dash_run.len() <= 40,
            "column B should be shrunk: {dash_run:?}"
        );
    }

    #[test]
    fn format_table_renders_row_separators_aligned() {
        // `/skills` style: header separator plus a separator between rows.
        let rows = vec![
            "| Skill | Description |".to_string(),
            "| ----- | ----------- |".to_string(),
            "| a | first skill |".to_string(),
            "| --- | --- |".to_string(),
            "| b | second skill |".to_string(),
        ];
        let (_styled, raw) = format_table(&rows, &theme(), None);
        assert_eq!(
            raw.len(),
            5,
            "header + sep + row + row-sep + row:\n{}",
            raw.join("\n")
        );
        // The row separator is a dashed divider, not a data row.
        assert!(
            raw[3].chars().all(|c| matches!(c, '|' | '-' | ' ')),
            "row separator should be dashed:\n{}",
            raw[3]
        );
        assert!(
            raw[3].contains("---"),
            "row separator should contain dashes:\n{}",
            raw[3]
        );
        // All rows — including the separators — keep identical pipe columns.
        let pipe_cols: Vec<Vec<usize>> = raw
            .iter()
            .map(|r| {
                r.char_indices()
                    .filter(|(_, c)| *c == '|')
                    .map(|(i, _)| i)
                    .collect()
            })
            .collect();
        assert!(
            pipe_cols.windows(2).all(|w| w[0] == w[1]),
            "pipe columns misaligned with row separators:\n{}",
            raw.join("\n")
        );
    }

    #[test]
    fn render_markdown_with_tables_mixes_prose_and_width_aware_table() {
        let md = "\
## 📋 Available skills

| Skill | Description |
| ----- | ----------- |
| code-reviewer | A very long description that keeps going and definitely exceeds the available width of the panel and must wrap inside the table |
| demo-test | 测试 skill 加载功能 |

Plain trailing paragraph.
";
        let (styled, raw) = render_markdown_with_tables(md, &theme(), Some(50));
        assert!(!styled.is_empty());

        // Table rows are width-aware: every emitted line fits and pipes align.
        let table_lines: Vec<&String> = raw.iter().filter(|l| l.contains('|')).collect();
        assert!(
            !table_lines.is_empty(),
            "expected table rows:\n{}",
            raw.join("\n")
        );
        assert!(
            table_lines
                .iter()
                .all(|l| UnicodeWidthStr::width(l.as_str()) <= 50),
            "table lines must fit available width:\n{}",
            raw.join("\n")
        );
        // Wrapped continuation rows keep the pipes (alignment preserved).
        assert!(
            table_lines.len() > 3,
            "long description should wrap into extra sub-rows:\n{}",
            raw.join("\n")
        );
        // Heading is rendered by pulldown-cmark (not swallowed by table logic).
        assert!(
            raw.iter().any(|l| l.contains("Available skills")),
            "heading missing:\n{}",
            raw.join("\n")
        );
        // Trailing prose survives after the table.
        assert!(
            raw.iter().any(|l| l.contains("Plain trailing paragraph")),
            "trailing prose missing:\n{}",
            raw.join("\n")
        );
    }

    #[test]
    fn render_markdown_with_tables_none_width_keeps_unlimited_table() {
        // `None` must flow through to `format_table` unchanged (unlimited):
        // a long cell stays on a single row instead of being wrapped at the
        // nominal 80-column width.
        let cell = "word ".repeat(40); // ~200 chars
        let long_cell = cell.trim_end();
        let md = format!("| A | B |\n| --- | --- |\n| 1 | {long_cell} |");
        let (_styled, raw) = render_markdown_with_tables(&md, &theme(), None);
        let text = raw.join("\n");

        let full_row = format!("| 1 | {long_cell} |");
        assert!(
            text.lines().any(|l| l == full_row),
            "long cell must stay on one row when width is None:\n{text}"
        );
    }

    /// Regression baseline for the pulldown-cmark renderer: locks the
    /// observable behaviour of the constructs the consolidation preserves
    /// (heading levels, emphasis nesting, lists, task lists, blockquotes).
    mod baseline {
        use ratatui::style::Modifier;

        use super::*;

        /// Locks the current pipeline's observable behaviour for the
        /// constructs the consolidation must preserve.
        #[test]
        fn corpus_baseline_current_pipeline() {
            let theme = Theme::from(ThemeName::Dark);

            // H1: marker stripped, bold+underline heading style.
            let (lines, _) = render_markdown_tui("# Title one\n\nplain", &theme);
            let h1 = &lines[0];
            assert!(
                h1.to_string().contains("Title one") && !h1.to_string().contains('#'),
                "h1 text/marker: {h1:?}"
            );
            assert!(
                h1.style
                    .add_modifier
                    .contains(Modifier::BOLD | Modifier::UNDERLINED)
                    || h1.spans.iter().any(|s| s
                        .style
                        .add_modifier
                        .contains(Modifier::BOLD | Modifier::UNDERLINED))
            );

            // H4: marker stripped, styled bold+italic.
            let (lines, _) = render_markdown_tui("#### Small heading", &theme);
            let h4 = &lines[0];
            assert!(
                h4.to_string().contains("Small heading") && !h4.to_string().contains("####"),
                "h4 text/marker: {h4:?}"
            );
            assert!(
                h4.style.add_modifier.contains(Modifier::BOLD)
                    || h4
                        .spans
                        .iter()
                        .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
            );

            // Bold: `**` markers stripped, BOLD span present.
            let (lines, _) = render_markdown_tui("Some **bold** text", &theme);
            let text = lines[0].to_string();
            assert!(
                !text.contains("**"),
                "bold markers must be stripped: {text}"
            );
            assert!(
                lines[0]
                    .spans
                    .iter()
                    .any(|s| s.style.add_modifier.contains(Modifier::BOLD))
            );

            // Nested ordered list keeps its number.
            let (lines, _) = render_markdown_tui("- item\n  1. ordered sub", &theme);
            let text = lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("1. ordered sub"),
                "ordered number lost: {text}"
            );

            // Task-list markers render as checkbox glyphs (no panic, no escape).
            let (lines, _) = render_markdown_tui("- [ ] pending\n- [x] done", &theme);
            let text = lines
                .iter()
                .map(Line::to_string)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(
                text.contains("☐ pending") && text.contains("☑ done"),
                "{text}"
            );

            // Blockquote: ▎ gutter replaces the `>` marker.
            let (lines, _) = render_markdown_tui("> quoted", &theme);
            let text = lines[0].to_string();
            assert!(text.contains('▎') && !text.contains('>'), "{text}");
        }
    }
}
