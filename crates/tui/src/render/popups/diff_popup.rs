use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

use super::selectable_text::{layout_display_rows, scalar_styles, source_lines};
use crate::widgets::state::App;

/// Infer a language label from the file extension.
fn lang_from_path(path: &str) -> String {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("rs") => "rust".to_string(),
        Some("py") => "python".to_string(),
        Some("js") | Some("mjs") => "javascript".to_string(),
        Some("ts") | Some("tsx") => "typescript".to_string(),
        Some("go") => "go".to_string(),
        Some("c") | Some("h") => "c".to_string(),
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "cpp".to_string(),
        Some("toml") => "toml".to_string(),
        Some("yaml") | Some("yml") => "yaml".to_string(),
        Some("json") => "json".to_string(),
        Some("md") | Some("mdx") => "markdown".to_string(),
        Some("sh") | Some("bash") | Some("zsh") => "bash".to_string(),
        Some("sql") => "sql".to_string(),
        Some("html") => "html".to_string(),
        Some("css") => "css".to_string(),
        Some("java") => "java".to_string(),
        Some("kt") | Some("kts") => "kotlin".to_string(),
        Some("swift") => "swift".to_string(),
        _ => String::new(),
    }
}

/// Run tui-markdown (syntect) syntax highlighting on raw code text.
fn syntax_highlight(
    code: &str,
    lang: &str,
    code_fg: ratatui::style::Color,
    code_bg: ratatui::style::Color,
) -> Vec<Line<'static>> {
    if lang.is_empty() {
        return code
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(code_fg).bg(code_bg),
                ))
            })
            .collect();
    }

    let md = format!("```{lang}\n{code}\n```");
    let styled = tui_markdown::from_str(&md);

    let mut in_code = false;
    let mut result: Vec<Line<'static>> = Vec::new();
    for line in &styled.lines {
        let raw: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        let trimmed = raw.trim();
        if trimmed.starts_with("```") {
            if in_code {
                break;
            }
            in_code = true;
            continue;
        }
        if in_code {
            let spans: Vec<Span<'static>> = line
                .spans
                .iter()
                .map(|s| {
                    let mut style = s.style;
                    if style.fg.is_none() {
                        style = style.fg(code_fg);
                    }
                    style = style.bg(code_bg);
                    Span::styled(s.content.clone().into_owned(), style)
                })
                .collect();
            result.push(Line::from(spans));
        }
    }
    result
}

fn run_git_diff(workspace_dir: Option<&str>, path: &str) -> Option<String> {
    let mut cmd = std::process::Command::new("git");
    cmd.arg("diff").arg("--").arg(path);
    if let Some(cwd) = workspace_dir {
        cmd.current_dir(cwd);
    }
    let output = cmd.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    if text.is_empty() {
        return None;
    }
    Some(text)
}

fn load_popup_content(
    popup: &mut crate::widgets::state::DiffPopup,
    code_fg: ratatui::style::Color,
    code_bg: ratatui::style::Color,
) {
    if popup.cached_content.is_some() {
        return;
    }
    let content = if let Some(path) = &popup.git_diff_path {
        run_git_diff(popup.workspace_dir.as_deref(), path).or_else(|| {
            // git diff failed – fall back to inline content as plain text
            popup.is_diff = false;
            popup.inline_content.clone()
        })
    } else if let Some(path) = &popup.file_path {
        std::fs::read_to_string(path)
            .ok()
            .or_else(|| popup.inline_content.clone())
    } else {
        popup.inline_content.clone()
    };
    if let Some(text) = content {
        if popup.is_diff {
            // Don't syntax-highlight diff output; render natively in render_diff_popup.
            popup.highlighted_lines = Vec::new();
        } else {
            popup.highlighted_lines = syntax_highlight(&text, &popup.lang, code_fg, code_bg);
        }
        popup.cached_content = Some(text);
    }
}

fn render_popup_chrome(
    frame: &mut Frame,
    area: Rect,
    app: &App,
    title: &str,
    body: Text<'static>,
) -> Rect {
    let popup_area = super::centered_popup_area(area);
    frame.render_widget(Clear, popup_area);

    let code_bg = app.theme.code_block_bg();

    let para = Paragraph::new(body).block(
        Block::default()
            .borders(Borders::ALL)
            .border_type(app.theme.block_border_type())
            .border_style(Style::default().fg(app.theme.code_card_border()))
            .title(Span::styled(
                title,
                Style::default()
                    .fg(app.theme.code_card_title_fg())
                    .add_modifier(Modifier::BOLD),
            ))
            .title_bottom(Line::from(vec![
                Span::styled(
                    app.msgs().popup_copy_hint,
                    Style::default().fg(app.theme.accent),
                ),
                Span::styled(
                    app.msgs().popup_scroll_hint,
                    Style::default().fg(app.theme.accent),
                ),
                Span::styled(
                    app.msgs().popup_close_hint,
                    Style::default().fg(app.theme.accent),
                ),
            ]))
            .style(Style::default().bg(code_bg)),
    );

    frame.render_widget(para, popup_area);
    popup_area
}

