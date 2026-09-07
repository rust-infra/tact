//! Two-domain sticky host under the Log (pure render): Tasks | Subagent.
//!
//! Each domain (`TaskPanelState` / `SubagentPanelState`) independently decides
//! visibility and expand state. This host draws one shared strip:
//!
//! - visible domains appear as `[Tasks] …` / `[Subagent] …` tab segments on
//!   the title row (accent = active, muted = inactive);
//! - when the active visible domain is expanded, a hairline row and that
//!   domain's body follow;
//! - hit rectangles for each tab segment are returned so the host app can
//!   route clicks (mirrors `render_log_panel_pure` returning cancel-button
//!   areas).
//!
//! Rendering invariants: the full `area` gets the base background, the strip
//! continues the Log box (LEFT|RIGHT|BOTTOM borders), and every cell outside
//! glyphs carries `theme.bg` (AGENTS.md render invariants).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};
use unicode_width::UnicodeWidthStr;

use crate::{
    render::ctx::RenderCtx,
    state::{
        StickyTab,
        subagent_panel::{format_sticky_title_line as format_subagent_title, format_subagent_lines},
        task_panel::{
            format_grouped_lines, format_sticky_title_line as format_tasks_title,
        },
    },
};

/// Extra rows for sticky chrome: bottom border joins the Log box (sides continue).
pub const STICKY_BORDER_ROWS: u16 = 1;

/// Hit areas the host renders this frame (tab label rects).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct StickyHostHitAreas {
    pub tab_areas: Vec<(StickyTab, Rect)>,
}

pub fn sticky_host_visible(ctx: &RenderCtx) -> bool {
    ctx.task_panel.visible || ctx.subagent_panel.visible
}

pub fn domain_visible(ctx: &RenderCtx, tab: StickyTab) -> bool {
    match tab {
        StickyTab::Tasks => ctx.task_panel.visible,
        StickyTab::Subagent => ctx.subagent_panel.visible,
    }
}

fn domain_expanded(ctx: &RenderCtx, tab: StickyTab) -> bool {
    match tab {
        StickyTab::Tasks => ctx.task_panel.expanded,
        StickyTab::Subagent => ctx.subagent_panel.expanded,
    }
}

/// The tab whose body the host should show: the mouse-active tab when visible,
/// otherwise the first visible domain (Tasks preferred, then Subagent).
pub fn active_visible_tab(ctx: &RenderCtx) -> StickyTab {
    let active = ctx.mouse.active_sticky_tab;
    if domain_visible(ctx, active) {
        active
    } else if ctx.task_panel.visible {
        StickyTab::Tasks
    } else {
        StickyTab::Subagent
    }
}

/// Content rows inside the sticky (excluding the border). Collapsed = 1
/// (title row). Expanded = 2 + the active domain's visible body rows.
pub fn sticky_host_content_height(ctx: &RenderCtx) -> usize {
    if !sticky_host_visible(ctx) {
        return 0;
    }
    let tab = active_visible_tab(ctx);
    if !domain_expanded(ctx, tab) {
        return 1;
    }
    let body = body_lines(ctx, tab).len().max(1);
    2 + body
}

fn body_lines(ctx: &RenderCtx, tab: StickyTab) -> Vec<String> {
    match tab {
        StickyTab::Tasks => format_grouped_lines(
            &ctx.task_panel.snapshot,
            ctx.task_panel.scroll,
            ctx.task_panel.max_visible,
        ),
        StickyTab::Subagent => format_subagent_lines(
            &ctx.subagent_panel.snapshot,
            ctx.subagent_panel.scroll,
            ctx.subagent_panel.max_visible,
        ),
    }
}

/// Domain summary text for the title row (the "rest" after the `[Label]` tab
/// segment). The per-domain title helpers render `▸ <label> <counts> …`; strip
/// the leading `▸ ` and the repeated label word so a row reads
/// `[Tasks] 1/3 · focus`, not `[Tasks] Tasks 1/3 · focus`.
fn title_rest(ctx: &RenderCtx, tab: StickyTab) -> String {
    let (full, label): (String, &str) = match tab {
        StickyTab::Tasks => (
            format_tasks_title(&ctx.messages, &ctx.task_panel.snapshot),
            ctx.messages.tasks_sticky_title,
        ),
        StickyTab::Subagent => (
            format_subagent_title(&ctx.messages, &ctx.subagent_panel.snapshot),
            ctx.messages.subagents_sticky_title,
        ),
    };
    let trimmed = full.trim_start_matches('▸').trim_start();
    if let Some(rest) = trimmed.strip_prefix(label) {
        rest.trim_start().to_string()
    } else {
        trimmed.to_string()
    }
}

