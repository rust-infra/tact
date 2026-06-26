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
    if app.diff_popup.is_some() {
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
