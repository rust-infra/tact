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

fn load_popup_content(popup: &mut crate::widgets::state::DiffPopup) {
    if popup.cached_content.is_some() {
        return;
    }
    let content = if let Some(path) = &popup.file_path {
        std::fs::read_to_string(path)
            .ok()
            .or_else(|| popup.inline_content.clone())
    } else {
        popup.inline_content.clone()
    };
    if let Some(text) = content {
        popup.highlighted_lines = syntax_highlight(&text, &popup.lang);
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
    super::render_popup_shadow(frame, popup_area);

    let para = Paragraph::new(body)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(CODE_BORDER))
                .title(Span::styled(
                    title,
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
    popup_area
}

pub(crate) fn render_diff_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let snapshot = {
        let popup = match app.tools.popup.as_mut() {
            Some(p) => p,
            None => return,
        };
        load_popup_content(popup);
        (
            popup.cached_content.clone(),
            popup.title.clone(),
            popup.file_path.clone(),
            popup.lang.clone(),
            popup.use_diff_gutter,
            popup.scroll,
            popup.highlighted_lines.clone(),
        )
    };

    let (
        cached_content,
        popup_title,
        file_path,
        lang,
        use_diff_gutter,
        scroll,
        highlighted_lines,
    ) = snapshot;

    let Some(content) = cached_content.as_ref() else {
        let err = if let Some(path) = &file_path {
            app.msgs()
                .tool_popup_read_error
                .replace("{}", path)
        } else {
            app.msgs().tool_popup_empty.to_string()
        };
        let body = Text::from(Line::from(Span::styled(
            err,
            Style::default().fg(app.theme.error).bg(CODE_BG),
        )));
        let popup_area = render_popup_chrome(frame, area, app, &popup_title, body);
        app.mouse.diff_popup_area = popup_area;
        return;
    };

    let total = content.lines().count().max(1);
    let content_height = {
        let popup_area = super::centered_popup_area(area);
        popup_area.height.saturating_sub(3) as usize
    };
    let max_scroll = total.saturating_sub(1);
    let scroll = (scroll as usize).min(max_scroll);

    let num_width = (total + 1).to_string().len().max(3);
    let gutter_cols = usize::from(use_diff_gutter) * 2;
    let code_width = {
        let popup_area = super::centered_popup_area(area);
        (popup_area.width as usize).saturating_sub(6 + num_width + gutter_cols)
    };
    let num_style = Style::default().fg(LINE_NUM_FG).bg(CODE_BG);
    let plus_style = Style::default().fg(app.theme.success).bg(CODE_BG);

    let title = if lang.is_empty() {
        format!(" {} ({} lines) ", popup_title, total)
    } else {
        format!(" {} ({} lines, {}) ", popup_title, total, lang)
    };

    let visible_end = (scroll + content_height).min(total);
    let mut text = Text::default();

    for i in scroll..visible_end {
        let num = format!("{:>nw$}", i + 1, nw = num_width);

        let content_line: Line<'static> = if i < highlighted_lines.len() {
            let hl_spans: Vec<Span<'static>> = highlighted_lines[i]
                .spans
                .iter()
                .map(|s| {
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
            let mut spans = vec![Span::styled(format!(" {} ", num), num_style)];
            if use_diff_gutter {
                spans.push(Span::styled("+ ", plus_style));
            }
            text.push_line(Line::from(spans));
        } else {
            let mut spans = vec![Span::styled(format!(" {} ", num), num_style)];
            if use_diff_gutter {
                spans.push(Span::styled("+ ", plus_style));
            }
            spans.extend(content_line.spans);
            text.push_line(Line::from(spans));
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
}

pub(crate) fn popup_lang_for_path(path: &str) -> String {
    lang_from_path(path)
}
