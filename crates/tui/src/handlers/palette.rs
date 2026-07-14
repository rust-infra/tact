use super::{execute_palette_command, prev_word_boundary};
use crate::widgets::state::{App, InputMode};
use crossterm::event::{KeyCode, KeyEvent};

/// Palette mode key handling: filter the command list and navigate with arrow keys; Enter to execute.
pub(crate) fn handle_palette_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let filter = app.cmd_line.to_lowercase();
            let commands = app.palette_commands();
            let filtered: Vec<usize> = commands
                .iter()
                .enumerate()
                .filter(|(_, (cmd, desc))| {
                    filter.is_empty()
                        || cmd.to_lowercase().contains(&filter)
                        || desc.to_lowercase().contains(&filter)
                })
                .map(|(i, _)| i)
                .collect();
            if !filtered.is_empty() {
                let idx = app.palette_selected.min(filtered.len() - 1);
                let cmd = commands[filtered[idx]].0.as_str();
                app.cmd_line.clear();
                app.input_mode = InputMode::Normal;
                let _ = execute_palette_command(app, cmd);
            }
        }
        // Ctrl+W: delete last word
        KeyCode::Char('w')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            let pos = prev_word_boundary(&app.cmd_line, app.cmd_line.len());
            app.cmd_line.drain(pos..);
            app.palette_selected = 0;
        }
        // Ctrl+U: clear palette input
        KeyCode::Char('u')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.cmd_line.clear();
            app.palette_selected = 0;
        }
        KeyCode::Char(c) => {
            app.cmd_line.push(c);
            app.palette_selected = 0;
        }
        KeyCode::Backspace => {
            app.cmd_line.pop();
            app.palette_selected = 0;
        }
        KeyCode::Up => {
            if app.palette_selected > 0 {
                app.palette_selected -= 1;
            }
        }
        KeyCode::Down => {
            app.palette_selected += 1;
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
    use crate::widgets::state::App;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::empty())
    }

    fn help_index(app: &App) -> usize {
        app.palette_commands()
            .iter()
            .position(|(cmd, _)| *cmd == "help")
            .expect("help command")
    }

    #[test]
    fn up_down_navigates_palette_selection() {
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.palette_selected = 0;

        handle_palette_mode(&mut app, key(KeyCode::Down));
        assert_eq!(app.palette_selected, 1);
        handle_palette_mode(&mut app, key(KeyCode::Up));
        assert_eq!(app.palette_selected, 0);
    }

    #[test]
    fn enter_executes_highlighted_command() {
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.palette_selected = help_index(&app);

        handle_palette_mode(&mut app, key(KeyCode::Enter));

        assert!(app.show_help, "Enter should execute help command");
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.cmd_line.is_empty());
    }

    #[test]
    fn esc_exits_palette_without_executing() {
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.cmd_line = "qui".into();
        app.palette_selected = 3;

        handle_palette_mode(&mut app, key(KeyCode::Esc));

        assert!(matches!(app.input_mode, InputMode::Normal));
        assert!(app.cmd_line.is_empty());
        assert!(!app.show_help);
        assert!(!app.should_quit);
    }
}