pub(crate) fn render_diff_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let code_bg = app.theme.code_block_bg();
    let code_fg = app.theme.code_block_fg();
    let line_num_fg = app.theme.muted_fg();
    let popup_area = super::centered_popup_area(area);
    let body_area = Rect::new(
        popup_area.x.saturating_add(1),
        popup_area.y.saturating_add(1),
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(3),
    );

    let snapshot = {
        let popup = match app.tools.popup.as_mut() {
            Some(p) => p,
            None => return,
        };
        load_popup_content(popup, code_fg, code_bg);
        (
            popup.cached_content.clone(),
            popup.title.clone(),
            popup.file_path.clone(),
            popup.git_diff_path.clone(),
            popup.lang.clone(),
            popup.use_diff_gutter,
            popup.is_diff,
            popup.scroll,
            popup.selection,
            popup.highlighted_lines.clone(),
        )
    };

    let (
        cached_content,
        popup_title,
        file_path,
        git_diff_path,
        lang,
        use_diff_gutter,
        is_diff,
        scroll,
        selection,
        highlighted_lines,
    ) = snapshot;

    let Some(content) = cached_content.as_ref() else {
        let err = if let Some(path) = &file_path {
            app.msgs().tool_popup_read_error.replace("{}", path)
        } else if let Some(path) = &git_diff_path {
            format!("git diff failed for {}", path)
        } else {
            app.msgs().tool_popup_empty.to_string()
        };
        let body = Text::from(Line::from(Span::styled(
            err,
            Style::default().fg(app.theme.error).bg(code_bg),
        )));
        let popup_area = render_popup_chrome(frame, area, app, &popup_title, body);
        app.mouse.diff_popup_area = popup_area;
        app.mouse.popup_text_body_area = body_area;
        app.mouse.popup_text_hit_rows.clear();
        return;
    };

    let source_lines = source_lines(content);
    let selection = selection.and_then(|selection| selection.normalized_non_empty(content));
    let total = source_lines.len();
    let content_height = body_area.height as usize;
    let body_width = body_area.width as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (scroll as usize).min(max_scroll);

    let title = if is_diff {
        format!(" {} (diff, {} lines) ", popup_title, total)
    } else if lang.is_empty() {
        format!(" {} ({} lines) ", popup_title, total)
    } else {
        format!(" {} ({} lines, {}) ", popup_title, total, lang)
    };

    let mut text = Text::default();
    let mut hit_rows = Vec::new();

    if is_diff {
        // ── native git diff rendering ────────────────────────────────────
        let diff_hunk = app.theme.accent; // @@ hunk headers (cyan in dark theme)
        let diff_add = app.theme.success; // + lines
        let diff_del = app.theme.error; // - lines
        let diff_header = app.theme.muted_fg(); // ---/+++ file headers
        let diff_context = code_fg; // context lines (starting with space)

        'source: for source in source_lines.iter().skip(scroll) {
            debug_assert_eq!(source.end, source.start + source.text.len());
            let prefix = source.text.chars().next().unwrap_or(' ');

            let (fg, line_style) = match prefix {
                '@' => (diff_hunk, Modifier::BOLD),
                '+' => (diff_add, Modifier::empty()),
                '-' => (diff_del, Modifier::empty()),
                ' ' => (diff_context, Modifier::empty()),
                _ => (diff_header, Modifier::empty()),
            };

            let style = Style::default().fg(fg).bg(code_bg).add_modifier(line_style);
            let styles = vec![style; source.text.chars().count()];
            for display in layout_display_rows(source.text, source.start, &styles, body_width, true)
            {
                if hit_rows.len() >= content_height {
                    break 'source;
                }
                let screen_y = body_area.y.saturating_add(hit_rows.len() as u16);
                hit_rows.push(display.hit_row(screen_y, body_area.x));
                text.push_line(Line::from(display.spans(selection.as_ref())));
            }
        }
    } else {
        // ── plain code rendering with line numbers ───────────────────────
        let num_width = (total + 1).to_string().len().max(3);
        let gutter_cols = usize::from(use_diff_gutter) * 2;
        let prefix_width = num_width + 2 + gutter_cols;
        let code_width = body_width.saturating_sub(prefix_width + 2);
        let num_style = Style::default().fg(line_num_fg).bg(code_bg);
        let plus_style = Style::default().fg(app.theme.success).bg(code_bg);
        let fallback_style = Style::default().fg(code_fg).bg(code_bg);

        for (i, source) in source_lines
            .iter()
            .enumerate()
            .skip(scroll)
            .take(content_height)
        {
            debug_assert_eq!(source.end, source.start + source.text.len());
            let num = format!("{:>nw$}", i + 1, nw = num_width);
            let styles = scalar_styles(
                highlighted_lines.get(i),
                fallback_style,
                source.text.chars().count(),
            );
            let display =
                layout_display_rows(source.text, source.start, &styles, code_width, false)
                    .remove(0);
            let mut spans = vec![Span::styled(format!(" {} ", num), num_style)];
            if use_diff_gutter {
                spans.push(Span::styled("+ ", plus_style));
            }
            spans.extend(display.spans(selection.as_ref()));
            text.push_line(Line::from(spans));

            let screen_y = body_area.y.saturating_add(hit_rows.len() as u16);
            let text_x = body_area.x.saturating_add(prefix_width as u16);
            hit_rows.push(display.hit_row(screen_y, text_x));
        }
    }

    let popup_area = render_popup_chrome(frame, area, app, &title, text);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    app.mouse.diff_popup_area = popup_area;
    app.mouse.popup_text_body_area = body_area;
    app.mouse.popup_text_hit_rows = hit_rows;
}

