// Input handlers — split by mode.
mod file_picker;
mod insert;
mod normal;
mod palette;
mod search;
mod select;

pub(crate) use file_picker::handle_file_picker_mode;
pub(crate) use insert::handle_insert_mode;
pub(crate) use normal::handle_normal_mode;
pub(crate) use palette::handle_palette_mode;
pub(crate) use search::handle_search_mode;
pub(crate) use select::handle_select_mode;

use crate::widgets::state::{App, FocusedPanel, InputMode, PALETTE_COMMANDS, Status};
use arboard::Clipboard;
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;
use chrono::Local;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::widgets::{ListState, ScrollbarState};
use tact_core::UserCommand;
use tokio::sync::mpsc::UnboundedSender;

fn copy_text(app: &mut App, text: &str) {
    let preview = text.chars().take(40).collect::<String>();

    // 1. Try native clipboard
    if let Ok(mut clip) = Clipboard::new() {
        if clip.set_text(text).is_ok() {
            let msgs = app.msgs();
            app.add_system_message(msgs.copied_tmpl.replace("{}", &preview));
            return;
        }
    }

    // 2. Fallback: OSC 52 terminal clipboard (for SSH / tmux scenarios)
    let encoded = BASE64.encode(text);
    let osc52 = format!("\x1b]52;c;{}\x07", encoded);
    if std::io::Write::write_all(&mut std::io::stdout(), osc52.as_bytes()).is_ok() {
        let msgs = app.msgs();
        app.add_system_message(msgs.copied_terminal_tmpl.replace("{}", &preview));
        return;
    }

    // 3. Last resort: save to internal buffer
    app.clipboard_buffer = text.to_string();
    let msgs = app.msgs();
    app.add_system_message(msgs.copied_internal_tmpl.replace("{}", &preview));
}

/// Returns the byte index of the previous char boundary before `cursor`.
fn prev_char_boundary(s: &str, cursor: usize) -> usize {
    s[..cursor]
        .char_indices()
        .last()
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Returns the byte index of the next char boundary after `cursor`.
fn next_char_boundary(s: &str, cursor: usize) -> usize {
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
    s[..cursor].rfind('\n').map(|i| i + 1).unwrap_or(0)
}

/// Returns the byte index at the end of the line that contains `cursor` (newline position, or string length for the last line).
fn end_of_line(s: &str, cursor: usize) -> usize {
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

/// Execute the selected command in the command palette.
pub(super) fn execute_palette_command(app: &mut App, cmd: &str) {
    match cmd {
        "theme" => app.toggle_theme(),
        "save" => {
            let timestamp = Local::now().format("%Y%m%d_%H%M%S");
            let filename = format!("agent_log_{}.txt", timestamp);
            if let Ok(mut file) = std::fs::File::create(&filename) {
                use std::io::Write;
                for msg in &app.raw_messages {
                    writeln!(file, "{}", msg).ok();
                }
                let msgs = app.msgs();
                app.add_system_message(msgs.log_saved_tmpl.replace("{}", &filename));
            } else {
                let msgs = app.msgs();
                app.add_system_message(msgs.log_save_failed.to_string());
            }
        }
        "quit" => app.should_quit = true,
        "help" => {
            app.show_help = !app.show_help;
            app.show_history = false;
        }
        "history" => {
            app.show_history = !app.show_history;
            app.show_help = false;
        }
        "search" => {
            app.input_mode = InputMode::Search;
            app.cmd_line.clear();
        }
        "cancel" => {
            if !matches!(app.status, Status::Idle) {
                let _ = app.user_cmd_tx.send(UserCommand::Cancel);
            }
        }
        "balance" => {
            let _ = app.user_cmd_tx.send(UserCommand::QueryBalance);
        }
        "lang" => {
            app.toggle_language();
        }
        "party" => {
            app.toggle_party_mode();
        }
        _ => {}
    }
}
