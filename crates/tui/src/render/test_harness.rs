//! Shared TestBackend helpers for render-layer tests.

#![allow(dead_code)]

use super::{
    log::render_log_panel, render_bottom_bar, render_command_palette, render_file_picker,
    render_input_box, render_main_area, render_select_popup, render_slash_command_popup,
    render_status_bar,
};
use crate::widgets::state::{App, InputMode};
use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier},
    widgets::ScrollbarState,
};
use std::path::PathBuf;
use tokio::sync::mpsc::unbounded_channel;

/// Build a minimal `App` for render tests (retro theme, empty log).
pub fn make_app() -> App {
    let (_agent_tx, agent_rx) = unbounded_channel();
    let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
    let (history_tx, _history_rx) = unbounded_channel();
    App::new(
        agent_rx,
        user_cmd_tx,
        PathBuf::from("."),
        Vec::new(),
        "render-test".to_string(),
        history_tx,
        "retro".to_string(),
        false,
    )
}

/// Flatten a ratatui buffer into plain text (one row per line).
pub fn buffer_text(buf: &ratatui::buffer::Buffer) -> String {
    let mut out = String::new();
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            out.push_str(buf[(x, y)].symbol());
        }
        out.push('\n');
    }
    out
}

pub fn buffer_contains(buf: &ratatui::buffer::Buffer, needle: &str) -> bool {
    buffer_text(buf).contains(needle)
}

/// True if any cell in the buffer carries `modifier`.
pub fn buffer_has_modifier(buf: &ratatui::buffer::Buffer, modifier: Modifier) -> bool {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if buf[(x, y)].modifier.contains(modifier) {
                return true;
            }
        }
    }
    false
}

/// True if any cell uses the given background color.
pub fn buffer_has_bg(buf: &ratatui::buffer::Buffer, bg: Color) -> bool {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if buf[(x, y)].bg == bg {
                return true;
            }
        }
    }
    false
}

/// Column of the first cell whose symbol equals `ch` (useful for indent assertions).
pub fn buffer_first_char_x(buf: &ratatui::buffer::Buffer, ch: char) -> Option<u16> {
    for y in 0..buf.area.height {
        for x in 0..buf.area.width {
            if buf[(x, y)].symbol() == ch.to_string() {
                return Some(x);
            }
        }
    }
    None
}

/// Mirror the main TUI frame layout from `lib.rs` (status + main + input + bottom).
pub fn draw_full_ui(frame: &mut Frame, size: Rect, app: &mut App) {
    let input_lines = app.input.lines().count().clamp(1, 3) as u16;
    let input_height = input_lines + 2;
    let bottom_height = if app.shows_account_bar_row() { 3 } else { 2 };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(input_height),
            Constraint::Length(bottom_height),
        ])
        .split(size);

    app.log_scroll.height = chunks[1].height.saturating_sub(2);
    app.log_scroll.state = ScrollbarState::new(app.messages.len().saturating_sub(1));

    render_status_bar(frame, chunks[0], app);
    render_main_area(frame, chunks[1], app);
    render_input_box(frame, chunks[2], app);
    render_bottom_bar(frame, chunks[3], app);

    if app.input_mode == InputMode::Palette {
        render_command_palette(frame, size, app);
    }
    if app.input_mode == InputMode::Select {
        render_select_popup(frame, size, app);
    }
    if app.input_mode == InputMode::FilePicker {
        render_file_picker(frame, size, app);
    }
    if app.slash_command.active {
        render_slash_command_popup(frame, size, app);
    }
}

/// Render the Log panel into a terminal for buffer-level assertions.
pub fn render_log_panel_terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_log_panel(frame, frame.area(), app))
        .expect("draw");
    terminal
}

/// Draw only the Log panel and return flattened text.
pub fn render_log_panel_text(app: &mut App, width: u16, height: u16) -> String {
    let terminal = render_log_panel_terminal(app, width, height);
    buffer_text(terminal.backend().buffer())
}

/// Draw only the main content area (plan/log + overlay popups).
pub fn render_main_area_text(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_main_area(frame, frame.area(), app))
        .expect("draw");
    buffer_text(terminal.backend().buffer())
}

/// Draw the full UI into a `TestBackend` and return the rendered buffer text.
pub fn render_app_text(app: &mut App, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| draw_full_ui(frame, frame.area(), app))
        .expect("draw");
    buffer_text(terminal.backend().buffer())
}