pub(crate) fn popup_lang_for_path(path: &str) -> String {
    lang_from_path(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::widgets::state::PopupTextHit;

    fn test_hit_rows(
        text: &str,
        line_start: usize,
        text_x: u16,
        max_width: usize,
        wrap: bool,
    ) -> Vec<crate::widgets::state::PopupHitRow> {
        layout_display_rows(text, line_start, &[], max_width, wrap)
            .into_iter()
            .enumerate()
            .map(|(row, display)| display.hit_row(row as u16, text_x))
            .collect()
    }

    #[test]
    fn hit_map_resolves_ascii_cells_and_clamps_outside_text() {
        let row = test_hit_rows("abc", 5, 7, 20, false).remove(0);

        assert_eq!(row.hit(6), PopupTextHit::empty(5));
        assert_eq!(row.hit(7), PopupTextHit::new(5, 6));
        assert_eq!(row.hit(8), PopupTextHit::new(6, 7));
        assert_eq!(row.hit(9), PopupTextHit::new(7, 8));
        assert_eq!(row.hit(10), PopupTextHit::empty(8));
    }

    #[test]
    fn hit_map_repeats_wide_scalar_span_for_each_screen_cell() {
        let row = test_hit_rows("界x", 10, 8, 20, false).remove(0);

        assert_eq!(row.hit(8), PopupTextHit::new(10, 13));
        assert_eq!(row.hit(9), PopupTextHit::new(10, 13));
        assert_eq!(row.hit(10), PopupTextHit::new(13, 14));
    }

    #[test]
    fn hit_map_treats_emoji_presentation_sequence_as_one_grapheme() {
        let text = "a⌨️b";
        let row = test_hit_rows(text, 0, 4, 20, false).remove(0);

        assert_eq!(row.hit(4), PopupTextHit::new(0, 1));
        assert_eq!(row.hit(5), PopupTextHit::new(1, 7));
        assert_eq!(row.hit(6), PopupTextHit::new(1, 7));
        assert_eq!(row.hit(7), PopupTextHit::new(7, 8));
    }

    #[test]
    fn hit_map_treats_zwj_emoji_sequence_as_one_grapheme() {
        let text = "a👩‍💻b";
        let row = test_hit_rows(text, 0, 4, 20, false).remove(0);

        assert_eq!(row.hit(4), PopupTextHit::new(0, 1));
        assert_eq!(row.hit(5), PopupTextHit::new(1, 12));
        assert_eq!(row.hit(6), PopupTextHit::new(1, 12));
        assert_eq!(row.hit(7), PopupTextHit::new(12, 13));
    }

    #[test]
    fn hit_map_merges_trailing_zero_width_sequence_into_previous_cell() {
        let text = "a\u{0301}\u{0327}界z";
        let row = test_hit_rows(text, 4, 3, 20, false).remove(0);

        assert_eq!(row.hit(3), PopupTextHit::new(4, 9));
        assert_eq!(row.hit(4), PopupTextHit::new(9, 12));
        assert_eq!(row.hit(5), PopupTextHit::new(9, 12));
        assert_eq!(row.hit(6), PopupTextHit::new(12, 13));
        for hit in &row.cells {
            assert!(text.is_char_boundary(hit.start - 4));
            assert!(text.is_char_boundary(hit.end - 4));
        }
    }

    #[test]
    fn hit_map_merges_leading_zero_width_sequence_into_first_cell() {
        let text = "\u{0301}\u{0327}a界";
        let row = test_hit_rows(text, 10, 5, 20, false).remove(0);

        assert_eq!(row.hit(4), PopupTextHit::empty(10));
        assert_eq!(row.hit(5), PopupTextHit::new(10, 15));
        assert_eq!(row.hit(6), PopupTextHit::new(15, 18));
        assert_eq!(row.hit(7), PopupTextHit::new(15, 18));
        assert_eq!(row.hit(8), PopupTextHit::empty(18));
        for hit in &row.cells {
            assert!(text.is_char_boundary(hit.start - 10));
            assert!(text.is_char_boundary(hit.end - 10));
        }
    }

    #[test]
    fn hit_map_empty_row_clamps_to_its_source_offset() {
        let row = test_hit_rows("", 12, 5, 20, false).remove(0);

        assert!(row.cells.is_empty());
        assert_eq!(row.hit(4), PopupTextHit::empty(12));
        assert_eq!(row.hit(5), PopupTextHit::empty(12));
        assert_eq!(row.hit(50), PopupTextHit::empty(12));
    }

    #[test]
    fn hit_map_excludes_number_and_diff_gutter() {
        let row = test_hit_rows("界x", 10, 8, 20, false).remove(0);

        assert_eq!(row.hit(7), PopupTextHit::empty(10));
        assert_eq!(row.hit(8), PopupTextHit::new(10, 13));
        assert_eq!(row.hit(9), PopupTextHit::new(10, 13));
        assert_eq!(row.hit(10), PopupTextHit::new(13, 14));
    }

    #[test]
    fn hit_map_wraps_unified_diff_rows_at_display_width() {
        let rows = test_hit_rows("+ab界cd", 20, 2, 4, true);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].line_start, 20);
        assert_eq!(rows[0].line_end, 23);
        assert_eq!(rows[0].hit(4), PopupTextHit::new(22, 23));
        assert_eq!(rows[0].hit(5), PopupTextHit::empty(23));
        assert_eq!(rows[1].line_start, 23);
        assert_eq!(rows[1].line_end, 28);
        assert_eq!(rows[1].hit(2), PopupTextHit::new(23, 26));
        assert_eq!(rows[1].hit(3), PopupTextHit::new(23, 26));
        assert_eq!(rows[1].hit(4), PopupTextHit::new(26, 27));
        assert_eq!(rows[1].hit(6), PopupTextHit::empty(28));
    }

    #[test]
    fn hit_map_wraps_only_between_extended_grapheme_clusters() {
        let rows = test_hit_rows("a👩‍💻b", 20, 2, 3, true);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].line_start, 20);
        assert_eq!(rows[0].line_end, 32);
        assert_eq!(rows[0].hit(2), PopupTextHit::new(20, 21));
        assert_eq!(rows[0].hit(3), PopupTextHit::new(21, 32));
        assert_eq!(rows[0].hit(4), PopupTextHit::new(21, 32));
        assert_eq!(rows[1].line_start, 32);
        assert_eq!(rows[1].hit(2), PopupTextHit::new(32, 33));
    }

    #[test]
    fn styled_span_layout_truncates_at_aggregate_grapheme_width() {
        let red = Style::default().fg(ratatui::style::Color::Red);
        let blue = Style::default().fg(ratatui::style::Color::Blue);
        let green = Style::default().fg(ratatui::style::Color::Green);
        let line = Line::from(vec![
            Span::styled("ab", red),
            Span::styled("👩‍💻", blue),
            Span::styled("c", green),
        ]);
        let styles = scalar_styles(Some(&line), Style::default(), 7);

        let display = layout_display_rows("ab👩‍💻c", 10, &styles, 4, false).remove(0);
        let spans = display.spans(None);

        assert_eq!(
            spans
                .iter()
                .map(|span| span.content.as_ref())
                .collect::<String>(),
            "ab👩‍💻"
        );
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].style, red);
        assert_eq!(spans[1].style, blue);
        assert_eq!(display.cells.len(), 4);
        assert_eq!(display.cells[2], PopupTextHit::new(12, 23));
        assert_eq!(display.cells[3], PopupTextHit::new(12, 23));
        assert_eq!(display.line_end, 23);
    }
}
