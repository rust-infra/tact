use crate::state::{App, InputMode};
use crossterm::event::{KeyCode, KeyEvent};

/// Select popup mode key handling: up/down to navigate, Enter to confirm, Esc to cancel.
pub(crate) fn handle_select_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            if app.select.options.is_empty() {
                let msgs = app.msgs();
                app.add_system_message(msgs.no_options.to_string());
            } else {
                let idx = app.select.confirm().unwrap_or(0);
                let msgs = app.msgs();
                app.add_system_message(
                    msgs.selected_tmpl
                        .replace("{}", &app.select
                            .options
                            .get(idx)
                            .cloned()
                            .unwrap_or_else(|| "?".to_string()))
                );
            }
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.select.move_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.select.move_up();
        }
        KeyCode::Esc => {
            app.select.cancel();
            let msgs = app.msgs();
            app.add_system_message(msgs.selection_cancelled.to_string());
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}