/// Renders the sticky host strip into `area` (which includes the bottom border
/// row). Returns per-frame hit areas for the tab labels.
pub fn render_sticky_host(frame: &mut Frame, area: Rect, ctx: &RenderCtx) -> StickyHostHitAreas {
    if area.height == 0 || area.width == 0 || !sticky_host_visible(ctx) {
        return StickyHostHitAreas::default();
    }

    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_type(ctx.theme.block_border_type())
        .border_style(Style::default().fg(ctx.theme.border))
        .style(Style::default().bg(ctx.theme.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(ctx.theme.bg)),
        inner,
    );

    if inner.height == 0 || inner.width == 0 {
        return StickyHostHitAreas::default();
    }

    let bg = ctx.theme.bg;
    let accent = Style::default()
        .fg(ctx.theme.accent)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let accent_plain = Style::default().fg(ctx.theme.accent).bg(bg);
    let muted = Style::default().fg(ctx.theme.muted_fg()).bg(bg);
    let row_style = Style::default().fg(ctx.theme.fg).bg(bg);

    let active = active_visible_tab(ctx);

    // Build the title row segment by segment so tab hit rects stay exact.
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut tab_areas: Vec<(StickyTab, Rect)> = Vec::new();
    let mut x_cursor = inner.x;
    let title_y = inner.y;

    let mut first = true;
    for tab in [StickyTab::Tasks, StickyTab::Subagent] {
        if !domain_visible(ctx, tab) {
            continue;
        }
        if !first {
            spans.push(Span::styled("   ", row_style));
            x_cursor = x_cursor.saturating_add(3);
        }
        first = false;

        let label_text = match tab {
            StickyTab::Tasks => "[Tasks]",
            StickyTab::Subagent => "[Subagent]",
        };
        let label_style = if tab == active {
            if domain_expanded(ctx, tab) {
                accent
            } else {
                accent_plain
            }
        } else {
            muted
        };
        let rest_style = if tab == active { row_style } else { muted };

        let label_width = UnicodeWidthStr::width(label_text);
        spans.push(Span::styled(label_text.to_string(), label_style));
        tab_areas.push((
            tab,
            Rect::new(x_cursor, title_y, label_width as u16, 1),
        ));
        x_cursor = x_cursor.saturating_add(label_width as u16);

        spans.push(Span::styled(" ", row_style));
        x_cursor = x_cursor.saturating_add(1);

        let rest = title_rest(ctx, tab);
        let rest_width = UnicodeWidthStr::width(rest.as_str());
        spans.push(Span::styled(rest, rest_style));
        x_cursor = x_cursor.saturating_add(rest_width as u16);
    }

    let title_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(Line::from(spans)), title_area);

    let expanded_tab = active_visible_tab(ctx);
    if !domain_expanded(ctx, expanded_tab) || inner.height == 1 {
        return StickyHostHitAreas {
            tab_areas,
        };
    }

    // Expanded: hairline + active-domain body.
    if inner.height < 2 {
        return StickyHostHitAreas { tab_areas };
    }
    let gap_area = Rect {
        x: inner.x,
        y: inner.y.saturating_add(1),
        width: inner.width,
        height: 1,
    };
    let rule = "─".repeat(inner.width as usize);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(rule, muted))),
        gap_area,
    );

    let body_y = inner.y.saturating_add(2);
    let body_h = inner.height.saturating_sub(2);
    if body_h == 0 {
        return StickyHostHitAreas { tab_areas };
    }
    let body_area = Rect {
        x: inner.x,
        y: body_y,
        width: inner.width,
        height: body_h,
    };

    let lines = body_lines(ctx, expanded_tab);
    let styled: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, row_style)))
        .collect();
    frame.render_widget(Paragraph::new(styled), body_area);

    StickyHostHitAreas { tab_areas }
}
