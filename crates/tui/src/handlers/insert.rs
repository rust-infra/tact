use crate::widgets::state::{InputMode, Status};
use crate::widgets::state::App;
use crossterm::event::{KeyCode, KeyEvent};
use tact_core::UserCommand;
use tokio::sync::mpsc::UnboundedSender;
use super::{start_of_line, cursor_line_col, end_of_line, exit_history, line_col_to_cursor, line_length, next_char_boundary, next_word_boundary, prev_char_boundary, prev_word_boundary};

pub(crate) fn handle_insert_mode(
    app: &mut App,
    key: KeyEvent,
    user_cmd_tx: &UnboundedSender<UserCommand>,
) {
    match key.code {
        // --- Slash command popup shortcuts (only when active) ---
        KeyCode::Up if app.slash_command.active => {
            let n = app.slash_command.matched_commands(
                &app.input,
                app.input_cursor,
                crate::widgets::state::PALETTE_COMMANDS,
            ).len();
            if n > 0 {
                app.slash_command.selected = app.slash_command.selected.saturating_sub(1);
            }
            return;
        }
        KeyCode::Down if app.slash_command.active => {
            let n = app.slash_command.matched_commands(
                &app.input,
                app.input_cursor,
                crate::widgets::state::PALETTE_COMMANDS,
            ).len();
            if n > 0 {
                let max = n.saturating_sub(1);
                app.slash_command.selected = (app.slash_command.selected + 1).min(max);
            }
            return;
        }
        KeyCode::Tab if app.slash_command.active => {
            let cmds = app.slash_command.matched_commands(
                &app.input,
                app.input_cursor,
                crate::widgets::state::PALETTE_COMMANDS,
            );
            let sel = app.slash_command.selected.min(cmds.len().saturating_sub(1));
            if let Some(&(_idx, (cmd, _desc), _score)) = cmds.get(sel) {
                let start = app.slash_command.start_pos;
                let end = app.input_cursor;
                let replacement = format!("/{cmd} ");
                app.input.replace_range(start..end, &replacement);
                app.input_cursor = start + cmd.len() + 2;
                app.slash_command.active = false;
            }
            return;
        }
        KeyCode::Esc if app.slash_command.active => {
            app.slash_command.active = false;
            return;
        }
        KeyCode::Char(' ') if app.slash_command.active => {
            app.slash_command.active = false;
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.input.insert(app.input_cursor, ' ');
            app.input_cursor += 1;
            return;
        }
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
            return;
        }
                // --- End slash command shortcuts ---

        KeyCode::Enter => {
            // Deactivate slash command on submit
            app.slash_command.active = false;
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::SHIFT)
                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT)
            {
                // insert blank charater for writing next line
                app.save_undo();
                app.input.insert(app.input_cursor, '\n');
                app.input_cursor += 1;
            } else if !app.input.is_empty() {
                // Save to history (skip consecutive duplicates)
                let task_text = app.input.clone();
                if app.input_history.entries.last() != Some(&task_text) {
                    app.input_history.entries.push(task_text.clone());
                    app.save_history(&task_text);
                }
                app.input_history.index = None;
                app.input_history.saved.clear();
                let task = std::mem::take(&mut app.input);
                app.input_cursor = 0;
                // If waiting for approval, reject it before starting new task
                let old_status = std::mem::replace(&mut app.status, Status::Planning);
                if let Status::WaitingForUser { approval_tx, .. } = old_status {
                    let _ = approval_tx.send(false);
                    let msgs = app.msgs();
                    app.add_system_message(msgs.approval_cancelled.to_string());
                }
                let blank_task = format!("{}", task.clone());
                app.add_user_message(blank_task);
                app.plan.reset();
                app.last_prompt_elapsed_secs = None;
                app.task_start_time = Some(chrono::Local::now());
                // Send command to agent
                let _ = user_cmd_tx.send(UserCommand::SubmitTask(task));
            }
        }
        // Quick word delete
        KeyCode::Char('w')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor > 0 {
                app.save_undo();
                let pos = prev_word_boundary(&app.input, app.input_cursor);
                app.input.drain(pos..app.input_cursor);
                app.input_cursor = pos;
            }
        }
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
        }
        // Ctrl+A: jump to input start
        KeyCode::Char('a')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.input_cursor = 0;
        }
        // Ctrl+E: jump to input end
        KeyCode::Char('e')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.input_cursor = app.input.len();
        }
        // Ctrl+K: kill to end of line; if cursor is at end of line, delete newline to merge with next line
        KeyCode::Char('k')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            exit_history(app);
            let end = end_of_line(&app.input, app.input_cursor);
            let delete_end = if end < app.input.len() { end + 1 } else { end };
            if app.input_cursor < delete_end {
                app.save_undo();
            }
            app.input.drain(app.input_cursor..delete_end);
        }
        KeyCode::Char('d') if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor < app.input.len() {
                app.save_undo();
                let pos = next_word_boundary(&app.input, app.input_cursor);
                app.input.drain(app.input_cursor..pos);
            }
        }
        // Ctrl+U: kill to beginning of line
        KeyCode::Char('u')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            exit_history(app);
            let start = start_of_line(&app.input, app.input_cursor);
            if start < app.input_cursor {
                app.save_undo();
            }
            app.input.drain(start..app.input_cursor);
            app.input_cursor = start;
        }
        // Ctrl+D: delete character after cursor (only when input is non-empty)
        KeyCode::Char('d')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            exit_history(app);
            if app.input_cursor < app.input.len() {
                app.save_undo();
                let next = next_char_boundary(&app.input, app.input_cursor);
                app.input.drain(app.input_cursor..next);
            }
        }
        // Ctrl+Home: jump to input start
        KeyCode::Home
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.input_cursor = 0;
        }
        // Ctrl+End: jump to input end
        KeyCode::End
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            app.input_cursor = app.input.len();
        }
        // Ctrl+Backspace: delete previous word (consistent with Ctrl+W / Alt+Backspace)
        KeyCode::Backspace
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            exit_history(app);
            if app.input_cursor > 0 {
                app.save_undo();
                let pos = prev_word_boundary(&app.input, app.input_cursor);
                app.input.drain(pos..app.input_cursor);
                app.input_cursor = pos;
            }
        }
        // Ctrl+P: history back (unrestricted by cursor line position)
        KeyCode::Char('p')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            if !app.input_history.entries.is_empty() {
                if app.input_history.index.is_none() {
                    app.input_history.saved = app.input.clone();
                    app.input_history.index = Some(app.input_history.entries.len() - 1);
                } else if let Some(idx) = app.input_history.index {
                    if idx > 0 {
                        app.input_history.index = Some(idx - 1);
                    }
                }
                if let Some(idx) = app.input_history.index {
                    app.input = app.input_history.entries[idx].clone();
                    app.input_cursor = app.input.len();
                }
            }
        }
        // Ctrl+N: history forward (unrestricted by cursor line position)
        KeyCode::Char('n')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
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
        }
        // Ctrl+Z: undo
        KeyCode::Char('z')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            if let Some((prev_input, prev_cursor)) = app.undo_stack.pop() {
                app.redo_stack
                    .push((app.input.clone(), app.input_cursor));
                app.input = prev_input;
                app.input_cursor = prev_cursor;
            }
        }
        // Ctrl+Y: redo
        KeyCode::Char('y')
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL) =>
        {
            if let Some((next_input, next_cursor)) = app.redo_stack.pop() {
                app.undo_stack
                    .push((app.input.clone(), app.input_cursor));
                app.input = next_input;
                app.input_cursor = next_cursor;
            }
        }
        KeyCode::Char('/')
            if {
                let cursor = app
                    .input
                    .floor_char_boundary(app.input_cursor.min(app.input.len()));
                cursor == 0
                    || app.input[..cursor]
                        .chars()
                        .next_back()
                        .map_or(true, |c| c.is_whitespace())
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
        }
        KeyCode::Char('/') => {
            // Literal '/' (not at start / after whitespace).
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.input.insert(app.input_cursor, '/');
            app.input_cursor += '/'.len_utf8();
        }

        KeyCode::Char('@')
            if {
                let cursor = app
                    .input
                    .floor_char_boundary(app.input_cursor.min(app.input.len()));
                cursor == 0
                    || app.input[..cursor]
                        .chars()
                        .next_back()
                        .map_or(true, |c| c.is_whitespace())
            } =>
        {
            // Open the file picker when '@' is typed at the start of the input
            // or after whitespace.
            app.open_file_picker();
        }
        KeyCode::Char('@') => {
            // Literal '@' (e.g. inside an email address).
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            app.input.insert(app.input_cursor, '@');
            app.input_cursor += '@'.len_utf8();
        }
        KeyCode::Char(c) => {
            // Typing anything exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            app.save_undo();
            // Deactivate slash command popup if the character is not valid for
            // a command name (letters, digits, '-', '_', '/' are allowed).
            // Also reset selection when typing updates the query.
            if app.slash_command.active {
                app.slash_command.selected = 0;
                if !c.is_alphanumeric() && c != '-' && c != '_' && c != '/' {
                    app.slash_command.active = false;
                }
            }
            app.input.insert(app.input_cursor, c);
            app.input_cursor += c.len_utf8();
        }
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
        }
        KeyCode::Delete => {
            // Editing a history entry exits history navigation
            app.input_history.index = None;
            app.input_history.saved.clear();
            if app.input_cursor < app.input.len() {
                app.save_undo();
                app.input.remove(app.input_cursor);
            }
        }
        // Quick cursor movement (by word)
        KeyCode::Left
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
        {
            app.input_cursor = prev_word_boundary(&app.input, app.input_cursor);
        }
        KeyCode::Right
            if key
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
                || key.modifiers.contains(crossterm::event::KeyModifiers::ALT) =>
        {
            app.input_cursor = next_word_boundary(&app.input, app.input_cursor);
        }
        KeyCode::Left => {
            app.input_cursor = prev_char_boundary(&app.input, app.input_cursor);
        }
        KeyCode::Right => {
            app.input_cursor = next_char_boundary(&app.input, app.input_cursor);
        }
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
                } else if let Some(idx) = app.input_history.index {
                    if idx > 0 {
                        app.input_history.index = Some(idx - 1);
                    }
                }
                if let Some(idx) = app.input_history.index {
                    app.input = app.input_history.entries[idx].clone();
                    app.input_cursor = app.input.len();
                }
            }
        }
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
        }
        KeyCode::Home => {
            let (line, _) = cursor_line_col(&app.input, app.input_cursor);
            app.input_cursor = line_col_to_cursor(&app.input, line, 0);
        }
        KeyCode::End => {
            let (line, _) = cursor_line_col(&app.input, app.input_cursor);
            app.input_cursor = line_col_to_cursor(&app.input, line, line_length(&app.input, line));
        }
        KeyCode::Esc => app.input_mode = InputMode::Normal,
        _ => {}
    }
}
