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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::{make_app, render_app_text};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    #[test]
    fn empty_picker_shows_no_options() {
        let mut app = make_app();
        app.input_mode = InputMode::FilePicker;

        let text = render_app_text(&mut app, 80, 24);

        assert!(
            text.contains("No options"),
            "empty file picker should show placeholder, got:\n{text}"
        );
    }

    #[test]
    fn j_k_navigates_selection() {
        let mut app = make_app();
        app.input_mode = InputMode::FilePicker;
        app.file_picker
            .set(vec!["a.rs".into(), "b.rs".into(), "c.rs".into()]);

        assert_eq!(app.file_picker.selected, 0);
        handle_file_picker_mode(&mut app, key(KeyCode::Char('j')));
        assert_eq!(app.file_picker.selected, 1);
        handle_file_picker_mode(&mut app, key(KeyCode::Down));
        assert_eq!(app.file_picker.selected, 2);
        handle_file_picker_mode(&mut app, key(KeyCode::Char('k')));
        assert_eq!(app.file_picker.selected, 1);
        handle_file_picker_mode(&mut app, key(KeyCode::Up));
        assert_eq!(app.file_picker.selected, 0);
    }

    #[test]
    fn enter_inserts_selected_path_and_returns_to_insert() {
        let mut app = make_app();
        app.input_mode = InputMode::FilePicker;
        app.file_picker.set(vec!["src/lib.rs".into()]);

        handle_file_picker_mode(&mut app, key(KeyCode::Enter));

        assert!(matches!(app.input_mode, InputMode::Insert));
        assert!(
            app.input.contains("@src/lib.rs"),
            "Enter should insert @path, input={}",
            app.input
        );
    }

    #[test]
    fn esc_inserts_literal_at_sign() {
        let mut app = make_app();
        app.input_mode = InputMode::FilePicker;

        handle_file_picker_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Insert));
        assert_eq!(app.input, "@");
    }
}
