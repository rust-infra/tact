//! Persistent tasks sticky strip under Log.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::widgets::state::{
    App,
    task_panel::{format_grouped_lines, format_sticky_title_line},
};

/// Extra rows for sticky chrome: bottom border joins the Log box (sides continue).
pub(crate) const STICKY_BORDER_ROWS: u16 = 1;

pub(crate) fn sticky_host_visible(app: &App) -> bool {
    app.task_panel.visible
}

/// Content rows inside the sticky (excluding border).
pub(crate) fn sticky_host_content_height(app: &App) -> usize {
    if !sticky_host_visible(app) {
        return 0;
    }
    if !app.task_panel.expanded {
        return 1;
    }
    // title + hairline + body
    let body = format_grouped_lines(
        &app.task_panel.snapshot,
        app.task_panel.scroll,
        app.task_panel.max_visible,
    )
    .len()
    .max(1);
    2 + body
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

    let msgs = app.msgs();
    let rest = format_sticky_title_line(&msgs, &app.task_panel.snapshot)
        .trim_start_matches('▸')
        .trim_start()
        .to_string();
    let title_spans = vec![
        Span::styled("[Tasks]", accent),
        Span::styled(" ", row_style),
        Span::styled(rest, row_style),
    ];
    let title_line = Line::from(title_spans);

    if !app.task_panel.expanded || inner.height == 1 {
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

#[cfg(test)]
mod sticky_tests {
    use tact_protocol::{TaskSnapshot, TaskStatusSnapshot};

    use super::super::test_harness::{make_app, render_main_area_text};

    #[test]
    fn expanded_tasks_sticky_puts_a_rule_between_tabs_and_body() {
        let mut app = make_app();
        app.task_panel.apply_snapshot(vec![TaskSnapshot {
            id: 97,
            subject: "buy bitcoin".into(),
            status: TaskStatusSnapshot::Pending,
            ..Default::default()
        }]);
        app.task_panel.expanded = true;

        let text = render_main_area_text(&mut app, 80, 20);
        let lines: Vec<&str> = text.lines().collect();
        let tabs = lines
            .iter()
            .position(|l| l.contains("[Tasks]"))
            .expect("tab row missing, got:\n{text}");
        let pending = lines
            .iter()
            .position(|l| l.contains("Pending"))
            .expect("pending group header missing");

        assert_eq!(
            pending - tabs,
            2,
            "expected one separator row between tabs and body, got:\n{text}"
        );
        assert!(
            lines[tabs + 1].trim().chars().all(|c| c == '─' || c == '│'),
            "separator row should be a hairline rule, got: {:?}",
            lines[tabs + 1]
        );
    }
}
