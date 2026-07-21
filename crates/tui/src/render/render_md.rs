use std::borrow::Cow;

use pulldown_cmark::{Event, Options as MarkdownOptions, Parser};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use unicode_width::UnicodeWidthStr;

use crate::theme::Theme;

/// Theme-aware StyleSheet for tui-markdown.
#[derive(Clone, Copy, Debug)]
struct TuiStyleSheet {
    theme: Theme,
}

impl TuiStyleSheet {
    fn new(theme: Theme) -> Self {
        Self { theme }
    }
}

impl tui_markdown::StyleSheet for TuiStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::new()
                .fg(self.theme.accent)
                .bg(self.theme.highlight)
                .bold()
                .underlined(),
            2 => Style::new().fg(self.theme.accent).bold(),
            3 => Style::new().fg(self.theme.accent).bold().italic(),
            4 => Style::new().fg(self.theme.fg).bold().italic(),
            5 => Style::new().fg(self.theme.fg).italic(),
            _ => Style::new().fg(self.theme.fg).italic(),
        }
    }

    fn code(&self) -> Style {
        Style::new()
            .fg(self.theme.code_block_fg())
            .bg(self.theme.code_block_bg())
    }

    fn link(&self) -> Style {
        Style::new().fg(Color::Blue).underlined()
    }

    fn blockquote(&self) -> Style {
        Style::new().fg(self.theme.success)
    }

    fn heading_meta(&self) -> Style {
        Style::new().fg(self.theme.muted_fg())
    }

    fn metadata_block(&self) -> Style {
        Style::new().fg(self.theme.warning)
    }
}

/// Renders Markdown text into ratatui Line list and raw text list using tui-markdown.
pub(crate) fn render_markdown_tui(text: &str, theme: &Theme) -> (Vec<Line<'static>>, Vec<String>) {
    let options = tui_markdown::Options::new(TuiStyleSheet::new(*theme));
    let safe_text = escape_task_list_markers(text);
    let tui_text = tui_markdown::from_str_with_options(safe_text.as_ref(), &options);
    let mut styled_lines: Vec<Line<'static>> = tui_text
        .lines
        .into_iter()
        .map(|line| {
            let spans: Vec<Span<'static>> = line
                .spans
                .into_iter()
                .map(|s| Span::styled(s.content.into_owned(), s.style))
                .collect();
            let mut new_line = Line::from(spans).style(line.style);
            if let Some(alignment) = line.alignment {
                new_line = new_line.alignment(alignment);
            }
            new_line
        })
        .collect();
    let raw_lines: Vec<String> = styled_lines.iter().map(|l| l.to_string()).collect();

    apply_code_background(&mut styled_lines, &raw_lines, theme);
    apply_blockquote_indicator(&mut styled_lines, theme);

    let raw_lines: Vec<String> = styled_lines.iter().map(|l| l.to_string()).collect();
    (styled_lines, raw_lines)
}

/// Work around tui-markdown 0.3.x panicking on task markers in loose lists.
fn escape_task_list_markers(text: &str) -> Cow<'_, str> {
    if !text.contains("[ ]") && !text.contains("[x]") && !text.contains("[X]") {
        return Cow::Borrowed(text);
    }

    let mut options = MarkdownOptions::empty();
    options.insert(MarkdownOptions::ENABLE_STRIKETHROUGH);
    options.insert(MarkdownOptions::ENABLE_TASKLISTS);
    options.insert(MarkdownOptions::ENABLE_HEADING_ATTRIBUTES);
    options.insert(MarkdownOptions::ENABLE_YAML_STYLE_METADATA_BLOCKS);
    options.insert(MarkdownOptions::ENABLE_SUPERSCRIPT);
    options.insert(MarkdownOptions::ENABLE_SUBSCRIPT);
    let marker_starts: Vec<usize> = Parser::new_ext(text, options)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(event, Event::TaskListMarker(_)).then_some(range.start)
        })
        .collect();
    if marker_starts.is_empty() {
        return Cow::Borrowed(text);
    }

    let mut escaped = String::with_capacity(text.len() + marker_starts.len());
    let mut copied_until = 0;
    for marker_start in marker_starts {
        escaped.push_str(&text[copied_until..marker_start]);
        escaped.push('\\');
        copied_until = marker_start;
    }
    escaped.push_str(&text[copied_until..]);
    Cow::Owned(escaped)
}

