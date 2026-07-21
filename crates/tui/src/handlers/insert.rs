use crossterm::event::{KeyCode, KeyEvent};
use tact_protocol::UserCommand;
use tokio::sync::mpsc::UnboundedSender;

use super::{
    cursor_line_col, end_of_line, execute_palette_command, exit_history, line_col_to_cursor, line_length,
    next_char_boundary, next_word_boundary, prev_char_boundary, prev_word_boundary,
    skills::{is_skill_command, skill_name_set, submit_user_task},
    start_of_line,
};
use crate::widgets::state::{App, InputMode, Status};

fn apply_selected_slash_command(app: &mut App) -> bool {
    let cmds = app.palette_commands();
    let commands: Vec<(&str, &str)> = cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();
    let skill_names = skill_name_set(app);
    let cmds = app.slash_command.matched_commands(&app.input, app.input_cursor, &commands, &skill_names);
    let sel = app.slash_command.selected.min(cmds.len().saturating_sub(1));
    if let Some(&(_idx, (cmd, _desc), _score)) = cmds.get(sel) {
        let start = app.slash_command.start_pos;
        let end = app.input_cursor;
        let replacement = format!("/{cmd} ");
        app.input.replace_range(start..end, &replacement);
        app.input_cursor = start + cmd.len() + 2;
        app.slash_command.active = false;
        return true;
    }
    false
}

/// Enter on an open slash popup:
/// - **skills** → autocomplete to `/name ` only (user may add args, then Enter to run)
/// - **built-in commands** → execute immediately
fn execute_selected_slash_command(app: &mut App) -> bool {
    let cmds = app.palette_commands();
    let commands: Vec<(&str, &str)> = cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();
    let skill_names = skill_name_set(app);
    let matched = app.slash_command.matched_commands(&app.input, app.input_cursor, &commands, &skill_names);
    let sel = app.slash_command.selected.min(matched.len().saturating_sub(1));
    let Some(&(_idx, (cmd, _desc), _score)) = matched.get(sel) else {
        return false;
    };
    // Skills autocomplete; built-ins execute. Built-in names never appear as skills
    // in the palette list, but keep the guard if data drifts. Arg-taking built-ins
    // (e.g. /plugin list) also autocomplete so the user can type the subcommand.
    if (is_skill_command(app, cmd) && !super::is_builtin_palette_command(cmd)) || super::command_needs_args(cmd) {
        return apply_selected_slash_command(app);
    }
    app.slash_command.active = false;
    let outcome = execute_palette_command(app, cmd);
    if outcome.clear_input {
        app.input.clear();
        app.input_cursor = 0;
    }
    true
}

fn handle_enter_submit(app: &mut App, key: &KeyEvent, _user_cmd_tx: &UnboundedSender<UserCommand>) {
    // Deactivate slash command on submit.
    app.slash_command.active = false;

    if key.modifiers.contains(crossterm::event::KeyModifiers::SHIFT)
        || key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
    {
        // insert blank character for writing next line
        app.save_undo();
        app.input.insert(app.input_cursor, '\n');
        app.input_cursor += 1;
    } else if !app.input.is_empty() {
        let trimmed = app.input.trim();
        if let Some(rest) = trimmed.strip_prefix('/') {
            let cmd = rest.split_whitespace().next().unwrap_or_default().to_string();
            let outcome = execute_palette_command(app, &cmd);
            if outcome.handled {
                if outcome.clear_input {
                    app.input.clear();
                    app.input_cursor = 0;
                }
                return;
            }
        }

        if matches!(app.status, Status::Planning | Status::Executing { .. }) {
            app.flash_msg = Some((app.msgs().input_busy_msg.to_string(), std::time::Instant::now()));
            return;
        }

        let display = app.input.clone();
        if tact::consts::exceeds_input_char_limit(display.chars().count()) {
            let msg = app.msgs().input_too_long_tmpl.replace("{}", &tact::consts::MAX_INPUT_CHARS.to_string());
            app.add_system_message(msg);
            return;
        }

        app.input.clear();
        app.input_cursor = 0;
        let _ = submit_user_task(app, display.clone(), display);
    }
}

