use crossterm::event::{KeyCode, KeyEvent};

use super::{command_needs_args, execute_palette_command, prev_word_boundary};
use crate::widgets::state::{App, InputMode};

/// Palette mode key handling: filter the command list and navigate with arrow keys; Enter to execute.
pub(crate) fn handle_palette_mode(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let commands = app.palette_commands();
            let filtered = app.palette_filtered();
            if !filtered.is_empty() {
                let idx = app.palette_selected.min(filtered.len() - 1);
                let cmd = commands[filtered[idx]].0.clone();
                app.cmd_line.clear();
                // Arg-taking built-ins prefill Insert so the user can add the
                // subcommand, and keep `undo` pointing at the prior draft.
                // Everything else runs immediately. The palette lists built-ins
                // only — skills are reached through `/skill <name>` — so this
                // is the whole rule.
                if command_needs_args(&cmd) {
                    app.save_undo();
                    app.input = format!("/{cmd} ");
                    app.input_cursor = app.input.len();
                    app.input_mode = InputMode::Insert;
                    // A built-in with subcommands keeps completing where the
                    // palette left off: Enter on `/skill` opens the subcommand
                    // popup, so the next thing typed is already offered.
                    app.slash_command.selected = 0;
                    app.slash_command.start_pos = 0;
                    app.slash_command.active = !app.slash_candidates().is_empty();
                    return;
                }
                app.input_mode = InputMode::Normal;
                let _ = execute_palette_command(app, &cmd);
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
        // Same rule as the insert box: an unbound `Ctrl+<char>` types nothing.
        KeyCode::Char(c)
            if !key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.cmd_line.push(c);
            app.palette_selected = 0;
        }
        KeyCode::Backspace => {
            app.cmd_line.pop();
            app.palette_selected = 0;
        }
        KeyCode::Up => app.step_palette_selection(-1),
        KeyCode::Down => app.step_palette_selection(1),
        KeyCode::Esc => {
            app.cmd_line.clear();
            app.input_mode = InputMode::Normal;
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::*;
    use crate::{render::test_harness::make_app, widgets::state::App};

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
    fn an_unbound_ctrl_key_does_not_type_into_the_palette() {
        // `Ctrl+<char>` is either a global shortcut (consumed before dispatch)
        // or nothing at all — it must never reach the palette's text arm and
        // leave a stray letter in the command line.
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.cmd_line = "sk".to_string();

        handle_palette_mode(
            &mut app,
            KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL),
        );

        assert_eq!(app.cmd_line, "sk");
    }

    #[test]
    fn palette_enter_on_a_command_with_subcommands_opens_its_completion() {
        // Picking `/skill` from the Normal-mode palette pre-fills the input;
        // the popup must come up with it, or the subcommands are once again
        // something the user has to remember.
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.palette_selected = app
            .palette_commands()
            .iter()
            .position(|(cmd, _)| cmd == "skill")
            .expect("skill command");

        handle_palette_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(app.input, "/skill ");
        assert!(matches!(app.input_mode, InputMode::Insert));
        assert!(app.slash_command.active, "subcommands should be offered");
        let paths: Vec<String> = app
            .slash_candidates()
            .into_iter()
            .map(|candidate| candidate.path)
            .collect();
        assert_eq!(paths, ["skill list", "skill reload"]);
    }

    #[test]
    fn palette_enter_on_a_command_without_subcommands_stays_quiet() {
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.palette_selected = app
            .palette_commands()
            .iter()
            .position(|(cmd, _)| cmd == "subagent_cancel")
            .expect("subagent_cancel command");

        handle_palette_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(app.input, "/subagent_cancel ");
        assert!(
            !app.slash_command.active,
            "nothing to complete: no popup over an id argument"
        );
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

    #[test]
    fn enter_on_an_arg_taking_command_preserves_prior_input_via_undo() {
        let mut app = make_app();
        app.input = "draft text".into();
        app.input_cursor = app.input.len();
        app.input_mode = InputMode::Palette;
        app.palette_selected = app
            .palette_commands()
            .iter()
            .position(|(c, _)| c == "mcp")
            .expect("mcp command");

        handle_palette_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(app.input, "/mcp ");
        assert!(matches!(app.input_mode, InputMode::Insert));
        assert!(
            !app.undo_stack.is_empty(),
            "overwrite must push undo snapshot"
        );
        // Restore prior draft.
        let (prev, _) = app.undo_stack.last().cloned().expect("undo");
        assert_eq!(prev, "draft text");
    }

    #[test]
    fn enter_on_builtin_wins_over_same_named_skill() {
        use crate::widgets::state::SkillEntry;

        let mut app = make_app();
        app.skills_data = vec![SkillEntry {
            name: "help".into(),
            description: "skill help".into(),
            body: "should not equip".into(),
        }];
        app.input_mode = InputMode::Palette;
        app.cmd_line = "help".into();
        app.palette_selected = 0;

        handle_palette_mode(&mut app, key(KeyCode::Enter));

        assert!(app.show_help, "builtin help should execute");
        assert!(matches!(app.input_mode, InputMode::Normal));
        assert_ne!(app.input, "/help ");
    }

    #[test]
    fn enter_on_plugin_opens_insert_for_subcommand() {
        let mut app = make_app();
        app.input_mode = InputMode::Palette;
        app.palette_selected = app
            .palette_commands()
            .iter()
            .position(|(c, _)| c == "plugin")
            .expect("plugin command in palette");

        handle_palette_mode(&mut app, key(KeyCode::Enter));

        assert_eq!(app.input, "/plugin ");
        assert_eq!(app.input_cursor, "/plugin ".len());
        assert!(matches!(app.input_mode, InputMode::Insert));
        assert!(
            !app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Usage: /plugin") || item.raw.contains("用法")),
            "must not execute bare /plugin from palette: {:?}",
            app.log.items
        );
    }
}
