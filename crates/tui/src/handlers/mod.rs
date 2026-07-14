// Input handlers — split by mode.
mod file_picker;
mod insert;
mod mouse;
mod normal;
mod overlay;
mod palette;
mod select;

pub(crate) use file_picker::handle_file_picker_mode;
pub(crate) use insert::handle_insert_mode;
pub(crate) use mouse::handle_mouse_event;
pub(crate) use normal::handle_normal_mode;
pub(crate) use overlay::handle_overlay_key;
pub(crate) use palette::handle_palette_mode;
pub(crate) use select::handle_select_mode;

use crate::widgets::state::{App, Status};
use chrono::Local;
use tact_protocol::UserCommand;

/// Returns the byte index of the previous char boundary before `cursor`.
fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[..cursor]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Returns the byte index of the next char boundary after `cursor`.
fn next_char_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[cursor..]
        .chars()
        .next()
        .map(|c| cursor + c.len_utf8())
        .unwrap_or(cursor)
}

/// Returns the byte index at the start of the line that contains `cursor`.
fn start_of_line(s: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Returns the byte index at the end of the line that contains `cursor` (newline position, or string length for the last line).
fn end_of_line(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    s[cursor..]
        .find('\n')
        .map(|i| cursor + i)
        .unwrap_or(s.len())
}

/// Exit history navigation mode.
fn exit_history(app: &mut App) {
    app.input_history.index = None;
    app.input_history.saved.clear();
}

/// Compute the (line, column) of the cursor position, counting columns in characters.
fn cursor_line_col(s: &str, cursor: usize) -> (usize, usize) {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in s.char_indices() {
        if i >= cursor {
            break;
        }
        if c == '\n' {
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Returns the character length (excluding newline) of the given line.
fn line_length(s: &str, target_line: usize) -> usize {
    let mut line = 0;
    let mut len = 0;
    for c in s.chars() {
        if line == target_line {
            if c == '\n' {
                break;
            }
            len += 1;
        } else if c == '\n' {
            line += 1;
        }
    }
    len
}

/// Convert (line, column) to a byte index.
fn line_col_to_cursor(s: &str, target_line: usize, target_col: usize) -> usize {
    let mut line = 0;
    let mut col = 0;
    for (i, c) in s.char_indices() {
        if line == target_line && col == target_col {
            return i;
        }
        if c == '\n' {
            if line == target_line {
                return i;
            }
            line += 1;
            col = 0;
        } else {
            col += 1;
        }
    }
    s.len()
}

/// Returns true if the character is a word character (alphanumeric or underscore).
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Returns the byte index of the word start before `cursor` (backward-delete word).
fn prev_word_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    let mut pos = cursor;
    let mut chars = s[..cursor].chars().rev().peekable();

    // Skip whitespace
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            pos -= c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }

    // Record the type of the first non-whitespace char, then skip same-type chars
    if let Some(&first) = chars.peek() {
        if is_word_char(first) {
            while let Some(&c) = chars.peek() {
                if is_word_char(c) {
                    pos -= c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() && !is_word_char(c) {
                    pos -= c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }

    pos
}

/// Returns the byte index of the word end after `cursor` (forward-delete word).
fn next_word_boundary(s: &str, cursor: usize) -> usize {
    let cursor = s.floor_char_boundary(cursor.min(s.len()));
    let mut pos = cursor;
    let mut chars = s[cursor..].chars().peekable();

    // Skip whitespace
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            pos += c.len_utf8();
            chars.next();
        } else {
            break;
        }
    }

    // Record the type of the first non-whitespace char, then skip same-type chars
    if let Some(&first) = chars.peek() {
        if is_word_char(first) {
            while let Some(&c) = chars.peek() {
                if is_word_char(c) {
                    pos += c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if !c.is_whitespace() && !is_word_char(c) {
                    pos += c.len_utf8();
                    chars.next();
                } else {
                    break;
                }
            }
        }
    }

    pos
}

/// Execute a command from palette/slash input.
///
/// Returns whether the caller should clear the input box afterwards.
pub(super) struct CommandExecOutcome {
    pub handled: bool,
    pub clear_input: bool,
}

pub(super) fn execute_palette_command(app: &mut App, cmd: &str) -> CommandExecOutcome {
    // Handle skill commands — each skill is a palette command (Claude Code style)
    if let Some((_name, body)) = app
        .skills_data
        .iter()
        .find(|(name, _)| name.as_str() == cmd)
    {
        let body = body.clone();
        let user_input = std::mem::take(&mut app.input);
        let combined = if user_input.is_empty() {
            body
        } else {
            format!("{body}\n\nARGUMENTS: {user_input}")
        };
        let _ = app.user_cmd_tx.send(UserCommand::SubmitTask(combined));
        return CommandExecOutcome {
            handled: true,
            clear_input: true,
        };
    }

    match cmd {
        "theme" => {
            app.toggle_theme();
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "model" => {
            crate::handlers::select::start_model_picker(app);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "save" => {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            let path = std::env::temp_dir().join(format!("agent_log_{timestamp}.txt"));
            if let Ok(mut file) = std::fs::File::create(&path) {
                use std::io::Write;
                for msg in &app.raw_messages {
                    writeln!(file, "{}", msg).ok();
                }
                let msgs = app.msgs();
                app.add_system_message(
                    msgs.log_saved_tmpl
                        .replace("{}", &path.display().to_string()),
                );
            } else {
                let msgs = app.msgs();
                app.add_system_message(msgs.log_save_failed.to_string());
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "quit" => {
            app.should_quit = true;
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "help" => {
            app.show_help = !app.show_help;
            app.show_history = false;
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "history" => {
            app.show_history = !app.show_history;
            app.show_help = false;
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "skills" => {
            app.add_system_message(format!(
                "# Available skills\n\n{}",
                app.skills_description,
            ));
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "skill-reload" => {
            let count = reload_skills(app);
            let msg = app.msgs().skill_reloaded_tmpl.replace("{}", &count.to_string());
            app.add_system_message(msg);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "cancel" => {
            // Only cancel an in-flight task; Idle and Done have nothing to abort.
            if matches!(app.status, Status::Planning | Status::Executing { .. }) {
                let _ = app.user_cmd_tx.send(UserCommand::Cancel);
            } else {
                app.flash_msg = Some((
                    app.msgs().cancel_noop_msg.to_string(),
                    std::time::Instant::now(),
                ));
            }
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "balance" => {
            if app.account_rx.is_none() {
                return CommandExecOutcome {
                    handled: true,
                    clear_input: true,
                };
            }
            let _ = app.user_cmd_tx.send(UserCommand::QueryBalance);
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        "lang" => {
            app.toggle_language();
            CommandExecOutcome {
                handled: true,
                clear_input: true,
            }
        }
        _ => CommandExecOutcome {
            handled: false,
            clear_input: false,
        },
    }
}

/// Reload skills from disk. Returns number of skills loaded.
fn reload_skills(app: &mut App) -> usize {
    match tact::skill::get_skill_registry(&app.work_dir) {
        Ok(reg) => {
            app.skills_description = reg.describe_available();
            app.skills_data = reg
                .skills()
                .iter()
                .map(|(name, doc)| (name.clone(), doc.body.clone()))
                .collect();
            app.skills_data.len()
        }
        Err(e) => {
            tracing::warn!("Failed to reload skills: {e}");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::execute_palette_command;
    use crate::widgets::state::{App, Status};
    use std::path::PathBuf;
    use tact_protocol::{AgentUpdate, UserCommand};
    use tokio::sync::mpsc::unbounded_channel;

    fn make_app() -> (App, tokio::sync::mpsc::UnboundedReceiver<UserCommand>) {
        let (agent_tx, agent_rx) = unbounded_channel::<AgentUpdate>();
        let (user_cmd_tx, user_cmd_rx) = unbounded_channel::<UserCommand>();
        let (history_tx, _history_rx) = unbounded_channel::<(String, String)>();
        drop(agent_tx);
        let app = App::new(
            agent_rx,
            None,
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
    fn palette_commands_are_all_handled() {
        let (mut app, _user_cmd_rx) = make_app();
        let (_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
        app.account_rx = Some(account_rx);
        let cmds = app.palette_commands();
        let commands: Vec<(&str, &str)> = cmds.iter().map(|(c, d)| (c.as_str(), d.as_str())).collect();

        for (cmd, _desc) in &commands {
            if *cmd == "cancel" {
                app.status = Status::Planning;
            }
            let outcome = execute_palette_command(&mut app, cmd);
            assert!(outcome.handled, "expected command `{cmd}` to be handled");
        }
    }

    #[test]
    fn unknown_command_is_not_handled() {
        let (mut app, _user_cmd_rx) = make_app();
        let outcome = execute_palette_command(&mut app, "nonexistent");
        assert!(!outcome.handled);
        assert!(!outcome.clear_input);
    }

    #[test]
    fn cancel_while_done_is_noop() {
        let (mut app, mut user_cmd_rx) = make_app();
        app.status = Status::Done;
        let outcome = execute_palette_command(&mut app, "cancel");
        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(app.flash_msg.is_some());
        assert!(
            user_cmd_rx.try_recv().is_err(),
            "Done must not dispatch Cancel"
        );
    }

    #[test]
    fn cancel_while_executing_dispatches() {
        let (mut app, mut user_cmd_rx) = make_app();
        app.status = Status::Executing {
            current_step: 0,
            total: 1,
        };
        let outcome = execute_palette_command(&mut app, "cancel");
        assert!(outcome.handled);
        assert!(outcome.clear_input);
        assert!(matches!(
            user_cmd_rx.try_recv().expect("expected Cancel"),
            UserCommand::Cancel
        ));
    }

    #[test]
    fn theme_command_toggles_theme() {
        use crate::theme::ThemeName;

        let (mut app, _user_cmd_rx) = make_app();
        assert_eq!(app.theme.name, ThemeName::Retro);
        let outcome = execute_palette_command(&mut app, "theme");
        assert!(outcome.handled);
        assert_ne!(app.theme.name, ThemeName::Retro);
    }
}
