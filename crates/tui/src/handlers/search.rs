use super::prev_word_boundary;
use crate::widgets::state::{App, InputMode};
use crossterm::event::{KeyCode, KeyEvent};

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::test_harness::make_app;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    #[test]
    fn enter_sets_search_term_and_returns_to_normal() {
        let mut app = make_app();
        app.input_mode = InputMode::Search;
        app.handle_agent_update(tact_protocol::AgentUpdate::StreamChunk(
            "searchable needle text".into(),
        ));
        app.cmd_line = "needle".into();

        handle_search_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(app.search.term, "needle");
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(
            !app.search.matches.is_empty(),
            "search should find matches in log"
        );
    }

    #[test]
    fn esc_clears_search_input_without_applying() {
        let mut app = make_app();
        app.input_mode = InputMode::Search;
        app.cmd_line = "aborted".into();

        handle_search_mode(&mut app, key(KeyCode::Esc));

        assert!(app.cmd_line.is_empty());
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.search.term.is_empty());
    }

    #[test]
    fn typing_updates_cmd_line() {
        let mut app = make_app();
        app.input_mode = InputMode::Search;

        handle_search_mode(&mut app, key(KeyCode::Char('x')));
        handle_search_mode(&mut app, key(KeyCode::Char('y')));

        assert_eq!(app.cmd_line, "xy");
    }
}
