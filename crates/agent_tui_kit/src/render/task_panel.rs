//! Persistent tasks sticky strip under Log (pure render).

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::{
    render::ctx::RenderCtx,
    state::task_panel::{format_grouped_lines, format_sticky_title_line},
};

/// Extra rows for sticky chrome: bottom border joins the Log box (sides continue).
pub const STICKY_BORDER_ROWS: u16 = 1;

/// Whether the sticky task strip is visible.
pub fn sticky_host_visible(ctx: &RenderCtx) -> bool {
    ctx.task_panel.visible
}

/// Content rows inside the sticky (excluding border).
pub fn sticky_host_content_height(ctx: &RenderCtx) -> usize {
    if !sticky_host_visible(ctx) {
        return 0;
    }
    if !ctx.task_panel.expanded {
        return 1;
    }
    // title + hairline + body
    let body = format_grouped_lines(
        &ctx.task_panel.snapshot,
        ctx.task_panel.scroll,
        ctx.task_panel.max_visible,
    )
    .len()
    .max(1);
    2 + body
}

pub fn render_task_panel(frame: &mut Frame, area: Rect, ctx: &RenderCtx) {
    if area.height == 0 || area.width == 0 || !sticky_host_visible(ctx) {
        return;
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
        return;
    }

    let bg = ctx.theme.bg;
    let accent = Style::default()
        .fg(ctx.theme.accent)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(ctx.theme.muted_fg()).bg(bg);
    let row_style = Style::default().fg(ctx.theme.fg).bg(bg);

    let rest = format_sticky_title_line(&ctx.messages, &ctx.task_panel.snapshot)
        .trim_start_matches('▸')
        .trim_start()
        .to_string();
    let title_spans = vec![
        Span::styled("[Tasks]", accent),
        Span::styled(" ", row_style),
        Span::styled(rest, row_style),
    ];
    let title_line = Line::from(title_spans);

    if !ctx.task_panel.expanded || inner.height == 1 {
        frame.render_widget(Paragraph::new(title_line), inner);
        return;
    }

    // Expanded: title + hairline + body
    let title_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(title_line), title_area);

    if inner.height < 2 {
        return;
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
        return;
    }
    let body_area = Rect {
        x: inner.x,
        y: body_y,
        width: inner.width,
        height: body_h,
    };

    let lines = format_grouped_lines(
        &ctx.task_panel.snapshot,
        ctx.task_panel.scroll,
        body_h as usize,
    );
    let styled: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(Span::styled(l, row_style)))
        .collect();
    frame.render_widget(Paragraph::new(styled), body_area);
}
