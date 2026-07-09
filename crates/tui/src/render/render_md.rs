use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

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
    let tui_text = tui_markdown::from_str_with_options(text, &options);
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

/// Parses Markdown table raw lines into column-aligned ratatui Lines.
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
        for (i, cell) in row.iter().enumerate() {
            if i < col_widths.len() {
                col_widths[i] = col_widths[i].max(cell.len());
            }
        }
    }

    let mut styled_lines = Vec::new();
    let mut raw_lines = Vec::new();

    for (row_idx, row) in rows.iter().enumerate() {
        let mut cells = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            let width = col_widths.get(i).copied().unwrap_or(0);
            cells.push(format!(" {:width$} ", cell, width = width));
        }
        let line_text = format!("|{}|", cells.join("|"));

        let is_sep = row.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        });

        if is_sep {
            continue;
        }

        let style = if row_idx == 0 {
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(theme.accent)
        } else {
            Style::default().fg(theme.fg)
        };

        styled_lines.push(Line::from(Span::styled(line_text.clone(), style)));
        raw_lines.push(line_text);
    }

    (styled_lines, raw_lines)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    fn theme() -> Theme {
        Theme::by_name_str("retro")
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
    }
}
