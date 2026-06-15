use ratatui::{Frame, layout::Rect};
use crate::state::App;
use crate::widgets::select_popup_widget::SelectPopupWidget;

pub(crate) fn render_select_popup(frame: &mut Frame, area: Rect, app: &App) {
    use crate::widgets::select_popup_widget::SelectPopupWidget;
    let widget = SelectPopupWidget::new(
        &app.select,
        app.theme.highlight,
        app.theme.fg,
        app.theme.bottom_bar_bg,
        app.msgs().select_empty,
        app.msgs().select_arrow,
    );
    frame.render_widget(widget, area);
}
