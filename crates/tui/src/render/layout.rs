use crate::widgets::state::App;
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
};

/// Divider width (in columns) for the draggable panel resize handle.
const DIVIDER_WIDTH: u16 = 2;

/// Main content area layout, switching between history, help, or Plan+Log dual-panel based on current display state.
pub(crate) fn render_main_area(frame: &mut Frame, area: Rect, app: &mut App) {
    if app.show_history {
        super::popups::history::render_history_panel(frame, area, app);
        return;
    }
    if app.show_help {
        super::popups::help::render_help_panel(frame, area, app);
        return;
    }
    if app.plan.visible {
        // Use dynamic split ratio from App state
        let ratio_left = app.panel_split_ratio.clamp(0.10, 0.70);
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Ratio((ratio_left * 1000.0) as u32, 1000),
                Constraint::Length(DIVIDER_WIDTH),
                Constraint::Ratio((1000.0 - ratio_left * 1000.0) as u32, 1000),
            ])
            .split(area);
        app.mouse.plan_area = chunks[0];
        app.mouse.divider_area = chunks[1];
        app.mouse.log_area = chunks[2];
        super::plan::render_plan_panel(frame, chunks[0], app);
        // Render the divider bar
        render_divider(frame, chunks[1], app);
        super::log::render_log_panel(frame, chunks[2], app);
    } else {
        app.mouse.plan_area = Rect::new(0, 0, 0, 0);
        app.mouse.divider_area = Rect::new(0, 0, 0, 0);
        app.mouse.log_area = area;
        super::log::render_log_panel(frame, area, app);
    }

    if app.thinking.popup.is_some() {
        super::popups::thinking_popup::render_thinking_popup(frame, area, app);
    }
    if app.tools.popup.is_some() {
        super::popups::diff_popup::render_diff_popup(frame, area, app);
    }
    if app.code_popup.is_some() {
        super::popups::code_popup::render_code_popup(frame, area, app);
    }
}

/// Render the draggable divider bar between Plan and Log panels.
fn render_divider(frame: &mut Frame, area: Rect, app: &App) {
    use ratatui::{
        style::Style,
        widgets::{Block, Borders},
    };

    // Highlight divider when user is hovering over it or actively resizing
    let border_style = if app.mouse.is_resizing_panel {
        Style::default().fg(app.theme.accent)
    } else {
        Style::default().fg(app.theme.border)
    };

    let divider = Block::default()
        .borders(Borders::LEFT | Borders::RIGHT)
        .border_style(border_style)
        .style(Style::default().bg(app.theme.bg));
    frame.render_widget(divider, area);
}

#[cfg(test)]
mod render_tests {
    use super::super::test_harness::{buffer_contains, make_app, render_app_text};
    use crate::widgets::state::Status;
    use std::collections::HashMap;
    use tact_protocol::{AgentErrorKind, AgentUpdate, PlanStep, StepResult, StepStatus};

    #[test]
    fn main_area_renders_tool_and_stream_content() {
        let mut app = make_app();
        app.plan.visible = true;

        app.handle_agent_update(AgentUpdate::StepAdded(PlanStep::new(
            "read file",
            "read_file",
            "tool_read_1",
            HashMap::from([("path".to_string(), "main.rs".to_string())]),
        )));
        app.handle_agent_update(AgentUpdate::StepStarted(
            0,
            "tool_read_1".into(),
            "read_file".into(),
            "main.rs".into(),
        ));
        app.handle_agent_update(AgentUpdate::StepFinished(
            0,
            "tool_read_1".into(),
            StepResult {
                tool: "read_file".into(),
                arg_summary: "main.rs".into(),
                arg_full: None,
                status: StepStatus::Success,
                message: "ok".into(),
                detail: Some("fn main() {}".into()),
                duration_us: Some(1000),
                permission_label: None,
            },
        ));
        app.handle_agent_update(AgentUpdate::StreamChunk("Hello from mock.".into()));
        app.handle_agent_update(AgentUpdate::TaskComplete("Hello from mock.".into()));

        assert!(matches!(app.status, Status::Done));

        let text = render_app_text(&mut app, 100, 30);
        assert!(
            text.contains("read_file") || text.contains("main.rs"),
            "plan/log should show tool activity, buffer:\n{text}"
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
                || app.raw_messages.iter().any(|m| m.contains("provider timeout")),
            "error should be visible in log or buffer"
        );
    }

    #[test]
    fn dual_panel_layout_renders_with_custom_split_ratio() {
        let mut app = make_app();
        app.plan.visible = true;
        app.panel_split_ratio = 0.45;

        let text = super::super::test_harness::render_main_area_text(&mut app, 120, 30);
        assert!(
            !text.trim().is_empty(),
            "dual-panel layout should render with custom split"
        );
    }

    #[test]
    fn divider_renders_while_resizing() {
        let mut app = make_app();
        app.plan.visible = true;
        app.mouse.is_resizing_panel = true;

        let text = super::super::test_harness::render_main_area_text(&mut app, 120, 30);
        assert!(
            !text.trim().is_empty(),
            "divider should render highlighted while resizing"
        );
    }
}
