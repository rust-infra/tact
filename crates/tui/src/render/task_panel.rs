//! Persistent tasks sticky strip — app-layer wrapper (mouse area + kit render).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) use agent_tui_kit::render::task_panel::STICKY_BORDER_ROWS;

pub(crate) fn sticky_host_visible(app: &App) -> bool {
    app.task_panel().visible
}

/// Content rows inside the sticky (excluding border).
pub(crate) fn sticky_host_content_height(app: &App) -> usize {
    if !sticky_host_visible(app) {
        return 0;
    }
    if !app.task_panel().expanded {
        return 1;
    }
    // title + hairline + body
    let body = crate::widgets::state::task_panel::format_grouped_lines(
        &app.task_panel().snapshot,
        app.task_panel().scroll,
        app.task_panel().max_visible,
    )
    .len()
    .max(1);
    2 + body
}

pub(crate) fn render_task_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    app.mouse.task_panel_area = area;
    let ctx = app.render_ctx();
    agent_tui_kit::render::task_panel::render_task_panel(frame, area, &ctx);
}

#[cfg(test)]
mod sticky_tests {
    use tact_protocol::{TaskSnapshot, TaskStatusSnapshot};

    use super::super::test_harness::{make_app, render_main_area_text};

    #[test]
    fn expanded_tasks_sticky_puts_a_rule_between_tabs_and_body() {
        let mut app = make_app();
        app.task_panel_mut().apply_snapshot(vec![TaskSnapshot {
            id: 97,
            subject: "buy bitcoin".into(),
            status: TaskStatusSnapshot::Pending,
            ..Default::default()
        }]);
        app.task_panel_mut().expanded = true;

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
