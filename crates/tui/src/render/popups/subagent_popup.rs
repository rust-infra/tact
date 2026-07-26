//! Subagent popup: full conversation text during run and after completion.
//!
//! During live execution the content is read from the active tool block's
//! `live_output` and rendered as plain wrapped text. After completion the
//! content is rendered as Markdown (with a one-shot cache in the popup struct).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Scrollbar, ScrollbarState},
};

use super::selectable_text::{layout_display_rows, scalar_styles, source_lines};
use crate::{render::render_md::render_markdown_tui, widgets::state::App};

pub(crate) fn render_subagent_popup(frame: &mut Frame, area: Rect, app: &mut App) {
    // Snapshot what we need before any mutable borrows.
    let popup_snapshot = match app.subagent_popup.as_ref() {
        Some(p) => (
            p.tool_id.clone(),
            p.scroll,
            p.selection,
            p.cached_markdown.clone(),
        ),
        None => return,
    };
    let (tool_id, scroll, selection, cached_markdown) = popup_snapshot;

    let is_live = app.tools.active.iter().any(|a| a.tool_id == tool_id);

    // Get the content: live output or finished detail_full.
    let raw_text = if is_live {
        app.tools
            .active
            .iter()
            .find(|a| a.tool_id == tool_id)
            .map(|a| a.live_output.detail_text())
            .unwrap_or_default()
    } else {
        app.tools
            .blocks
            .iter()
            .find(|b| b.tool_id == tool_id)
            .and_then(|b| b.output.detail_full.clone())
            .unwrap_or_default()
    };

    if raw_text.is_empty() {
        return;
    }

    // Build styled lines: plain during live, markdown after completion.
    let styled_lines: Vec<Line<'static>> = if is_live {
        raw_text
            .lines()
            .map(|l| Line::from(l.to_string()))
            .collect()
    } else {
        if let Some(cached) = cached_markdown {
            cached
        } else {
            let (styled, _) = render_markdown_tui(&raw_text, &app.theme);
            // Store the cache back in the popup for next frame.
            if let Some(popup) = app.subagent_popup.as_mut()
                && popup.tool_id == tool_id
            {
                popup.cached_markdown = Some(styled.clone());
            }
            styled
        }
    };

    let title = match app.subagent_popup.as_ref() {
        Some(p) => p.title.clone(),
        None => return,
    };

    let popup_area = super::centered_popup_area(area);
    let body_area = Rect::new(
        popup_area.x.saturating_add(1),
        popup_area.y.saturating_add(3),
        popup_area.width.saturating_sub(2),
        popup_area.height.saturating_sub(4),
    );

    let source = source_lines(&raw_text);
    let fallback = Style::default()
        .fg(app.theme.fg)
        .bg(app.theme.code_block_bg());
    let mut display_rows = Vec::new();
    for (index, source_line) in source.iter().enumerate() {
        let styles = scalar_styles(
            styled_lines.get(index),
            fallback,
            source_line.text.chars().count(),
        );
        display_rows.extend(layout_display_rows(
            source_line.text,
            source_line.start,
            &styles,
            body_area.width as usize,
            true, // wrap
        ));
    }

    let total = display_rows.len();
    let content_height = body_area.height as usize;
    let max_scroll = total.saturating_sub(content_height);
    let scroll = (scroll as usize).min(max_scroll);

    let code_bg = app.theme.code_block_bg();

    let header = if is_live {
        format!(" {} (live, {} lines) ", title, raw_text.lines().count())
    } else {
        format!(" {} ({} lines) ", title, raw_text.lines().count())
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

    let selection_range = selection.and_then(|sel| sel.normalized_non_empty(&raw_text));

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

    // Update selection_text for copy-with-selection support.
    if let Some(active_popup) = app.subagent_popup.as_mut()
        && active_popup.tool_id == tool_id
    {
        active_popup.selection_text = raw_text;
    }

    app.mouse.subagent_popup_area = popup_area;
    app.mouse.popup_text_body_area = body_area;
    app.mouse.popup_text_hit_rows = hit_rows;
}
