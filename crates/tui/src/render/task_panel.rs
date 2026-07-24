//! Sticky persistent-task progress strip (under Log, outer layout split).

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
};

use crate::widgets::state::{
    App,
    task_panel::{STICKY_BODY_CAP, format_checklist_lines, format_sticky_title_line},
};

/// Extra rows for sticky chrome: bottom border joins the Log box (sides continue).
pub(crate) const STICKY_BORDER_ROWS: u16 = 1;

pub(crate) fn render_task_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.task_panel_area = area;
    if area.height == 0 || area.width == 0 || !app.task_panel.visible {
        return;
    }

    // Clear first so Input/placeholder cells do not show through as a gray band.
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

    let msgs = app.msgs();
    let title = format_sticky_title_line(&msgs, &app.task_panel.snapshot);
    let bg = app.theme.bg;
    let title_style = Style::default().fg(app.theme.accent).bg(bg);
    let row_style = Style::default().fg(app.theme.fg).bg(bg);

    if !app.task_panel.expanded || inner.height == 1 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(title, title_style))),
            inner,
        );
        return;
    }

    let mut lines = vec![Line::from(Span::styled(
        title.replace('▼', "▲").replace('▸', "▾"),
        title_style,
    ))];
    for row in format_checklist_lines(&app.task_panel.snapshot, STICKY_BODY_CAP) {
        lines.push(Line::from(Span::styled(row, row_style)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use tact_protocol::{AgentUpdate, TaskSnapshot, TaskStatusSnapshot, TasksChangeReason};

    use crate::render::test_harness::{make_app, render_app_text};

    #[test]
    fn main_area_shows_sticky_when_tasks_visible() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::TasksChanged {
            tasks: vec![TaskSnapshot {
                id: 1,
                subject: "Fix auth".into(),
                status: TaskStatusSnapshot::InProgress,
                owner: String::new(),
            }],
            reason: TasksChangeReason::Created,
        });
        let text = render_app_text(&mut app, 100, 30);
        assert!(
            text.contains('▸') || text.contains('▼'),
            "sticky collapsed marker should render, got:\n{text}"
        );
        assert!(
            text.contains("Fix auth"),
            "sticky/log should show task subject, got:\n{text}"
        );
    }

    #[test]
    fn sticky_expanded_joins_log_without_orphan_band() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::TasksChanged {
            tasks: vec![
                TaskSnapshot {
                    id: 1,
                    subject: "design schema".into(),
                    status: TaskStatusSnapshot::InProgress,
                    owner: "alice".into(),
                },
                TaskSnapshot {
                    id: 2,
                    subject: "build api".into(),
                    status: TaskStatusSnapshot::Pending,
                    owner: String::new(),
                },
            ],
            reason: TasksChangeReason::Created,
        });
        app.task_panel.expanded = true;
        let text = render_app_text(&mut app, 100, 30);
        assert!(
            text.contains("design schema") && text.contains("build api"),
            "expanded sticky checklist should render inside main chrome, got:\n{text}"
        );
        assert!(
            text.contains('▾') || text.contains('▲'),
            "expanded sticky title marker should render, got:\n{text}"
        );
        // Input placeholder should still appear below the joined panel.
        assert!(
            text.contains("Type a task") || text.contains("输入"),
            "input should remain below sticky, got:\n{text}"
        );
        // Sticky bottom border should sit above the Input box (joined Log chrome).
        let sticky_line = text
            .lines()
            .position(|l| l.contains("build api"))
            .expect("sticky row");
        let input_line = text
            .lines()
            .position(|l| l.contains("Type a task") || l.contains("输入"))
            .expect("input");
        assert!(
            sticky_line < input_line,
            "sticky must render above input (no overlap), sticky={sticky_line} input={input_line}"
        );
    }
}