fn apply_code_background(lines: &mut [Line<'static>], raw: &[String], theme: &Theme) {
    let code_bg = theme.code_block_bg();
    let code_fg = theme.code_block_fg();

    let mut i = 0;
    while i < raw.len() {
        let trimmed = raw[i].trim();
        if trimmed.starts_with("```") {
            let mut end_marker = None;
            let mut j = i + 1;
            while j < raw.len() {
                if raw[j].trim() == "```" {
                    end_marker = Some(j);
                    break;
                }
                j += 1;
            }

            if let Some(end) = end_marker {
                for line in lines.iter_mut().take(end).skip(i + 1) {
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    for span in &line.spans {
                        let mut style = span.style;
                        if style.fg.is_none() {
                            style = style.fg(code_fg);
                        }
                        style = style.bg(code_bg);
                        spans.push(Span::styled(span.content.clone(), style));
                    }
                    if !spans.is_empty() {
                        *line = Line::from(spans);
                    }
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
    }
}

fn apply_blockquote_indicator(lines: &mut Vec<Line<'static>>, theme: &Theme) {
    let quote_style = Style::new().fg(theme.success);
    for line in lines.iter_mut() {
        if line.style.fg == quote_style.fg && line.style.bg == quote_style.bg {
            let mut spans = vec![Span::styled("▎ ", line.style)];
            spans.extend(std::mem::take(&mut line.spans));
            line.spans = spans;
        }
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
pub(crate) fn format_table(lines: &[String], theme: &Theme) -> (Vec<Line<'static>>, Vec<String>) {
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

        let mut cells = Vec::new();
        for i in 0..col_count {
            let cell = row.get(i).map(|s| s.as_str()).unwrap_or("");
            let width = col_widths.get(i).copied().unwrap_or(0);
            cells.push(pad_cell(cell, width));
        }
        let line_text = format!("|{}|", cells.join("|"));

        let styled = if row_idx == 0 {
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

    (styled_lines, raw_lines)
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

        assert!(joined.contains("[ ] pending"), "{joined}");
        assert!(joined.contains("[x] complete"), "{joined}");
        assert!(joined.contains("[X] ordered"), "{joined}");
    }

    #[test]
    fn render_markdown_preserves_task_marker_inside_fenced_code() {
        let md = "```markdown\n- [ ] literal example\n```";

        let (_lines, raw) = render_markdown_tui(md, &theme());

        assert!(raw.join("\n").contains("- [ ] literal example"));
    }

    #[test]
    fn render_markdown_fenced_code_block() {
        let md = "```rust\nfn md_test() {}\n```";
        let (lines, raw) = render_markdown_tui(md, &theme());
        let joined = raw.join("\n");
        assert!(
            joined.contains("md_test") || joined.contains("fn"),
            "code block content: {joined}"
        );
        assert!(lines.iter().any(|l| !l.spans.is_empty()));
    }

    #[test]
    fn render_markdown_blockquote() {
        let md = "> quoted wisdom";
        let (_lines, raw) = render_markdown_tui(md, &theme());
        let joined = raw.join("\n");
        assert!(
            joined.contains("quoted wisdom"),
            "blockquote text: {joined}"
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
        let (styled, raw) = format_table(&rows, &theme());
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
        let (_styled, raw) = format_table(&rows, &theme());
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
        let (styled, _raw) = format_table(&rows, &theme());
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
}
