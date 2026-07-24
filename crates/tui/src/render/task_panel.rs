//! Sticky persistent-task progress strip (under Log, outer layout split).

use ratatui::{
    Frame,
    layout::Rect,
    style::Style,
    text::{Line, Span},
    widgets::Paragraph,
};

use crate::widgets::state::{
    App,
    task_panel::{STICKY_BODY_CAP, format_checklist_lines, format_sticky_title_line},
};

pub(crate) fn render_task_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.task_panel_area = area;
    if area.height == 0 || area.width == 0 || !app.task_panel.visible {
        return;
    }

    let msgs = app.msgs();
    let title = format_sticky_title_line(&msgs, &app.task_panel.snapshot);
    let style = Style::default().fg(app.theme.accent);

    if !app.task_panel.expanded || area.height == 1 {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(title, style))),
            area,
        );
        return;
    }

    let mut lines = vec![Line::from(Span::styled(
        title.replace('▼', "▲").replace('▸', "▾"),
        style,
    ))];
    for row in format_checklist_lines(&app.task_panel.snapshot, STICKY_BODY_CAP) {
        lines.push(Line::from(Span::styled(
            row,
            Style::default().fg(app.theme.fg),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
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
}
