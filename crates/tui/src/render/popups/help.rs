use ratatui::{Frame, layout::Rect};

use crate::widgets::{help_widget::HelpWidget, state::App};

pub(crate) fn render_help_panel(frame: &mut Frame, area: Rect, app: &mut App) {
    let msgs = app.msgs();
    let theme = app.theme;
    let voice_label: Option<&str> = app.voice_parsed_keybind.as_ref().map(|(m, k)| {
        let _ = m;
        let key_str = match k {
            crossterm::event::KeyCode::Char(c) => {
                let upper = c.to_uppercase().to_string();
                format!("Ctrl+{upper}")
            }
            _ => format!("{:?}", k),
        };
        // Leak the string for the widget's static lifetime requirement
        // (this is fine: the help panel is short-lived and rendered once per frame)
        let leaked: &'static mut str = Box::leak(key_str.into_boxed_str());
        leaked as &str
    });
    let widget = HelpWidget::new(&msgs, &theme, voice_label);
    frame.render_widget(widget, area);
}

// ── Overlay popups ──
