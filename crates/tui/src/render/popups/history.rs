use ratatui::{Frame, layout::Rect};
use crate::state::App;
use crate::widgets::history_panel_widget::HistoryPopupWidget;

pub(crate) fn render_history_panel(frame: &mut Frame, area: Rect, app: &App) {
    use crate::widgets::history_panel_widget::HistoryPopupWidget;
    let widget = HistoryPopupWidget::new(
        &app.task_history,
        app.theme.accent,
        app.theme.border,
        app.msgs().history_title,
    );
    frame.render_widget(widget, area);
}

