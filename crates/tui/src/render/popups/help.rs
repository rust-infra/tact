//! Help panel — app-layer wrapper (voice keybind label injection).

use ratatui::{Frame, layout::Rect};

use crate::widgets::state::App;

pub(crate) fn render_help_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let msgs = app.msgs();
    let theme = app.theme;
    // Built fresh each frame, so it is an ordinary local `String`: the widget
    // takes the label on its own lifetime (it used to require `'static`, which
    // forced a `Box::leak` per frame).
    let voice_label: Option<String> = app.voice_parsed_keybind.as_ref().map(|(m, k)| {
        let _ = m;
        match k {
            crossterm::event::KeyCode::Char(c) => {
                let upper = c.to_uppercase().to_string();
                format!("Ctrl+{upper}")
            }
            _ => format!("{:?}", k),
        }
    });
    let widget =
        agent_tui_kit::widgets::help_widget::HelpWidget::new(&msgs, &theme, voice_label.as_deref());
    frame.render_widget(widget, area);
}
