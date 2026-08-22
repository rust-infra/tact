//! Diff popup renderer — pure render + explicit prepare phase.
//!
//! `prepare_diff_popup` runs the lazy content load (git diff / file read, a
//! side effect) and caches it on the popup state; `render_diff_popup` only
//! reads the cache. The host calls prepare once per frame before rendering.

use std::sync::LazyLock;

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;

use super::PopupMouseSurface;
use crate::{
    render::{
        ctx::RenderCtx,
        selectable_text::{layout_display_rows, scalar_styles, source_lines},
    },
    state::DiffPopup,
    theme::Theme,
};

/// Shared syntax definitions, mirroring what tui-markdown used to load.
static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static THEME_SET: LazyLock<ThemeSet> = LazyLock::new(ThemeSet::load_defaults);

/// Theme key matching tui-markdown's default code theme (Base16 Ocean Dark).
const HIGHLIGHT_THEME: &str = "base16-ocean.dark";

/// Infer a language label from the file extension.
pub fn lang_from_path(path: &str) -> String {
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

/// Run syntect syntax highlighting on raw code text (same syntax set and
/// theme tui-markdown used) and map the styles directly to ratatui spans.
fn syntax_highlight(
    code: &str,
    lang: &str,
    code_fg: ratatui::style::Color,
    code_bg: ratatui::style::Color,
) -> Vec<Line<'static>> {
    let plain = || {
        code.lines()
            .map(|l| {
                Line::from(Span::styled(
                    l.to_string(),
                    Style::default().fg(code_fg).bg(code_bg),
                ))
            })
            .collect()
    };
    if lang.is_empty() {
        return plain();
    }
    let Some(syntax) = SYNTAX_SET.find_syntax_by_token(lang) else {
        return plain();
    };
    let theme = THEME_SET
        .themes
        .get(HIGHLIGHT_THEME)
        .expect("bundled base16-ocean.dark theme");
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut result: Vec<Line<'static>> = Vec::new();
    for line in LinesWithEndings::from(code) {
        let Ok(segments) = highlighter.highlight_line(line, &SYNTAX_SET) else {
            continue;
        };
        let spans: Vec<Span<'static>> = segments
            .into_iter()
            .map(|(syn_style, text)| {
                let mut style = Style::default().bg(code_bg);
                let fg = syn_style.foreground;
                if fg.a > 0 {
                    style = style.fg(Color::Rgb(fg.r, fg.g, fg.b));
                } else {
                    style = style.fg(code_fg);
                }
                let font = syn_style.font_style;
                if font.contains(syntect::highlighting::FontStyle::BOLD) {
                    style = style.add_modifier(Modifier::BOLD);
                }
                if font.contains(syntect::highlighting::FontStyle::ITALIC) {
                    style = style.add_modifier(Modifier::ITALIC);
                }
                if font.contains(syntect::highlighting::FontStyle::UNDERLINE) {
                    style = style.add_modifier(Modifier::UNDERLINED);
                }
                Span::styled(text.to_string(), style)
            })
            .collect();
        result.push(Line::from(spans));
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

/// Lazy-load and cache the popup content (git diff / file read). Side effect
/// phase; runs once per popup (cached on `popup.cached_content`).
pub fn prepare_diff_popup(popup: &mut DiffPopup, theme: &Theme) {
    if popup.cached_content.is_some() {
        return;
    }
    let code_fg = theme.code_block_fg();
    let code_bg = theme.code_block_bg();
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

pub fn render_diff_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let code_bg = ctx.theme.code_block_bg();
    let code_fg = ctx.theme.code_block_fg();
    let line_num_fg = ctx.theme.muted_fg();
    let popup_area = super::centered_popup_area(area);
    let body_area = Rect::new(
        popup_area.x.saturating_add(1),
        popup_area.y.saturating_add(1),
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(3),
    );

    let popup = match ctx.tools.popup.as_ref() {
        Some(p) => p,
        None => return surface,
    };
    let Some(content) = popup.cached_content.as_ref() else {
        let err = if let Some(path) = &popup.file_path {
            ctx.messages.tool_popup_read_error.replace("{}", path)
        } else if let Some(path) = &popup.git_diff_path {
            format!("git diff failed for {}", path)
        } else {
            ctx.messages.tool_popup_empty.to_string()
        };
        let body = Text::from(Line::from(Span::styled(
            err,
            Style::default().fg(ctx.theme.error).bg(code_bg),
        )));
        let inner = super::render_popup_chrome(frame, popup_area, ctx.theme, &popup.title, None);
        frame.render_widget(Paragraph::new(body), inner);
        surface.diff_popup_area = popup_area;
        surface.body_area = body_area;
        return surface;
    };

    let source_lines = source_lines(content);
    let selection = popup
        .selection
        .and_then(|selection| selection.normalized_non_empty(content));
    let total = source_lines.len();
    let content_height = body_area.height as usize;
    let body_width = body_area.width as usize;
    let max_scroll = total.saturating_sub(1);
    let scroll = (popup.scroll as usize).min(max_scroll);

    let title = if popup.is_diff {
        format!(" {} (diff, {} lines) ", popup.title, total)
    } else if popup.lang.is_empty() {
        format!(" {} ({} lines) ", popup.title, total)
    } else {
        format!(" {} ({} lines, {}) ", popup.title, total, popup.lang)
    };

    let mut text = Text::default();
    let mut hit_rows = Vec::new();

    if popup.is_diff {
        // ── native git diff rendering ────────────────────────────────────
        let diff_hunk = ctx.theme.accent; // @@ hunk headers (cyan in dark theme)
        let diff_add = ctx.theme.success; // + lines
        let diff_del = ctx.theme.error; // - lines
        let diff_header = ctx.theme.muted_fg(); // ---/+++ file headers
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
        let gutter_cols = usize::from(popup.use_diff_gutter) * 2;
        let prefix_width = num_width + 2 + gutter_cols;
        let code_width = body_width.saturating_sub(prefix_width + 2);
        let num_style = Style::default().fg(line_num_fg).bg(code_bg);
        let plus_style = Style::default().fg(ctx.theme.success).bg(code_bg);
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
                popup.highlighted_lines.get(i),
                fallback_style,
                source.text.chars().count(),
            );
            let display =
                layout_display_rows(source.text, source.start, &styles, code_width, false)
                    .remove(0);
            let mut spans = vec![Span::styled(format!(" {} ", num), num_style)];
            if popup.use_diff_gutter {
                spans.push(Span::styled("+ ", plus_style));
            }
            spans.extend(display.spans(selection.as_ref()));
            text.push_line(Line::from(spans));

            let screen_y = body_area.y.saturating_add(hit_rows.len() as u16);
            let text_x = body_area.x.saturating_add(prefix_width as u16);
            hit_rows.push(display.hit_row(screen_y, text_x));
        }
    }

    let inner = super::render_popup_chrome(frame, popup_area, ctx.theme, &title, None);
    frame.render_widget(Paragraph::new(text), inner);

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    surface.diff_popup_area = popup_area;
    surface.body_area = body_area;
    surface.hit_rows = hit_rows;
    surface
}
