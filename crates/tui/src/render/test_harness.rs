//! Shared TestBackend helpers for render-layer tests.

#![allow(dead_code)]

use ratatui::{
    Frame, Terminal,
    backend::TestBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier},
    widgets::ScrollbarState,
};

use super::{
    log::render_log_panel, render_bottom_bar, render_command_palette, render_file_picker,
    render_input_box, render_main_area, render_select_popup, render_slash_command_popup,
    render_status_bar,
};
use crate::widgets::state::App;
use tact_protocol::SubagentStepStatus;

/// Build a minimal `App` for render tests (ink theme, empty log).
pub fn make_app() -> App {
    crate::test_fixtures::TestApp::with_identity("render-test", "ink").app
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
    let bottom_height = 2u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(input_height),
            Constraint::Length(bottom_height),
        ])
        .split(size);

    app.log_scroll.height = chunks[1].height.saturating_sub(2);
    app.log_scroll.state = ScrollbarState::new(app.log.items.len().saturating_sub(1));

    render_status_bar(frame, chunks[0], app);
    render_main_area(frame, chunks[1], app);
    render_input_box(frame, chunks[2], app);
    render_bottom_bar(frame, chunks[3], app);

    // The four list popups render every frame: each one no-ops when its popup
    // is inactive and is responsible for clearing the mouse hit area it records
    // while active.
    render_command_palette(frame, chunks[1], app);
    render_select_popup(frame, chunks[1], app);
    render_file_picker(frame, chunks[1], app);
    render_slash_command_popup(frame, chunks[1], app);
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
    let terminal = render_main_area_terminal(app, width, height);
    buffer_text(terminal.backend().buffer())
}

/// Draw only the main content area into a `TestBackend` for buffer-level
/// assertions (styles/backgrounds), not just flattened text.
pub fn render_main_area_terminal(app: &mut App, width: u16, height: u16) -> Terminal<TestBackend> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal
        .draw(|frame| render_main_area(frame, frame.area(), app))
        .expect("draw");
    terminal
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

/// Open the subagent view on a live `spawn_subagent` card with one transcript
/// entry — the same path the host takes, for handler tests that need the view
/// up without caring how it was rendered.
///
/// The one step carries 20 lines of output, so its block is capped and draws
/// the overflow row that doubles as the per-block toggle.
pub fn seed_subagent_view(app: &mut App) {
    seed_subagent_view_with(app, SubagentStepStatus::Success);
}

/// The same, with the step's status chosen by the caller — `Running` for tests
/// about the live state (the animated phase glyph).
pub fn seed_subagent_view_with(app: &mut App, status: SubagentStepStatus) {
    use tact_protocol::{
        RuntimeEvent, SubagentStep, SubagentTranscriptEntry, ToolDetailKind, ToolPopupKind,
        ToolPresentationInfo, ToolVisualKind,
    };

    app.handle_runtime_event(RuntimeEvent::StepStarted {
        run_id: None,
        idx: 0,
        tool_id: "sa-1".into(),
        tool_name: "spawn_subagent".into(),
        arg_summary: "research".into(),
        arg_full: "## Task\n\n1. **Find** the price".into(),
        presentation: ToolPresentationInfo {
            visual_kind: ToolVisualKind::Subagent,
            display_name: "🤖 Subagent".into(),
            keep_full_live_output: true,
            detail: ToolDetailKind::Result,
            popup: ToolPopupKind::SubagentTranscript,
            compact_result_to_meta: false,
            keep_live: false,
        },
    });
    app.handle_runtime_event(RuntimeEvent::ToolTranscript {
        run_id: None,
        tool_id: "sa-1".into(),
        entry: SubagentTranscriptEntry::Step(SubagentStep {
            tool_id: "c1".into(),
            tool_name: "read_file".into(),
            arg_summary: "src/main.rs".into(),
            arg_full: "src/main.rs".into(),
            status,
            message: "20 lines".into(),
            detail: Some(
                (0..20)
                    .map(|i| format!("line {i}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            ),
            duration_us: Some(1_400),
            presentation: ToolPresentationInfo::generic("read_file"),
        }),
    });
    app.handle_runtime_event(RuntimeEvent::ToolTranscript {
        run_id: None,
        tool_id: "sa-1".into(),
        entry: SubagentTranscriptEntry::Assistant("Bitcoin is **up** today.".into()),
    });
    app.open_subagent_popup_by_id("sa-1");
    assert!(app.has_subagent_popup(), "the view must be open");
}
