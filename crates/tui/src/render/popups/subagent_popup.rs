//! Subagent popup: full conversation text during run and after completion.
//!
//! During live execution the content is read from the active tool block's
//! `live_output` and rendered as plain wrapped text. After completion the
//! content is rendered as Markdown (with a one-shot cache in the popup struct).
//! Layout is always driven by the text actually shown (plain live text, or the
//! markdown renderer's line text) so styles and grapheme positions stay in sync.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

use super::selectable_text::{PopupLayoutCache, layout_all_display_rows};
use crate::{render::render_md::render_markdown_tui, widgets::state::App};

fn line_text(line: &Line<'_>) -> String {
    line.spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect()
}

pub(crate) fn render_subagent_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    let Some((tool_id, scroll, selection, title)) = app
        .subagent_popup
        .as_ref()
        .map(|p| (p.tool_id.clone(), p.scroll, p.selection, p.title.clone()))
    else {
        return;
    };

    let is_live = app.tools.active.iter().any(|a| a.tool_id == tool_id);
    // Cheap fingerprint (byte length) so we can skip the clone + full re-wrap
    // when neither the content nor the body width changed.
    let content_len = if is_live {
        app.tools
            .active
            .iter()
            .find(|a| a.tool_id == tool_id)
            .map(|a| a.live_output.full_detail_len())
            .unwrap_or(0)
    } else {
        app.tools
            .blocks
            .iter()
            .find(|b| b.tool_id == tool_id)
            .and_then(|b| b.output.detail_full.as_ref().map(String::len))
            .unwrap_or(0)
    };
    if content_len == 0 {
        return;
    }

    let popup_area = super::centered_popup_area(area);
    let body_area = Rect::new(
        popup_area.x.saturating_add(1),
        popup_area.y.saturating_add(3),
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(4),
    );

    let cache_valid = app
        .subagent_popup
        .as_ref()
        .and_then(|p| p.layout_cache.as_ref())
        .is_some_and(|c| c.is_valid(is_live, content_len, body_area.width));
    if !cache_valid {
        rebuild_layout_cache(app, &tool_id, is_live, content_len, body_area.width);
    }

    let Some(cache) = app
        .subagent_popup
        .as_ref()
        .and_then(|p| p.layout_cache.as_ref())
    else {
        return;
    };
    let display_rows = &cache.display_rows;
    let raw_text = &cache.raw_text;

    let total = display_rows.len();
    let content_height = body_area.height as usize;
    let max_scroll = total.saturating_sub(content_height);
    let scroll = (scroll as usize).min(max_scroll);

    let code_bg = app.theme.code_block_bg();

    let header = if is_live {
        format!(" {} (live, {} lines) ", title, cache.line_count)
    } else {
        format!(" {} ({} lines) ", title, cache.line_count)
    };

    let title_style = Style::default()
        .fg(app.theme.code_card_title_fg())
        .add_modifier(Modifier::BOLD);

    frame.render_widget(Clear, popup_area);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(app.theme.block_border_type())
        .border_style(Style::default().fg(app.theme.code_card_border()))
        .title_top(Line::from(Span::styled(&header, title_style)))
        .title_bottom(Line::from(vec![
            Span::styled(
                app.msgs().popup_copy_hint,
                Style::default().fg(app.theme.accent),
            ),
            Span::styled(
                app.msgs().popup_close_hint,
                Style::default().fg(app.theme.accent),
            ),
            Span::styled(
                app.msgs().popup_scroll_hint,
                Style::default().fg(app.theme.accent),
            ),
        ]))
        .style(Style::default().bg(code_bg));

    frame.render_widget(block, popup_area);

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

    app.mouse.subagent_popup_area = popup_area;
    app.mouse.popup_text_body_area = body_area;
    app.mouse.popup_text_hit_rows = hit_rows;
}

/// Clone the current content once, wrap it, and store the result on the popup.
/// Called only when the cached layout is missing or stale (content grew, width
/// changed, or a live→completed transition).
fn rebuild_layout_cache(
    app: &mut App,
    tool_id: &str,
    is_live: bool,
    content_len: usize,
    width: u16,
) {
    let source = if is_live {
        app.tools
            .active
            .iter()
            .find(|a| a.tool_id == tool_id)
            .map(|a| a.live_output.full_detail_text())
            .unwrap_or_default()
    } else {
        app.tools
            .blocks
            .iter()
            .find(|b| b.tool_id == tool_id)
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
    } else if let Some(cached) = app
        .subagent_popup
        .as_ref()
        .and_then(|p| p.cached_markdown.clone())
    {
        let display_text = cached.iter().map(line_text).collect::<Vec<_>>().join("\n");
        (cached, display_text)
    } else {
        let (styled, raw_lines) = render_markdown_tui(&source, &app.theme);
        if let Some(popup) = app.subagent_popup.as_mut().filter(|p| p.tool_id == tool_id) {
            popup.cached_markdown = Some(styled.clone());
        }
        let display_text = raw_lines.join("\n");
        (styled, display_text)
    };

    if display_text.is_empty() {
        return;
    }

    let fallback = Style::default()
        .fg(app.theme.fg)
        .bg(app.theme.code_block_bg());
    let display_rows =
        layout_all_display_rows(&display_text, &styled_lines, fallback, width as usize);
    let line_count = display_text.lines().count();

    if let Some(popup) = app.subagent_popup.as_mut().filter(|p| p.tool_id == tool_id) {
        popup.layout_cache = Some(PopupLayoutCache {
            is_live,
            content_len,
            width,
            raw_text: display_text,
            display_rows,
            line_count,
        });
    }
}
