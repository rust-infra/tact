//! Subagent view — app-layer wrapper.
//!
//! Renders the child conversation as a full-main-area view with a breadcrumb
//! header, not an overlay popup: the child runs a whole agent loop, so it
//! deserves the whole area. Tool blocks inside stay self-contained (no
//! `[ Open ]`), so nothing here opens a second overlay; `Enter` raises the cap
//! on a block's output instead.

use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::widgets::state::{App, SurfaceId};

/// Render the subagent as a full-main-area view (not an overlay).
///
/// Layout: one breadcrumb row on top, scrollable body + scrollbar below.
pub(crate) fn render_subagent_view(frame: &mut Frame, area: Rect, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(1)])
        .split(area);

    // Reserve one column for the scrollbar when the terminal is wide enough.
    let main_area = chunks[1];
    let scrollbar_w = if main_area.width > 1 { 1 } else { 0 };
    let body_area = Rect::new(
        main_area.x,
        main_area.y,
        main_area.width.saturating_sub(scrollbar_w),
        main_area.height,
    );

    // Prepare: rebuild layout when transcript grew, width changed, etc.
    // Use the narrowed body width so the layout matches the rendered area.
    let messages = app.msgs();
    if let Some(id) = app.active_subagent_popup.clone()
        && let Some(popup) = app.subagent_popups.get_mut(&id)
    {
        let tools_state = app
            .registry
            .get::<agent_tui_kit::components::ToolComponent>()
            .expect("tool component registered")
            .state();
        agent_tui_kit::render::popups::subagent_popup::prepare_subagent_popup(
            popup,
            tools_state,
            &app.theme,
            &messages,
            body_area.width,
        );
        // Keep the stored scroll honest, the way the log does with
        // `visual_top`: `G` parks it at `u16::MAX`, and a value that is only
        // clamped at draw time never comes back inside the range — the next `k`
        // would scroll from 65535 and look broken. `body_area.height` is the
        // same viewport the body renderer slices with.
        if let Some(layout) = popup.layout.as_ref() {
            let max_scroll = layout.total_rows.saturating_sub(body_area.height as usize);
            popup.scroll = (popup.scroll as usize)
                .min(max_scroll)
                .min(u16::MAX as usize) as u16;
        }
    }

    // Paint the full view background so blank cells are owned (AGENTS.md
    // render invariant: every blank cell must carry theme.bg).
    let bg = Block::default().style(Style::default().bg(app.theme.bg));
    frame.render_widget(bg, area);

    // Find step number and title for the breadcrumb.
    let (step_no, title) = app
        .active_subagent_popup
        .as_ref()
        .and_then(|id| {
            app.tools()
                .active
                .iter()
                .find(|a| a.tool_id == *id)
                .map(|a| (a.phys_idx + 1, a.output.title_raw.clone()))
                .or_else(|| {
                    app.tools()
                        .blocks
                        .iter()
                        .find(|b| b.tool_id == *id)
                        .map(|b| (b.phys_idx + 1, b.output.title_raw.clone()))
                })
        })
        .unwrap_or_else(|| (0, "Subagent".to_string()));

    // Breadcrumb: `‹ 4. Subagent · code review`, with the view's one real key
    // hint at the far right. The view draws no footer (it is not a popup), so
    // this row is the only place `Enter` can be advertised.
    let breadcrumb_text = if step_no > 0 {
        format!("‹ {step_no}. Subagent · {title}")
    } else {
        format!("‹ Subagent · {title}")
    };
    let hint = if app
        .subagent_popup()
        .is_some_and(|popup| !popup.expanded_steps.is_empty())
    {
        app.msgs().subagent_view_collapse_hint
    } else {
        app.msgs().subagent_view_expand_hint
    };
    let title_cols = UnicodeWidthStr::width(breadcrumb_text.as_str());
    let hint_cols = UnicodeWidthStr::width(hint);
    // Drop the hint rather than let it eat the title: the title is what says
    // *which* subagent this is, and a clipped `Enter exp` helps nobody.
    let gap = chunks[0]
        .width
        .saturating_sub(title_cols as u16 + hint_cols as u16);
    let mut spans = vec![Span::styled(
        breadcrumb_text,
        Style::default()
            .fg(app.theme.accent)
            .add_modifier(Modifier::BOLD),
    )];
    if hint_cols > 0 && gap >= 1 {
        spans.push(Span::raw(" ".repeat(gap as usize)));
        spans.push(Span::styled(hint, Style::default().fg(app.theme.muted)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), chunks[0]);

    // Body: reuse the kit's body renderer (no chrome).
    let surface = super::render_with_ctx(app, frame, body_area, |frame, area, ctx| {
        agent_tui_kit::render::popups::subagent_popup::render_subagent_body(frame, area, ctx)
    });

    // Scrollbar on the right edge of the body.
    if scrollbar_w > 0
        && !surface.body_area.is_empty()
        && let Some(popup) = app.subagent_popup()
        && let Some(layout) = popup.layout.as_ref()
    {
        let total = layout.total_rows;
        let content_height = body_area.height as usize;
        let scroll = popup.scroll as usize;
        let scrollbar_area = Rect::new(
            body_area.x + body_area.width,
            body_area.y,
            scrollbar_w,
            body_area.height,
        );
        agent_tui_kit::render::popups::render_popup_scrollbar(
            frame,
            scrollbar_area,
            total,
            content_height,
            scroll,
        );
    }

    // Record mouse hit areas: the whole main area is the subagent surface,
    // but text selection is limited to the body.
    app.mouse.set_area(SurfaceId::SubagentPopup, area);
    app.mouse.popup_text_body_area = surface.body_area;
    app.mouse.popup_text_hit_rows = surface.hit_rows;
    // The breadcrumb row is the view's back affordance: the `‹` is the promise
    // and the whole row is the target (a two-cell chevron is not a hit target).
    app.mouse.subagent_back_area = chunks[0];
    // Each capped block's overflow row is that block's own toggle.
    app.mouse.subagent_block_toggles = surface.block_toggles;
    // The pill's hit rect (empty while the view follows its tail).
    app.mouse.subagent_scroll_back_area = surface.scroll_back_area;
}
