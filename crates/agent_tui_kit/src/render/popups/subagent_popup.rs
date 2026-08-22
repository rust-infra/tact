//! Subagent popup: full conversation text during run and after completion.
//!
//! During live execution the content is read from the active tool block's
//! `live_output` and rendered as plain wrapped text. After completion the
//! content is rendered as Markdown (with a one-shot cache in the popup struct).
//! Layout is always driven by the text actually shown (plain live text, or the
//! markdown renderer's line text) so styles and grapheme positions stay in sync.
//!
//! `prepare_subagent_popup` rebuilds the layout cache (side effect on the
//! popup state); `render_subagent_popup` only reads it.

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::Line,
    widgets::{Paragraph, Scrollbar, ScrollbarState},
};

use super::PopupMouseSurface;
use crate::{
    render::{
        ctx::RenderCtx,
        render_md::render_markdown_tui,
        selectable_text::{PopupLayoutCache, layout_all_display_rows},
    },
    state::{SubagentPopup, ToolState},
    theme::Theme,
};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

/// Rebuild the popup's layout cache when missing or stale (content grew,
/// width changed, or a live→completed transition). Side-effect phase.
pub fn prepare_subagent_popup(
    popup: &mut SubagentPopup,
    tools: &ToolState,
    theme: &Theme,
    body_width: u16,
) {
    let tool_id = &popup.tool_id;
    let is_live = tools.active.iter().any(|a| a.tool_id == *tool_id);
    // Cheap fingerprint (byte length) so we can skip the clone + full re-wrap
    // when neither the content nor the body width changed.
    let content_len = if is_live {
        tools
            .active
            .iter()
            .find(|a| a.tool_id == *tool_id)
            .map(|a| a.live_output.full_detail_len())
            .unwrap_or(0)
    } else {
        tools
            .blocks
            .iter()
            .find(|b| b.tool_id == *tool_id)
            .and_then(|b| b.output.detail_full.as_ref().map(String::len))
            .unwrap_or(0)
    };
    if content_len == 0 {
        return;
    }

    let cache_valid = popup
        .layout_cache
        .as_ref()
        .is_some_and(|c| c.is_valid(is_live, content_len, body_width));
    if cache_valid {
        return;
    }

    let source = if is_live {
        tools
            .active
            .iter()
            .find(|a| a.tool_id == *tool_id)
            .map(|a| a.live_output.full_detail_text())
            .unwrap_or_default()
    } else {
        tools
            .blocks
            .iter()
            .find(|b| b.tool_id == *tool_id)
            .and_then(|b| b.output.detail_full.clone())
            .unwrap_or_default()
    };
    if source.is_empty() {
        return;
    }

    // Live: plain lines. Completed: markdown render — layout must use the
    // rendered line text (not the markdown source), matching thinking_popup,
    // otherwise style spans and grapheme positions drift apart.
    let (styled_lines, display_text) = if is_live {
        let styled: Vec<Line<'static>> =
            source.lines().map(|l| Line::from(l.to_string())).collect();
        (styled, source)
    } else if let Some(cached) = popup.cached_markdown.clone() {
        let display_text = cached.iter().map(line_text).collect::<Vec<_>>().join("\n");
        (cached, display_text)
    } else {
        let (styled, raw_lines) = render_markdown_tui(&source, theme);
        popup.cached_markdown = Some(styled.clone());
        let display_text = raw_lines.join("\n");
        (styled, display_text)
    };

    if display_text.is_empty() {
        return;
    }

    let fallback = Style::default().fg(theme.fg).bg(theme.code_block_bg());
    let display_rows =
        layout_all_display_rows(&display_text, &styled_lines, fallback, body_width as usize);
    let line_count = display_text.lines().count();

    popup.layout_cache = Some(PopupLayoutCache {
        is_live,
        content_len,
        width: body_width,
        raw_text: display_text,
        display_rows,
        line_count,
    });
}

pub fn render_subagent_popup(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> PopupMouseSurface {
    let mut surface = PopupMouseSurface::default();
    let Some((scroll, selection, title)) = ctx
        .subagent_popup
        .map(|p| (p.scroll, p.selection, p.title.clone()))
    else {
        return surface;
    };

    let popup = match ctx.subagent_popup {
        Some(p) => p,
        None => return surface,
    };
    let is_live = ctx.tools.active.iter().any(|a| a.tool_id == popup.tool_id);

    let popup_area = super::centered_popup_area(area);
    let body_area = super::popup_inner(popup_area);

    let Some(cache) = popup.layout_cache.as_ref() else {
        return surface;
    };
    if !cache.is_valid(is_live, cache.content_len, body_area.width) {
        return surface;
    }
    let display_rows = &cache.display_rows;
    let raw_text = &cache.raw_text;

    let total = display_rows.len();
    let content_height = body_area.height as usize;
    let max_scroll = total.saturating_sub(content_height);
    let scroll = (scroll as usize).min(max_scroll);

    let header = if is_live {
        format!(" {} (live, {} lines) ", title, cache.line_count)
    } else {
        format!(" {} ({} lines) ", title, cache.line_count)
    };

    let footer: &[super::FooterHint] = &[
        super::FooterHint {
            key: "y",
            label: " copy ",
        },
        super::FooterHint {
            key: "Esc",
            label: " close ",
        },
        super::FooterHint {
            key: "j/k",
            label: " scroll ",
        },
    ];
    let inner = super::render_popup_chrome(frame, popup_area, ctx.theme, &header, Some(footer));
    let body_area = inner;

    let selection_range = selection.and_then(|sel| sel.normalized_non_empty(raw_text));

    let mut hit_rows = Vec::new();
    for (visible_row, display) in display_rows
        .iter()
        .skip(scroll)
        .take(content_height)
        .enumerate()
    {
        let screen_y = body_area.y.saturating_add(visible_row as u16);
        frame.render_widget(
            Paragraph::new(Line::from(display.spans(selection_range.as_ref()))),
            Rect::new(body_area.x, screen_y, body_area.width, 1),
        );
        hit_rows.push(display.hit_row(screen_y, body_area.x));
    }

    let scrollbar =
        Scrollbar::default().orientation(ratatui::widgets::ScrollbarOrientation::VerticalRight);
    let mut state = ScrollbarState::new(total)
        .viewport_content_length(content_height)
        .position(scroll);
    frame.render_stateful_widget(scrollbar, popup_area, &mut state);

    surface.subagent_popup_area = popup_area;
    surface.body_area = body_area;
    surface.hit_rows = hit_rows;
    surface
}
