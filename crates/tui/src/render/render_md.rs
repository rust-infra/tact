use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use ratatui_markdown::{
    markdown::MarkdownRenderer,
    mermaid::{render_mermaid, theme::MermaidTheme},
    theme::{CodeColors, Generation, RichTextTheme},
};
use std::ops::Range;
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

/// Renders a Markdown table's cells into column-aligned ratatui Lines.
///
/// `headers` are the GFM header cells and `rows` the body rows; cells are
/// taken verbatim, so a `|` inside a cell (inline code, escaped pipe) is data,
/// never a column separator. Column widths use Unicode display width so CJK
/// headers/cells align with ASCII. The GFM header separator is synthesized;
/// body rows whose cells are all dashes render as dashed dividers (`/skills`
/// style). When `available_width` is given, columns are shrunk to fit and long
/// cells are wrapped *inside* the table layout (every wrapped sub-row keeps
/// its pipes and padding), so the panel's line wrapper never breaks a row and
/// misaligns the columns. Tables that are still too wide even at the
/// readability floor are split into contiguous column chunks (each with its
/// own header), so every rendered row always fits the panel width.
pub(crate) fn format_table(
    headers: &[String],
    rows: &[Vec<String>],
    theme: &Theme,
    available_width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    // No header row (degenerate input): the first body row acts as header,
    // matching the old string-parsing path where row 0 was styled as header.
    let (headers, rows): (Vec<String>, &[Vec<String>]) = if headers.is_empty() && !rows.is_empty() {
        (
            rows[0].iter().map(|s| s.trim().to_string()).collect(),
            &rows[1..],
        )
    } else {
        (headers.iter().map(|s| s.trim().to_string()).collect(), rows)
    };

    let col_count = rows
        .iter()
        .map(|r| r.len())
        .chain(std::iter::once(headers.len()))
        .max()
        .unwrap_or(0);
    if col_count == 0 {
        return (Vec::new(), Vec::new());
    }

    // Header + synthesized separator + body rows. Body rows whose cells are
    // all dashes stay in the stream and render as dashed dividers.
    let mut all_rows: Vec<Vec<String>> = Vec::with_capacity(rows.len() + 2);
    all_rows.push(headers);
    all_rows.push(vec!["---".to_string(); col_count]);
    all_rows.extend(
        rows.iter()
            .map(|r| r.iter().map(|c| c.trim().to_string()).collect::<Vec<_>>()),
    );

    let mut col_widths = vec![0; col_count];
    for row in &all_rows {
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
    // When even the floor is too wide, split the columns into chunks that
    // each fit, so rows are never wider than the panel (which would make the
    // outer line wrapper shred the pipe alignment).
    let chunks: Vec<Range<usize>> = match available_width {
        None => std::iter::once(0..col_count).collect(),
        Some(avail) => {
            fit_columns_to_width(&mut col_widths, avail, MIN_COL_WIDTH);
            if row_width(&col_widths, 0, col_count) > avail && col_count > 1 {
                // Keep the table intact if a compact layout (columns still ≥
                // COMPACT_COL_WIDTH wide) fits: an intact 4-column table is
                // far more readable than a 3+1 chunk split with a repeated
                // header. Only split when even the compact floor is too wide.
                let readable = col_widths.clone();
                fit_columns_to_width(&mut col_widths, avail, COMPACT_COL_WIDTH);
                if row_width(&col_widths, 0, col_count) > avail {
                    col_widths = readable;
                    split_into_fitting_chunks(&mut col_widths, avail)
                } else {
                    std::iter::once(0..col_count).collect()
                }
            } else {
                std::iter::once(0..col_count).collect()
            }
        }
    };

    let mut styled_lines = Vec::new();
    let mut raw_lines = Vec::new();

    for chunk in &chunks {
        render_table_chunk(
            &all_rows,
            &col_widths,
            chunk.clone(),
            theme,
            &mut styled_lines,
            &mut raw_lines,
        );
    }

    (styled_lines, raw_lines)
}

/// Renders raw Markdown table source lines (`| a | b |` style) through the
/// standard pulldown pipeline.
///
/// Used by the line-oriented streaming renderer, which buffers complete
/// source lines before flushing. Going through pulldown (instead of re-splitting
/// on `|`) keeps pipes inside code spans / escaped cells as data.
pub(crate) fn format_table_lines(
    lines: &[String],
    theme: &Theme,
    available_width: Option<usize>,
) -> (Vec<Line<'static>>, Vec<String>) {
    render_markdown_with_tables(&lines.join("\n"), theme, available_width)
}

/// Display width of a rendered row covering `cols[from..to]`: two padding
/// spaces per cell plus one pipe per column and a leading pipe.
fn row_width(col_widths: &[usize], from: usize, to: usize) -> usize {
    col_widths[from..to].iter().sum::<usize>() + 3 * (to - from) + 1
}

/// Split columns into contiguous chunks that each fit `available_width`.
///
/// Used when the whole table (even at the readability floor) is too wide.
/// A single column that alone would not fit is shrunk below the floor so its
/// chunk always fits; every chunk renders as its own table block with the
/// header repeated.
fn split_into_fitting_chunks(
    col_widths: &mut [usize],
    available_width: usize,
) -> Vec<Range<usize>> {
    let mut chunks = Vec::new();
    let mut start = 0usize;
    while start < col_widths.len() {
        // One column alone costs `w + 4` columns; shrink it so its chunk fits.
        if row_width(col_widths, start, start + 1) > available_width {
            col_widths[start] = available_width.saturating_sub(4).max(1);
        }
        let mut end = start + 1;
        while end < col_widths.len() && row_width(col_widths, start, end + 1) <= available_width {
            end += 1;
        }
        chunks.push(start..end);
        start = end;
    }
    chunks
}

/// Render one contiguous column chunk as a full table block: header row,
/// separator, then body rows with cells wrapped inside the column layout.
/// Every wrapped sub-row keeps its pipes and padding, so all visual rows of a
/// chunk align on the same pipe columns.
fn render_table_chunk(
    rows: &[Vec<String>],
    col_widths: &[usize],
    cols: Range<usize>,
    theme: &Theme,
    styled_lines: &mut Vec<Line<'static>>,
    raw_lines: &mut Vec<String>,
) {
    let push_separator = |styled_lines: &mut Vec<Line<'static>>,
                          raw_lines: &mut Vec<String>| {
        let sep_cells: Vec<String> = cols
            .clone()
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
    };

    for (row_idx, row) in rows.iter().enumerate() {
        let is_sep = row.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        });

        if is_sep {
            // Render a visual separator that matches column widths.
            push_separator(styled_lines, raw_lines);
            continue;
        }

        // Wrap each cell to its column width; a row becomes several visual
        // sub-rows, each still padded and pipe-separated so columns stay
        // aligned across the wrap.
        let wrapped: Vec<Vec<String>> = cols
            .clone()
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
                let width = col_widths.get(cols.start + i).copied().unwrap_or(0);
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

        // Row separators between body rows (`rows` = header + synthesized
        // separator + body): after each body row except the last, emit a
        // horizontal rule so multi-row tables read as grid lines, not as one
        // blob of wrapped text. Wrapped sub-rows stay above the rule. A body
        // row followed by an explicit dash-only row already has its divider,
        // so no extra rule is added there (`/skills` style).
        let is_body = row_idx >= 2;
        let is_last_row = row_idx + 1 == rows.len();
        let next_is_sep = rows.get(row_idx + 1).is_some_and(|r| {
            r.iter().all(|c| {
                c.chars()
                    .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
            })
        });
        if is_body && !is_last_row && !next_is_sep {
            push_separator(styled_lines, raw_lines);
        }
    }
}

