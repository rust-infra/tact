use ratatui::{Frame, layout::Rect};

use crate::widgets::{help_widget::HelpWidget, state::App};

pub(crate) fn render_help_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let msgs = app.msgs();
    let theme = app.theme;
    let widget = HelpWidget::new(&msgs, &theme);
    frame.render_widget(widget, area);
}

// ── Overlay popups ──
