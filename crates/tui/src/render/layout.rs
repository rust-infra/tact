use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::Borders,
};

use crate::widgets::state::App;

/// Main content area layout, switching between history, help, or the Log panel
/// based on current display state. The Log panel is always single-column and
/// full-width — there is no side panel or draggable divider.
///
/// When persistent tasks are visible, the main area is split: scrollable Log on
/// top, sticky task strip below (outer split — Log internals unchanged).
pub(crate) fn render_main_area(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.show_history {
        super::popups::history::render_history_panel(frame, area, app);
        return;
    }
    if app.show_help {
        super::popups::help::render_help_panel(frame, area, app);
        return;
    }

    let sticky_h = if super::task_panel::sticky_host_visible(app) {
        let content = super::task_panel::sticky_host_content_height(app) as u16;
        // Content rows + bottom border so sticky continues the Log box.
        content
            .saturating_add(super::task_panel::STICKY_BORDER_ROWS)
            .min(area.height.saturating_sub(2))
    } else {
        0
    };

    if sticky_h == 0 {
        app.mouse.task_panel_area = Rect::default();
        app.mouse.log_area = area;
        app.log_scroll.height = area.height.saturating_sub(2);
        super::log::render_log_panel(frame, area, app);
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(sticky_h)])
            .split(area);
        app.mouse.log_area = chunks[0];
        // Log omits bottom border; sticky draws LEFT|RIGHT|BOTTOM to close the box.
        super::log::render_log_panel_with_borders(
            frame,
            chunks[0],
            app,
            Borders::TOP | Borders::LEFT | Borders::RIGHT,
        );
        super::task_panel::render_task_panel(frame, chunks[1], app);
    }

    if app.thinking.popup.is_some() {
        super::popups::thinking_popup::render_thinking_popup(frame, area, app);
    }
    if app.tools.popup.is_some() {
        super::popups::diff_popup::render_diff_popup(frame, area, app);
    }
    if app.system_prompt_popup.is_some() {
        super::popups::system_prompt_popup::render_system_prompt_popup(frame, area, app);
    }
    if app.code_popup.is_some() {
        super::popups::code_popup::render_code_popup(frame, area, app);
    }
    if app.mermaid_popup.is_some() {
        super::popups::mermaid_popup::render_mermaid_popup(frame, area, app);
    }
    if app.task_dag_popup.is_some() {
        super::popups::task_dag_popup::render_task_dag_popup(frame, area, app);
    }
    if app.subagent_popup.is_some() {
        super::popups::subagent_popup::render_subagent_popup(frame, area, app);
    }
}

#[cfg(test)]
mod render_tests {
    use std::collections::HashMap;

    use tact_protocol::{
        AgentErrorKind, AgentUpdate, PlanStep, StepResult, StepStatus, ToolPresentationInfo,
    };

    use super::super::test_harness::{buffer_contains, make_app, render_app_text};
    use crate::widgets::state::Status;

    #[test]
    fn main_area_renders_tool_and_stream_content() {
        let mut app = make_app();

        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "main.rs".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted {
            idx: 0,
            tool_id: "tool_read_1".into(),
            tool_name: "read_file".into(),
            arg_summary: "main.rs".into(),
            arg_full: "main.rs".into(),
            presentation: ToolPresentationInfo::generic("read_file"),
        });
        app.handle_agent_update(AgentUpdate::StepFinished {
            idx: 0,
            tool_id: "tool_read_1".into(),
            result: StepResult {
                tool: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("fn main() {}".into()),
                duration_us: Some(1000),
                permission_label: None,
                presentation: ToolPresentationInfo::generic("read_file"),
            },
        });
        app.handle_agent_update(AgentUpdate::StreamChunk("Hello from mock.".into()));
        app.handle_agent_update(AgentUpdate::TaskComplete("Hello from mock.".into()));

        assert!(matches!(app.status, Status::Done));

        let text = render_app_text(&mut app, 100, 30);
        assert!(
            text.contains("read_file") || text.contains("main.rs"),
            "log should show tool activity, buffer:\n{text}"
        );
        assert!(
            text.contains("Hello from mock"),
            "stream chunk should be visible, buffer:\n{text}"
        );
    }

    #[test]
    fn main_area_renders_after_fatal_error() {
        let mut app = make_app();
        app.handle_agent_update(AgentUpdate::Error(AgentErrorKind::Other(
            "provider timeout".into(),
        )));

        assert!(matches!(app.status, Status::Idle));

        let backend = ratatui::backend::TestBackend::new(100, 24);
        let mut terminal = ratatui::Terminal::new(backend).expect("terminal");
        terminal
            .draw(|frame| super::render_main_area(frame, frame.area(), &mut app))
            .expect("draw");

        assert!(
            buffer_contains(terminal.backend().buffer(), "provider timeout")
                || app
                    .log
                    .items
                    .iter()
                    .any(|item| item.raw.contains("provider timeout")),
            "error should be visible in log or buffer"
        );
    }
}