/// Minimum content width a column keeps when shrinking to fit.
const MIN_COL_WIDTH: usize = 8;

/// Compact floor used to keep an over-wide table intact instead of splitting
/// it into column chunks. A column this narrow still shows ~2 CJK glyphs per
/// line; only when even this cannot fit do we fall back to chunk splits
/// (which repeat the header and visually break the table apart).
const COMPACT_COL_WIDTH: usize = 4;

/// Shrink the widest columns until the rendered table fits `available_width`.
///
/// A rendered row costs `sum(widths) + 3 * col_count + 1` display columns:
/// two padding spaces per cell plus one pipe per column and a leading pipe.
/// Columns stop shrinking at `floor` — [`MIN_COL_WIDTH`] for the readability
/// pass, [`COMPACT_COL_WIDTH`] when trying to keep an over-wide table intact.
fn fit_columns_to_width(col_widths: &mut [usize], available_width: usize, floor: usize) {
    let row_width = |widths: &[usize]| widths.iter().sum::<usize>() + 3 * widths.len() + 1;
    let mut overflow = row_width(col_widths).saturating_sub(available_width);
    while overflow > 0 {
        // Widest column still above the floor.
        let mut target = None;
        let mut widest = floor;
        for (i, &w) in col_widths.iter().enumerate() {
            if w > widest {
                widest = w;
                target = Some(i);
            }
        }
        let Some(idx) = target else { break };
        let reduce = (col_widths[idx] - floor).min(overflow);
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
        let headers = vec!["Name".to_string(), "Val".to_string()];
        let rows = vec![vec!["foo".to_string(), "1".to_string()]];
        let (styled, raw) = format_table(&headers, &rows, &theme(), None);
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
    fn format_table_keeps_pipe_inside_cell() {
        // 回归：单元格里的 `|`（行内代码 / 转义管道）是数据，不是列分隔符。
        // 旧实现把单元格拼回 "| a | b |" 字符串再按 '|' 切分，`a|b|c` 会被
        // 拆成三列导致整行错位。
        let headers = vec!["A".to_string(), "B".to_string()];
        let rows = vec![
            vec!["x".to_string(), "a|b|c".to_string()],
            vec!["y".to_string(), "esc\\|pipe".to_string()],
        ];
        let (_styled, raw) = format_table(&headers, &rows, &theme(), None);
        assert_eq!(
            raw.len(),
            5,
            "header + sep + body + row-sep + body:\n{}",
            raw.join("\n")
        );
        // The header row has exactly 3 structural pipes.
        assert_eq!(raw[0].matches('|').count(), 3, "header pipes: {}", raw[0]);
        // The pipe data survives intact inside the second column.
        let body = &raw[2];
        assert!(body.contains("a|b|c"), "{body}");
        let body2 = &raw[4];
        assert!(body2.contains("esc\\|pipe"), "{body2}");
        // Every row has the same display width (uniform padding), and the
        // structural pipes — the boundary after column A and the trailing
        // edge — sit at the same display columns in every row. Pipes inside
        // cell data are allowed between them.
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
        let sep: Vec<Vec<usize>> = raw.iter().map(|r| pipe_display_cols(r)).collect();
        assert!(
            sep.iter()
                .all(|c| c.first() == sep[0].first() && c.last() == sep[0].last()),
            "structural pipes must align:\n{}",
            raw.join("\n")
        );
        // Column A is one display column wide, so the A/B boundary pipe sits
        // at display column 4 in every row (`| x |`).
        assert_eq!(sep[0][1], 4, "boundary pipe position: {:?}", sep[0]);
    }

    #[test]
    fn format_table_aligns_cjk_and_ascii() {
        let headers = vec![
            "#".to_string(),
            "文件名".to_string(),
            "类型".to_string(),
            "内容".to_string(),
        ];
        let rows = vec![
            vec![
                "1".to_string(),
                "'alpha_27c4.txt'".to_string(),
                "文本".to_string(),
                "随机问候 + 时间戳".to_string(),
            ],
            vec![
                "3".to_string(),
                "'gamma_a1b2.json'".to_string(),
                "JSON".to_string(),
                "{\"name\":\"gamma\"}".to_string(),
            ],
            vec![
                "5".to_string(),
                "'epsilon.env'".to_string(),
                "环境变量".to_string(),
                "测试配置".to_string(),
            ],
        ];
        let (_styled, raw) = format_table(&headers, &rows, &theme(), None);
        assert_eq!(
            raw.len(),
            7,
            "header + sep + 3 data rows + 2 row separators"
        );

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
        let headers = vec!["#".to_string(), "文件".to_string(), "内容".to_string()];
        let rows = vec![vec![
            "1".to_string(),
            "a.txt".to_string(),
            "hello".to_string(),
        ]];
        let (styled, _raw) = format_table(&headers, &rows, &theme(), None);
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
        let headers = vec!["Name".to_string(), "Description".to_string()];
        let rows = vec![vec![
            "alpha".to_string(),
            "a very long description that definitely exceeds the available width and must be wrapped"
                .to_string(),
        ]];
        let (styled, raw) = format_table(&headers, &rows, &theme(), Some(40));
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
        let headers = vec!["列名".to_string(), "描述".to_string()];
        let rows = vec![vec![
            "x".to_string(),
            "这是一段非常长的中文描述内容没有任何空格只能按显示宽度拆分成多行显示".to_string(),
        ]];
        let (_styled, raw) = format_table(&headers, &rows, &theme(), Some(30));
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
        let headers = vec!["A".to_string(), "B".to_string()];
        let rows = vec![vec![
            "short".to_string(),
            "https://example.com/very/long/url/that/should/be/wrapped".to_string(),
        ]];
        let (_styled, raw) = format_table(&headers, &rows, &theme(), Some(50));
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
    fn format_table_keeps_overwide_table_intact_when_compact_fits() {
        // 回归 / 优化：表格内容超过可用宽度时，先尝试把列压缩到紧凑地板
        // （COMPACT_COL_WIDTH = 4）让整张表完整显示；只有紧凑布局也放不下
        // 时才拆成列块。此前 4 列长表格在窄面板被拆成 3+1，第二块孤零零
        // 一列、表头重复，观感破碎。
        let headers = vec![
            "编号".to_string(),
            "问题描述".to_string(),
            "影响范围".to_string(),
            "处理建议".to_string(),
        ];
        let rows = vec![vec![
            "1".to_string(),
            "当用户连续快速点击「保存」按钮超过五次时，系统会偶发出现重复提交".to_string(),
            "涉及用户管理、订单管理、商品管理、配置管理四个模块".to_string(),
            "在前端增加防抖与提交锁，后端增加唯一性约束校验".to_string(),
        ]];

        // 35 columns: 4 columns at COMPACT_COL_WIDTH = 4 cost 4*4 + 3*4 + 1 =
        // 29 <= 35, so the table must stay intact (header appears exactly once).
        let (_styled, raw) = format_table(&headers, &rows, &theme(), Some(35));
        // One chunk => exactly one synthesized separator row (a split table
        // would repeat the separator once per chunk).
        let sep_rows = raw
            .iter()
            .filter(|r| {
                let trimmed = r.trim_matches('|');
                !trimmed.is_empty()
                    && trimmed.split('|').all(|seg| {
                        seg.chars()
                            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                    })
            })
            .count();
        assert_eq!(
            sep_rows,
            1,
            "compact layout must keep the table intact (no chunk split):\n{}",
            raw.join("\n")
        );
        assert!(
            raw.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 35),
            "rows must fit the available width:\n{}",
            raw.join("\n")
        );
        // Four columns, all visible in one block.
        let pipe_cols = |s: &str| -> Vec<usize> {
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
        let first = pipe_cols(&raw[0]);
        assert_eq!(first.len(), 5, "4 columns + leading pipe: {first:?}");
        assert!(
            raw.iter().map(|r| pipe_cols(r)).all(|c| c == first),
            "all rows share the same pipe columns:\n{}",
            raw.join("\n")
        );
    }

    #[test]
    fn format_table_splits_chunks_only_when_compact_cannot_fit() {
        // 12 列在 95 列宽：紧凑布局 12*4 + 3*12 + 1 = 85 <= 95 → 完整显示，
        // 不拆块（此前贪心拆成 8+4，第二块只占一半宽度）。
        let headers: Vec<String> = (1..=12).map(|i| format!("列{i}")).collect();
        let rows = vec![
            (1..=12)
                .map(|c| format!("https://example.com/path/{c}/very/long/url"))
                .collect::<Vec<_>>(),
            (1..=12)
                .map(|c| format!("单元格{c}号的内容比较长需要换行显示"))
                .collect::<Vec<_>>(),
        ];
        let (_styled, raw) = format_table(&headers, &rows, &theme(), Some(95));
        let sep_rows = raw
            .iter()
            .filter(|r| {
                let trimmed = r.trim_matches('|');
                !trimmed.is_empty()
                    && trimmed.split('|').all(|seg| {
                        seg.chars()
                            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                    })
            })
            .count();
        assert_eq!(
            sep_rows,
            2,
            "12 columns at 95 wide: header separator + one row separator:\n{}",
            raw.join("\n")
        );
        assert!(
            raw.iter().all(|r| UnicodeWidthStr::width(r.as_str()) <= 95),
            "rows must fit:\n{}",
            raw.join("\n")
        );

        // 10 列在 40 列宽：紧凑布局 10*4 + 3*10 + 1 = 71 > 40 → 拆块兜底
        // 仍然工作（wide_table_chunks_into_fitting_blocks 覆盖渲染层）。
        let headers10: Vec<String> = (1..=10).map(|i| format!("c{i}")).collect();
        let rows10 = vec![(1..=10).map(|c| format!("v{c}")).collect::<Vec<_>>()];
        let (_styled, raw10) = format_table(&headers10, &rows10, &theme(), Some(40));
        let sep_rows10 = raw10
            .iter()
            .filter(|r| {
                let trimmed = r.trim_matches('|');
                !trimmed.is_empty()
                    && trimmed.split('|').all(|seg| {
                        seg.chars()
                            .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
                    })
            })
            .count();
        assert_eq!(
            sep_rows10,
            2,
            "10 columns at 40 wide must split into two chunks:\n{}",
            raw10.join("\n")
        );
        assert!(
            raw10
                .iter()
                .all(|r| UnicodeWidthStr::width(r.as_str()) <= 40),
            "chunk rows must fit:\n{}",
            raw10.join("\n")
        );
    }

    #[test]
    fn format_table_renders_row_separators_aligned() {
        // `/skills` style: header separator plus a separator between rows.
        let headers = vec!["Skill".to_string(), "Description".to_string()];
        let rows = vec![
            vec!["a".to_string(), "first skill".to_string()],
            vec!["---".to_string(), "---".to_string()],
            vec!["b".to_string(), "second skill".to_string()],
        ];
        let (_styled, raw) = format_table(&headers, &rows, &theme(), None);
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
