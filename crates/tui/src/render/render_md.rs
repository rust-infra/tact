use crate::theme::Theme;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

/// Custom StyleSheet that provides dark background and monospace style for code blocks.
#[derive(Clone, Copy, Debug, Default)]
struct TuiStyleSheet;

impl tui_markdown::StyleSheet for TuiStyleSheet {
    fn heading(&self, level: u8) -> Style {
        match level {
            1 => Style::new().on_cyan().bold().underlined(),
            2 => Style::new().cyan().bold(),
            3 => Style::new().cyan().bold().italic(),
            4 => Style::new().light_cyan().italic(),
            5 => Style::new().light_cyan().italic(),
            _ => Style::new().light_cyan().italic(),
        }
    }

    fn code(&self) -> Style {
        Style::new()
            .fg(Color::Rgb(220, 220, 220))
            .bg(Color::Rgb(30, 35, 50))
    }

    fn link(&self) -> Style {
        Style::new().blue().underlined()
    }

    fn blockquote(&self) -> Style {
        Style::new().green()
    }

    fn heading_meta(&self) -> Style {
        Style::new().dim()
    }

    fn metadata_block(&self) -> Style {
        Style::new().light_yellow()
    }
}

/// Renders Markdown text into ratatui Line list and raw text list using tui-markdown.
/// Post-processes code blocks: adds top separator (with language label), line numbers, and bottom separator.
pub(crate) fn render_markdown_tui(text: &str) -> (Vec<Line<'static>>, Vec<String>) {
    // NOTE: Do NOT call process_hyperlinks here — ratatui strips raw ESC sequences
    // (including OSC 8) from Span text, causing broken ]8;; garbage to appear on screen.
    // Plain URLs render fine in the TUI and can be copied via clipboard.
    let options = tui_markdown::Options::new(TuiStyleSheet);
    let tui_text = tui_markdown::from_str_with_options(&text, &options);
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

    // Post-process: apply background to code block content lines
    apply_code_background(&mut styled_lines, &raw_lines);

    let raw_lines: Vec<String> = styled_lines.iter().map(|l| l.to_string()).collect();
    (styled_lines, raw_lines)
}

/// Adds a uniform dark background to code block content lines, preserving tui-markdown's native syntax highlighting.
/// ``` marker lines are kept as-is (rendered by tui-markdown).
fn apply_code_background(lines: &mut Vec<Line<'static>>, raw: &[String]) {
    let code_bg = Color::Rgb(30, 35, 50);
    let code_fg = Color::Rgb(200, 200, 210);

    let mut i = 0;
    while i < raw.len() {
        let trimmed = raw[i].trim();
        if trimmed.starts_with("```") {
            // Find closing ```
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
                // Add background to content lines (``` markers kept as-is)
                for line_idx in (i + 1)..end {
                    let mut spans: Vec<Span<'static>> = Vec::new();
                    for span in &lines[line_idx].spans {
                        let mut style = span.style;
                        if style.fg.is_none() {
                            style = style.fg(code_fg);
                        }
                        style = style.bg(code_bg);
                        spans.push(Span::styled(span.content.clone(), style));
                    }
                    if !spans.is_empty() {
                        lines[line_idx] = Line::from(spans);
                    }
                }
                i = end + 1;
                continue;
            }
        }
        i += 1;
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
            // Strip empty cells caused by leading/trailing |
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

    // Calculate max width per column
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

        // Detect separator row (all cells contain only -, :, and whitespace)
        let is_sep = row.iter().all(|c| {
            c.chars()
                .all(|ch| ch == '-' || ch == ':' || ch.is_whitespace())
        });

        // Skip rendering separator row to prevent data rows from being mistakenly colored Gray
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
