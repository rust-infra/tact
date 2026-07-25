//! Unified sticky host under Log: tabs Tasks | Subagent.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::widgets::state::{
    App, StickyTab,
    task_panel::{format_grouped_lines, format_sticky_title_line},
};

/// Extra rows for sticky chrome: bottom border joins the Log box (sides continue).
pub(crate) const STICKY_BORDER_ROWS: u16 = 1;

pub(crate) fn sticky_host_visible(app: &App) -> bool {
    app.task_panel.visible || app.subagent_pane.has_content
}

/// Content rows inside the sticky (excluding border).
pub(crate) fn sticky_host_content_height(app: &App) -> usize {
    if !sticky_host_visible(app) {
        return 0;
    }
    if !app.sticky_expanded {
        return 1;
    }
    match effective_tab(app) {
        StickyTab::Tasks => {
            // title row + body (reuse prior expanded formula without double-counting)
            let body = format_grouped_lines(&app.task_panel.snapshot, app.task_panel.scroll, 10)
                .len()
                .max(1);
            1 + body
        }
        StickyTab::Subagent => {
            // title row + in-body header (`sub … · sess …`) + capped log lines
            2 + app.subagent_pane.visible_lines(10).len()
        }
    }
}

fn effective_tab(app: &App) -> StickyTab {
    match app.sticky_tab {
        StickyTab::Tasks if app.task_panel.visible => StickyTab::Tasks,
        StickyTab::Subagent if app.subagent_pane.has_content => StickyTab::Subagent,
        _ if app.subagent_pane.has_content => StickyTab::Subagent,
        _ => StickyTab::Tasks,
    }
}

pub(crate) fn render_task_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.task_panel_area = area;
    if area.height == 0 || area.width == 0 || !sticky_host_visible(app) {
        return;
    }

    frame.render_widget(Clear, area);

    let block = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT | Borders::BOTTOM)
        .border_type(app.theme.block_border_type())
        .border_style(Style::default().fg(app.theme.border))
        .style(Style::default().bg(app.theme.bg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new("").style(Style::default().bg(app.theme.bg)),
        inner,
    );

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let bg = app.theme.bg;
    let accent = Style::default()
        .fg(app.theme.accent)
        .bg(bg)
        .add_modifier(Modifier::BOLD);
    let muted = Style::default().fg(app.theme.muted_fg()).bg(bg);
    let row_style = Style::default().fg(app.theme.fg).bg(bg);
    let tab = effective_tab(app);

    let title_line = host_title_line(app, tab, accent, muted, row_style);

    if !app.sticky_expanded || inner.height == 1 {
        frame.render_widget(Paragraph::new(title_line), inner);
        return;
    }

    // Expanded: title + body
    let title_area = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width,
        height: 1,
    };
    frame.render_widget(Paragraph::new(title_line), title_area);

    let body_y = inner.y.saturating_add(1);
    let body_h = inner.height.saturating_sub(1);
    if body_h == 0 {
        return;
    }
    let body_area = Rect {
        x: inner.x,
        y: body_y,
        width: inner.width,
        height: body_h,
    };

    match tab {
        StickyTab::Tasks => {
            let msgs = app.msgs();
            let _ = msgs; // title already drawn
            let lines = format_grouped_lines(
                &app.task_panel.snapshot,
                app.task_panel.scroll,
                body_h as usize,
            );
            let styled: Vec<Line> = lines
                .into_iter()
                .map(|l| Line::from(Span::styled(l, row_style)))
                .collect();
            frame.render_widget(Paragraph::new(styled), body_area);
        }
        StickyTab::Subagent => {
            let mut body_lines: Vec<Line> = Vec::new();
            let header = format!(
                "sub {} · sess {}{}",
                short_id(&app.subagent_pane.parent_tool_id),
                short_id(&app.subagent_pane.session_id),
                if app.subagent_pane.running {
                    " · running"
                } else {
                    ""
                }
            );
            body_lines.push(Line::from(Span::styled(header, muted)));
            for line in app
                .subagent_pane
                .visible_lines(body_h.saturating_sub(1) as usize)
            {
                body_lines.push(Line::from(Span::styled(line.to_string(), row_style)));
            }
            frame.render_widget(Paragraph::new(body_lines), body_area);
        }
    }
}

fn host_title_line(
    app: &App,
    tab: StickyTab,
    accent: Style,
    muted: Style,
    row: Style,
) -> Line<'static> {
    let mut spans = Vec::new();
    if app.task_panel.visible {
        let style = if tab == StickyTab::Tasks { accent } else { muted };
        spans.push(Span::styled("[Tasks]", style));
        spans.push(Span::styled(" ", row));
    }
    if app.subagent_pane.has_content {
        let style = if tab == StickyTab::Subagent {
            accent
        } else {
            muted
        };
        let label = if app.subagent_badge > 0 && tab != StickyTab::Subagent {
            format!("[Subagent·{}]", app.subagent_badge.min(99))
        } else {
            "[Subagent]".into()
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::styled(" ", row));
    }

    let rest = match tab {
        StickyTab::Tasks => {
            let msgs = app.msgs();
            let t = format_sticky_title_line(&msgs, &app.task_panel.snapshot);
            // Drop leading ▸ — tabs already provide chrome.
            t.trim_start_matches('▸').trim_start().to_string()
        }
        StickyTab::Subagent => {
            let status = if app.subagent_pane.running {
                "running"
            } else {
                "idle"
            };
            let chevron = if app.sticky_expanded { "▼" } else { "▶" };
            format!(
                "sub {} · {status}  {chevron}",
                short_id(&app.subagent_pane.parent_tool_id)
            )
        }
    };
    spans.push(Span::styled(rest, row));
    Line::from(spans)
}

fn short_id(id: &str) -> String {
    let take = id.chars().take(8).collect::<String>();
    if id.chars().count() > 8 {
        format!("{take}…")
    } else {
        take
    }
}
