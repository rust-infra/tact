use crate::state::{App, InputMode};
use crossterm::event::{KeyCode, KeyEvent};
use super::prev_word_boundary;

/// Search mode key handling: enter search keywords, Enter to confirm and highlight matches.
pub(crate) fn handle_search_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            app.search.term = app.cmd_line.clone();
            app.update_search_matches();
            app.cmd_line.clear();
            app.input_mode = InputMode::Normal;
        }
        // Ctrl+W: delete last word
        KeyCode::Char('w')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            let pos = prev_word_boundary(&app.cmd_line, app.cmd_line.len());
            app.cmd_line.drain(pos..);
        }
        // Ctrl+U: clear search input
        KeyCode::Char('u')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.cmd_line.clear();
        }
        KeyCode::Char(c) => app.cmd_line.push(c),
        KeyCode::Backspace => {
            app.cmd_line.pop();
        }
        KeyCode::Esc => {
            app.cmd_line.clear();
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}