pub(crate) fn handle_insert_mode(app: &mut App, key: KeyEvent, user_cmd_tx: &UnboundedSender<UserCommand>) {
    match key.code {
        // --- Slash command popup shortcuts (only when active) ---
        KeyCode::Up if app.slash_command.active => {
            let cmds = app.palette_commands();
            let commands: Vec<(&str, &str)> = cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();
            let skill_names = skill_name_set(app);
            let n = app.slash_command.matched_commands(&app.input, app.input_cursor, &commands, &skill_names).len();
            if n > 0 {
                app.slash_command.selected = app.slash_command.selected.saturating_sub(1);
            }
        },
        KeyCode::Down if app.slash_command.active => {
            let cmds = app.palette_commands();
            let commands: Vec<(&str, &str)> = cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();
            let skill_names = skill_name_set(app);
            let n = app.slash_command.matched_commands(&app.input, app.input_cursor, &commands, &skill_names).len();
            if n > 0 {
                let max = n.saturating_sub(1);
                app.slash_command.selected = (app.slash_command.selected + 1).min(max);
            }
        },
        KeyCode::Tab if app.slash_command.active => {
            apply_selected_slash_command(app);
        },
        KeyCode::Enter if app.slash_command.active => {
            // Enter runs the highlighted slash command; Tab only autocompletes.
            if execute_selected_slash_command(app) {
                return;
            }
            // If no command matches, fallback to the normal Enter behavior.
            handle_enter_submit(app, &key, user_cmd_tx);
        },
        KeyCode::Esc if app.slash_command.active => {
            app.slash_command.active = false;
        },
        KeyCode::Char(' ') if app.slash_command.active => {
            app.slash_command.active = false;
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.input.insert(app.input_cursor, ' ');
            app.input_cursor += 1;
        },
        KeyCode::Backspace if app.slash_command.active && key.modifiers.is_empty() => {
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor > 0 {
                let prev = app.input.floor_char_boundary(app.input_cursor - 1);
                app.save_undo();
                app.input.replace_range(prev..app.input_cursor, "");
                app.input_cursor = prev;
                if app.input_cursor <= app.slash_command.start_pos {
                    app.slash_command.active = false;
                }
            }
        },
        // --- End slash command shortcuts ---
        KeyCode::Enter => {
            handle_enter_submit(app, &key, user_cmd_tx);
        },
        // Quick word delete
        KeyCode::Char('w') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor > 0 {
                app.save_undo();
                let pos = prev_word_boundary(&app.input, app.input_cursor);
                app.input.drain(pos..app.input_cursor);
                app.input_cursor = pos;
            }
        },
        KeyCode::Backspace if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor > 0 {
                app.save_undo();
                let pos = prev_word_boundary(&app.input, app.input_cursor);
                app.input.drain(pos..app.input_cursor);
                app.input_cursor = pos;
            }
        },
        // Ctrl+A: jump to input start
        KeyCode::Char('a') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.input_cursor = 0;
        },
        // Ctrl+E: jump to input end
        KeyCode::Char('e') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.input_cursor = app.input.len();
        },
        // Ctrl+K: kill to end of line; if cursor is at end of line, delete newline to merge with next line
        KeyCode::Char('k') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            exit_history(app);
            let end = end_of_line(&app.input, app.input_cursor);
            let delete_end = if end < app.input.len() { end + 1 } else { end };
            if app.input_cursor < delete_end {
                app.save_undo();
            }
            app.input.drain(app.input_cursor..delete_end);
        },
        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor < app.input.len() {
                app.save_undo();
                let pos = next_word_boundary(&app.input, app.input_cursor);
                app.input.drain(app.input_cursor..pos);
            }
        },
        // Ctrl+U: kill to beginning of line
        KeyCode::Char('u') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            exit_history(app);
            let start = start_of_line(&app.input, app.input_cursor);
            if start < app.input_cursor {
                app.save_undo();
            }
            app.input.drain(start..app.input_cursor);
            app.input_cursor = start;
        },
        // Ctrl+D: delete character after cursor (only when input is non-empty)
        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            exit_history(app);
            if app.input_cursor < app.input.len() {
                app.save_undo();
                let next = next_char_boundary(&app.input, app.input_cursor);
                app.input.drain(app.input_cursor..next);
            }
        },
        // Ctrl+Home: jump to input start
        KeyCode::Home if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.input_cursor = 0;
        },
        // Ctrl+End: jump to input end
        KeyCode::End if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            app.input_cursor = app.input.len();
        },
        // Ctrl+Backspace: delete previous word (consistent with Ctrl+W / Alt+Backspace)
        KeyCode::Backspace if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            exit_history(app);
            if app.input_cursor > 0 {
                app.save_undo();
                let pos = prev_word_boundary(&app.input, app.input_cursor);
                app.input.drain(pos..app.input_cursor);
                app.input_cursor = pos;
            }
        },
        // Ctrl+P: history back (unrestricted by cursor line position)
        KeyCode::Char('p') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            if !app.input_history.entries.is_empty() {
                if app.input_history.index.is_none() {
                    app.input_history.saved = app.input.clone();
                    app.input_history.index = Some(app.input_history.entries.len() - 1);
                } else if let Some(idx) = app.input_history.index
                    && idx > 0
                {
                    app.input_history.index = Some(idx - 1);
                }
                if let Some(idx) = app.input_history.index {
                    app.input = app.input_history.entries[idx].clone();
                    app.input_cursor = app.input.len();
                }
            }
        },
        // Ctrl+N: history forward (unrestricted by cursor line position)
        KeyCode::Char('n') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            if let Some(idx) = app.input_history.index {
                if idx + 1 < app.input_history.entries.len() {
                    app.input_history.index = Some(idx + 1);
                    app.input = app.input_history.entries[idx + 1].clone();
                    app.input_cursor = app.input.len();
                } else {
                    app.input_history.index = None;
                    app.input = std::mem::take(&mut app.input_history.saved);
                    app.input_cursor = app.input.len();
                }
            }
        },
        // Ctrl+Z: undo
        KeyCode::Char('z') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            if let Some((prev_input, prev_cursor)) = app.undo_stack.pop() {
                app.redo_stack.push((app.input.clone(), app.input_cursor));
                app.input = prev_input;
                app.input_cursor = prev_cursor;
            }
        },
        // Ctrl+Y: redo
        KeyCode::Char('y') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
            if let Some((next_input, next_cursor)) = app.redo_stack.pop() {
                app.undo_stack.push((app.input.clone(), app.input_cursor));
                app.input = next_input;
                app.input_cursor = next_cursor;
            }
        },
        KeyCode::Char('/')
            if {
                let cursor = app.input.floor_char_boundary(app.input_cursor.min(app.input.len()));
                cursor == 0 || app.input[..cursor].chars().next_back().is_none_or(|c| c.is_whitespace())
            } =>
        {
            // Activate slash command popup when '/' is typed at input start or after whitespace.
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.slash_command.start_pos = app.input_cursor;
            app.slash_command.active = true;
            app.slash_command.selected = 0;
            app.input.insert(app.input_cursor, '/');
            app.input_cursor += '/'.len_utf8();
        },
        KeyCode::Char('/') => {
            // Literal '/' (not at start / after whitespace).
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.input.insert(app.input_cursor, '/');
            app.input_cursor += '/'.len_utf8();
        },

        KeyCode::Char('@')
            if {
                let cursor = app.input.floor_char_boundary(app.input_cursor.min(app.input.len()));
                cursor == 0 || app.input[..cursor].chars().next_back().is_none_or(|c| c.is_whitespace())
            } =>
        {
            // Open the file picker when '@' is typed at the start of the input
            // or after whitespace.
            app.open_file_picker();
        },
        KeyCode::Char('@') => {
            // Literal '@' (e.g. inside an email address).
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.input.insert(app.input_cursor, '@');
            app.input_cursor += '@'.len_utf8();
        },
        KeyCode::Char(c) => {
            // Typing anything exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            // Deactivate slash command popup if the character is not valid for
            // a command name (letters, digits, '-', '_', '/', ':' are allowed).
            // Also reset selection when typing updates the query.
            if app.slash_command.active {
                app.slash_command.selected = 0;
                if !c.is_alphanumeric() && c != '-' && c != '_' && c != '/' && c != ':' {
                    app.slash_command.active = false;
                }
            }
            app.input.insert(app.input_cursor, c);
            app.input_cursor += c.len_utf8();
        },
        KeyCode::Backspace => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor > 0 {
                app.save_undo();
                let prev = prev_char_boundary(&app.input, app.input_cursor);
                app.input.remove(prev);
                app.input_cursor = prev;
            }
        },
        KeyCode::Delete => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor < app.input.len() {
                app.save_undo();
                app.input.remove(app.input_cursor);
            }
        },
        // Quick cursor movement (by word)
        KeyCode::Left
            if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
        {
            app.input_cursor = prev_word_boundary(&app.input, app.input_cursor);
        },
        KeyCode::Right
            if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
        {
            app.input_cursor = next_word_boundary(&app.input, app.input_cursor);
        },
        KeyCode::Left => {
            app.input_cursor = prev_char_boundary(&app.input, app.input_cursor);
        },
        KeyCode::Right => {
            app.input_cursor = next_char_boundary(&app.input, app.input_cursor);
        },
        KeyCode::Up => {
            let (line, _col) = cursor_line_col(&app.input, app.input_cursor);
            if line > 0 {
                // Multi-line: move cursor up within current input
                let new_col = _col.min(line_length(&app.input, line - 1));
                app.input_cursor = line_col_to_cursor(&app.input, line - 1, new_col);
            } else if !app.input_history.entries.is_empty() {
                // Cursor at first line → navigate history backward
                if app.input_history.index.is_none() {
                    // Enter history mode: save current input and start from the end
                    app.input_history.saved = app.input.clone();
                    app.input_history.index = Some(app.input_history.entries.len() - 1);
                } else if let Some(idx) = app.input_history.index
                    && idx > 0
                {
                    app.input_history.index = Some(idx - 1);
                }
                if let Some(idx) = app.input_history.index {
                    app.input = app.input_history.entries[idx].clone();
                    app.input_cursor = app.input.len();
                }
            }
        },
        KeyCode::Down => {
            let (line, _col) = cursor_line_col(&app.input, app.input_cursor);
            let next_len = line_length(&app.input, line + 1);
            if next_len > 0 || line_col_to_cursor(&app.input, line + 1, 0) < app.input.len() {
                // Multi-line: move cursor down within current input
                let new_col = _col.min(next_len);
                app.input_cursor = line_col_to_cursor(&app.input, line + 1, new_col);
            } else if app.input_history.index.is_some() {
                // Cursor at last line and in history mode → navigate forward
                if let Some(idx) = app.input_history.index {
                    if idx + 1 < app.input_history.entries.len() {
                        app.input_history.index = Some(idx + 1);
                        app.input = app.input_history.entries[idx + 1].clone();
                        app.input_cursor = app.input.len();
                    } else {
                        // Past the newest entry → restore saved input
                        app.input_history.index = None;
                        app.input = std::mem::take(&mut app.input_history.saved);
                        app.input_cursor = app.input.len();
                    }
                }
            }
        },
        KeyCode::Home => {
            let (line, _) = cursor_line_col(&app.input, app.input_cursor);
            app.input_cursor = line_col_to_cursor(&app.input, line, 0);
        },
        KeyCode::End => {
            let (line, _) = cursor_line_col(&app.input, app.input_cursor);
            app.input_cursor = line_col_to_cursor(&app.input, line, line_length(&app.input, line));
        },
        KeyCode::Esc => app.input_mode = InputMode::Normal,
        _ => {},
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use tact_protocol::{AgentUpdate, UserCommand};
    use tokio::sync::mpsc::unbounded_channel;

    use super::handle_insert_mode;
    use crate::widgets::state::{App, Status};

    fn make_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<UserCommand>) {
        let (agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel::<(String, String)>();
        drop(agent_tx);
        let app = App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx.clone(),
            PathBuf::from("."),
            Vec::new(),
            "test-session".to_string(),
            history_tx,
            "retro".to_string(),
            String::new(),
            Vec::new(),
        );
        (app, user_cmd_rx)
    }

    #[test]
    fn slash_quit_exits_without_submitting_task() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.input = "/quit".to_string();
        app.input_cursor = app.input.len();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(app.should_quit, "expected /quit to set should_quit");
        assert!(user_cmd_rx.try_recv().is_err(), "expected /quit not to dispatch SubmitTask");
    }

    #[test]
    fn slash_cancel_sends_cancel_without_submitting_task() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.status = Status::Planning;
        app.input = "/cancel".to_string();
        app.input_cursor = app.input.len();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        let cmd = user_cmd_rx.try_recv().expect("expected /cancel to dispatch UserCommand::Cancel");
        assert!(matches!(cmd, UserCommand::Cancel));
        assert!(user_cmd_rx.try_recv().is_err(), "expected /cancel not to dispatch SubmitTask");
    }

    #[test]
    fn slash_popup_enter_runs_selected_command() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.input = "/qu".to_string();
        app.input_cursor = app.input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(app.should_quit, "expected Enter to execute /quit");
        assert!(app.input.is_empty(), "expected input cleared after /quit");
        assert!(!app.slash_command.active);
        assert!(user_cmd_rx.try_recv().is_err(), "expected popup Enter not to submit a task");
    }

    #[test]
    fn slash_popup_enter_cancel_dispatches_while_executing() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.status = Status::Executing { current_step: 0, total: 1 };
        app.input = "/cancel".to_string();
        app.input_cursor = app.input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(matches!(user_cmd_rx.try_recv().expect("expected Cancel"), UserCommand::Cancel));
        assert!(app.input.is_empty());
        assert!(!app.slash_command.active);
    }

    #[test]
    fn slash_popup_enter_with_no_match_falls_back_to_submit() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.input = "/zzzzzz".to_string();
        app.input_cursor = app.input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        let cmd = user_cmd_rx.try_recv().expect("expected no-match slash Enter to submit task");
        match cmd {
            UserCommand::SubmitTask(task) => assert_eq!(task, "/zzzzzz"),
            other => panic!("expected SubmitTask, got {:?}", other),
        }
        assert!(!app.slash_command.active);
    }

    #[test]
    fn slash_cancel_idle_clears_input_and_does_not_dispatch() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.status = Status::Idle;
        app.input = "/cancel".to_string();
        app.input_cursor = app.input.len();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(app.input.is_empty());
        assert!(app.flash_msg.is_some());
        assert!(user_cmd_rx.try_recv().is_err(), "expected idle /cancel not to dispatch UserCommand");
    }

    #[test]
    fn slash_cancel_done_clears_input_and_does_not_dispatch() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.status = Status::Done;
        app.input = "/cancel".to_string();
        app.input_cursor = app.input.len();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(app.input.is_empty());
        assert!(app.flash_msg.is_some());
        assert!(user_cmd_rx.try_recv().is_err(), "expected Done /cancel not to dispatch UserCommand::Cancel");
    }

    #[test]
    fn submit_not_rejected_when_model_context_window_is_tiny() {
        // Input length guard must not use model_context_window as a char limit.
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.model_context_window = 5;
        app.input = "hello world".to_string();
        app.input_cursor = app.input.chars().count();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        let cmd =
            user_cmd_rx.try_recv().expect("expected short input to submit even when model_context_window is tiny");
        match cmd {
            UserCommand::SubmitTask(task) => assert_eq!(task, "hello world"),
            other => panic!("expected SubmitTask, got {:?}", other),
        }
    }

    #[test]
    fn submit_rejected_when_input_exceeds_char_limit() {
        // model_context_window must NOT control the insert-path char limit.
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.model_context_window = 200_000;
        app.input = "x".repeat(tact::consts::MAX_INPUT_CHARS + 1);
        app.input_cursor = app.input.chars().count();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(user_cmd_rx.try_recv().is_err(), "expected oversize input not to dispatch UserCommand");
        assert!(
            app.raw_messages
                .iter()
                .any(|m| m.contains("too long") || m.contains(&tact::consts::MAX_INPUT_CHARS.to_string())),
            "expected a system message indicating input is too long"
        );
        assert_eq!(
            app.input.chars().count(),
            tact::consts::MAX_INPUT_CHARS + 1,
            "expected oversize input to remain uncleared"
        );
    }

    #[test]
    fn submit_rejected_while_agent_busy() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.status = Status::Planning;
        app.input = "do something".to_string();
        app.input_cursor = app.input.chars().count();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(user_cmd_rx.try_recv().is_err(), "expected busy input not to submit task");
        assert!(app.flash_msg.is_some(), "expected a flash message while busy");
    }

    #[test]
    fn slash_popup_enter_on_skill_only_autocompletes() {
        use crate::widgets::state::SkillEntry;

        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.skills_data = vec![SkillEntry {
            name: "demo".into(),
            description: "Demo skill".into(),
            body: "Follow the checklist.".into(),
        }];
        app.input = "/dem".to_string();
        app.input_cursor = app.input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert_eq!(app.input, "/demo ");
        assert!(!app.slash_command.active);
        assert!(user_cmd_rx.try_recv().is_err(), "popup Enter on skill must not SubmitTask");
    }

    #[test]
    fn slash_popup_enter_on_plugin_autocompletes_for_subcommand() {
        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        // Partial query as when picking from the slash palette.
        app.input = "/pl".to_string();
        app.input_cursor = app.input.len();
        app.slash_command.active = true;
        app.slash_command.start_pos = 0;
        app.slash_command.selected = 0;

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert_eq!(app.input, "/plugin ", "Enter on /plugin should leave `/plugin ` so the user can type list/reload");
        assert_eq!(app.input_cursor, "/plugin ".len());
        assert!(!app.slash_command.active);
        assert!(
            !app.raw_messages.iter().any(|m| m.starts_with("Usage: /plugin")),
            "must not run bare /plugin yet: {:?}",
            app.raw_messages
        );
        assert!(user_cmd_rx.try_recv().is_err());
    }

    #[test]
    fn plugin_skill_autocomplete_accepts_namespace_colon() {
        use crate::widgets::state::SkillEntry;

        let (mut app, _user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.skills_data = vec![SkillEntry {
            name: "demo:review".into(),
            description: "Demo review skill".into(),
            body: "Review it.".into(),
        }];

        for c in "/demo:".chars() {
            handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE), &user_cmd_tx);
        }

        let commands = app.palette_commands();
        let refs: Vec<(&str, &str)> =
            commands.iter().map(|(command, description)| (command.as_str(), description.as_str())).collect();
        let matches =
            app.slash_command.matched_commands(&app.input, app.input_cursor, &refs, &super::skill_name_set(&app));
        assert!(app.slash_command.active);
        assert!(matches.iter().any(|(_, (name, _), _)| *name == "demo:review"));
    }

    #[test]
    fn slash_skill_without_args_invokes() {
        use crate::widgets::state::SkillEntry;

        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.skills_data = vec![SkillEntry {
            name: "demo".into(),
            description: "Demo skill".into(),
            body: "Follow the checklist.".into(),
        }];
        app.input = "/demo".to_string();
        app.input_cursor = app.input.len();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(matches!(app.status, Status::Planning));
        assert!(app.input.is_empty());
        match user_cmd_rx.try_recv().expect("SubmitTask") {
            UserCommand::SubmitTask(task) => {
                assert!(task.contains("<skill name=\"demo\">"));
                assert!(task.contains("Follow the checklist."));
                assert!(!task.contains("ARGUMENTS:"));
            },
            other => panic!("expected SubmitTask, got {other:?}"),
        }
        assert!(app.raw_messages.iter().any(|m| m.contains("/demo")), "user bubble should show slash command");
    }

    #[test]
    fn slash_skill_with_args_appends_arguments() {
        use crate::widgets::state::SkillEntry;

        let (mut app, mut user_cmd_rx) = make_app();
        let user_cmd_tx = app.user_cmd_tx.clone();
        app.skills_data = vec![SkillEntry {
            name: "demo".into(),
            description: "Demo skill".into(),
            body: "Follow the checklist.".into(),
        }];
        app.input = "/demo fix auth".to_string();
        app.input_cursor = app.input.len();

        handle_insert_mode(&mut app, KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &user_cmd_tx);

        assert!(matches!(app.status, Status::Planning));
        match user_cmd_rx.try_recv().expect("SubmitTask") {
            UserCommand::SubmitTask(task) => {
                assert!(task.contains("ARGUMENTS: fix auth"));
            },
            other => panic!("expected SubmitTask, got {other:?}"),
        }
        assert!(app.raw_messages.iter().any(|m| m.contains("/demo fix auth")), "user bubble should show slash + args");
    }
}
