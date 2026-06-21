use crate::widgets::state::{App, InputMode};
use crossterm::event::{KeyCode, KeyEvent};

/// File picker key handling: Up/Down or j/k to navigate, Enter to insert the
/// selected path prefixed with `@`, Esc to cancel and leave a literal `@`.
pub(crate) fn handle_file_picker_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            if let Some(path) = app.file_picker.selected_path() {
                let insert = if path.chars().any(|c| c.is_whitespace()) {
                    format!("@\"{}\"", path)
                } else {
                    format!("@{}", path)
                };
                app.save_undo();
                let mut insert = insert;
                insert.push(' ');
                app.input.insert_str(app.input_cursor, &insert);
                app.input_cursor += insert.len();
            }
            app.input_mode = InputMode::Insert;
        }
        KeyCode::Char('j') | KeyCode::Down => {
            app.file_picker.move_down();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            app.file_picker.move_up();
        }
        KeyCode::Esc => {
            app.save_undo();
            app.input.insert(app.input_cursor, '@');
            app.input_cursor += '@'.len_utf8();
            app.input_mode = InputMode::Insert;
        }
        _ => {}
    }
}
