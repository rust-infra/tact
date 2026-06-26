use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarState, Wrap},
};

/// Dark background shared with code blocks / markdown rendering.
const CODE_BG: Color = Color::Rgb(30, 35, 50);
const CODE_BORDER: Color = Color::Rgb(100, 120, 180);
const LINE_NUM_FG: Color = Color::Rgb(100, 110, 130);
const CODE_FG: Color = Color::Rgb(200, 200, 210);

/// Infer a language label from the file extension.
fn lang_from_path(path: &str) -> &str {
    match std::path::Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
    {
        Some("rs") => "rust",
        Some("py") => "python",
        Some("js") | Some("mjs") => "javascript",
        Some("ts") | Some("tsx") => "typescript",
        Some("go") => "go",
        Some("c") | Some("h") => "c",
        Some("cpp") | Some("cc") | Some("cxx") | Some("hpp") => "cpp",
        Some("toml") => "toml",
        Some("yaml") | Some("yml") => "yaml",
        Some("json") => "json",
        Some("md") | Some("mdx") => "markdown",
        Some("sh") | Some("bash") | Some("zsh") => "bash",
        Some("sql") => "sql",
        Some("html") => "html",
        Some("css") => "css",
        Some("java") => "java",
        Some("kt") | Some("kts") => "kotlin",
        Some("swift") => "swift",
        _ => "",
    }
}

/// Run tui-markdown (syntect) syntax highlighting on raw code text.
fn syntax_highlight(code: &str, lang: &str) -> Vec<Line<'static>> {
    if lang.is_empty() {
        return code
            .lines()
            .map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(CODE_FG).bg(CODE_BG),
                ))
            })
            .collect();
    }

    let md = format!("```{}\n{}\n```", lang, code);
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
                        style = style.fg(CODE_FG);
                    }
                    style = style.bg(CODE_BG);
                    Span::styled(s.content.clone().into_owned(), style)
                })
                .collect();
            result.push(Line::from(spans));
        }
    }
    result
}

pub(crate) fn render_diff_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let file_path = match app.diff_popup.as_ref().map(|p| p.file_path.clone()) {
        Some(p) => p,
        None => return,
    };

    let popup = app.diff_popup.as_mut().unwrap();

    // ------------------------------------------------------------------
    // Lazy-load file content and syntax-highlight it (once).
    // ------------------------------------------------------------------
    if popup.cached_content.is_none() {
        popup.cached_content = std::fs::read_to_string(&file_path).ok();
        // Re-highlight whenever content is (re)loaded.
        if let Some(content) = &popup.cached_content {
            let lang = lang_from_path(&file_path);
            popup.highlighted_lines = syntax_highlight(content, lang);
        }
    }

    let content = match &popup.cached_content {
        Some(c) => c,
        None => {
            let err = format!("Unable to read file: {}", file_path);
            let para = Paragraph::new(err).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .title(app.msgs().diff_popup_title.replace("{}", &file_path)),
            );
            frame.render_widget(para, area);
            return;
        }
    };

    let total = content.lines().count();
    if total == 0 {
        return;
    }

    // ------------------------------------------------------------------
    // Layout geometry (same every frame for the same area — fine).
    // ------------------------------------------------------------------
    let popup_width = (area.width as f32 * 0.8).max(40.0) as u16;
    let popup_height = (area.height as f32 * 0.8).max(10.0) as u16;
    let popup_x = area.x + (area.width.saturating_sub(popup_width)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_height)) / 2;
    let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

    frame.render_widget(Clear, popup_area);
    super::render_popup_shadow(frame, popup_area);

    let content_height = popup_height.saturating_sub(3) as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);

    let num_width = (total + 1).to_string().len().max(3);
    // │ NNN + content… │  →  borders(2) + spaces(2) + num_width + gutter(2) = num_width + 6
    let code_width = (popup_width as usize).saturating_sub(6 + num_width);
    let num_style = Style::default().fg(LINE_NUM_FG).bg(CODE_BG);
    let plus_style = Style::default().fg(app.theme.success).bg(CODE_BG);

    let lang = lang_from_path(&file_path);

    // Title: file name + line count + optional language.
    let title = if lang.is_empty() {
        format!(" {} ({} lines) ", file_path, total)
    } else {
        format!(" {} ({} lines, {}) ", file_path, total, lang)
    };

    // ------------------------------------------------------------------
    // Build the visible lines — reuse pre-rendered highlighted lines.
    // Truncation reuses the existing Span content without allocating.
    // ------------------------------------------------------------------
    let visible_end = (scroll + content_height).min(total);
    let mut text = Text::default();
    let highlighted = &popup.highlighted_lines;

    for i in scroll..visible_end {
        let num = format!("{:>nw$}", i + 1, nw = num_width);

        let content_line: Line<'static> = if i < highlighted.len() {
            let hl_spans: Vec<Span<'static>> = highlighted[i]
                .spans
                .iter()
                .map(|s| {
                    // Truncate inline: take at most code_width chars from the
                    // existing Span content (no allocation for short lines;
                    // allocate only for long ones that actually get cut).
                    if s.content.chars().count() <= code_width {
                        Span::styled(s.content.clone().into_owned(), s.style)
                    } else {
                        Span::styled(
                            s.content.chars().take(code_width).collect::<String>(),
                            s.style,
                        )
                    }
                })
                .collect();
            Line::from(hl_spans)
        } else {
            // Fallback (shouldn't normally happen when highlighted is kept in
            // sync): render raw line.
            let raw: String = content
                .lines()
                .nth(i)
                .unwrap_or("")
                .chars()
                .take(code_width)
                .collect();
            Line::from(Span::styled(raw, Style::default().fg(CODE_FG).bg(CODE_BG)))
        };

        if content_line.spans.is_empty() {
            text.push_line(Line::from(vec![
                Span::styled(format!(" {} ", num), num_style),
                Span::styled("+ ", plus_style),
            ]));
        } else {
            let mut spans = vec![
                Span::styled(format!(" {} ", num), num_style),
                Span::styled("+ ", plus_style),
            ];
            spans.extend(content_line.spans);
            text.push_line(Line::from(spans));
        }
    }

    let para = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(CODE_BORDER))
                .title(Span::styled(
                    &title,
                    Style::default()
                        .fg(Color::Rgb(160, 180, 240))
                        .add_modifier(Modifier::BOLD),
                ))
                .title_bottom(Line::from(vec![
                    Span::styled(" y ", Style::default().fg(app.theme.accent)),
                    Span::styled(
                        app.msgs().popup_copy_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                    Span::styled(" j/k ", Style::default().fg(app.theme.accent)),
                    Span::styled(
                        app.msgs().popup_scroll_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                    Span::styled(" Esc ", Style::default().fg(app.theme.accent)),
                    Span::styled(
                        app.msgs().popup_close_hint,
                        Style::default().fg(app.theme.accent),
                    ),
                ]))
                .style(Style::default().bg(CODE_BG)),
        )
        .wrap(Wrap { trim: false });

    frame.render_widget(para, popup_area);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    app.mouse.diff_popup_area = popup_area;
}