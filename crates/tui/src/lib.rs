// TUI main module
// Initializes the terminal (raw mode + alternate screen), drives the render loop,
// and dispatches input events.
// Bridges Agent status updates and terminal events to the App state and
// submodule render/handler functions.

mod handlers;
mod i18n;
mod render;
mod theme;
mod theme_detection;

mod widgets;

use crate::handlers::{
    handle_file_picker_mode, handle_insert_mode, handle_normal_mode, handle_palette_mode,
    handle_search_mode, handle_select_mode,
};
use crate::render::{
    render_bottom_bar, render_command_palette, render_file_picker, render_input_box,
    render_main_area, render_select_popup, render_slash_command_popup, render_status_bar,
};
use crate::widgets::state::{App, FocusedPanel, InputMode, Status};
use anyhow::Result;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        MouseButton, MouseEventKind, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::widgets::ScrollbarState;
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
};
use std::path::PathBuf;
use std::{
    io,
    time::Duration,
    time::{SystemTime, UNIX_EPOCH},
};
use tact_protocol::{AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_stream::StreamExt;

// ========== Main Loop ==========

/// Returns a random interval between 5–15 seconds to avoid rate-limiting on balance queries.
fn random_balance_duration() -> Duration {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    Duration::from_secs(5 + (nanos % 11) as u64)
}

/// TUI entry point: initializes the terminal, starts the event loop, runs until the user exits.
pub async fn run_tui(
    agent_rx: UnboundedReceiver<AgentUpdate>,
    user_cmd_tx: UnboundedSender<UserCommand>,
    work_dir: PathBuf,
    input_history_entries: Vec<String>,
    session_id: String,
    history_save_tx: UnboundedSender<(String, String)>,
    theme: String,
) -> Result<()> {
    // Enter raw mode, enable the alternate screen buffer, capture mouse events
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture,
        PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
    )?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Initialize application state
    let mut app = App::new(
        agent_rx,
        user_cmd_tx.clone(),
        work_dir,
        input_history_entries,
        session_id,
        history_save_tx,
        theme,
    );
    app.add_startup_logo();
    let msgs = app.msgs();
    app.add_system_message(msgs.startup_welcome.to_string());
    app.add_system_message(msgs.startup_mode_hint.to_string());

    // Record the previous terminal size so we can recompute layout on resize
    let term_size = terminal.size()?;
    let mut last_size = Rect::new(0, 0, term_size.width, term_size.height);

    // Use an async EventStream to read terminal events — no dedicated blocking thread needed
    let (event_tx, mut event_rx) = unbounded_channel::<Event>();
    let mut event_stream = EventStream::new();
    tokio::spawn(async move {
        while let Some(Ok(ev)) = event_stream.next().await {
            if event_tx.send(ev).is_err() {
                break;
            }
        }
    });

    let mut balance_timer: std::pin::Pin<Box<tokio::time::Sleep>> =
        Box::pin(tokio::time::sleep(random_balance_duration()));
    loop {
        // Process all Agent status updates first, ensuring rendering and event handling
        // use consistent state.
        // This matters: operations like close_active_thinking_block insert rows into the
        // messages array. If these updates were processed after rendering, the
        // log_scroll.visual_start computed during rendering would be inconsistent with the
        // actual message array, causing mouse clicks to map to wrong lines.
        while let Ok(update) = app.agent_rx.try_recv() {
            app.handle_agent_update(update);
        }

        // Only repaint when the dirty flag is true or in Done state, avoiding pointless
        // high-frequency refreshes while idle.
        // Done state transitions to Idle after 2s timeout; must keep rendering to check
        // the clock.
        if app.dirty || matches!(app.status, Status::Done) || !app.tools.active.is_empty() {
            // Advance spinner frame when in an active state
            if !matches!(app.status, Status::Idle | Status::Done) {
                app.spinner_frame = (app.spinner_frame + 1) % 10;
            }
            terminal.draw(|f| {
                let size = f.area();
                // Input box height auto-expands with content (1–3 lines of content + 2 for border)
                let input_lines = app.input.lines().count().max(1).min(3) as u16;
                let input_height = input_lines + 2;
                // Third row is balance info only; omit when unavailable to reclaim space.
                let bottom_height = if app.balance_info.is_some() { 3 } else { 2 };
                let log_area = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(3),
                        Constraint::Length(input_height),
                        Constraint::Length(bottom_height),
                    ])
                    .split(size)[1];
                if size != last_size {
                    last_size = size;
                    app.log_scroll.state =
                        ScrollbarState::new(app.messages.len().saturating_sub(1));
                }
                let main_chunks = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
                    .split(log_area);
                app.log_scroll.height = main_chunks[1].height.saturating_sub(2);
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(3),
                        Constraint::Length(input_height),
                        Constraint::Length(bottom_height),
                    ])
                    .split(size);
                render_status_bar(f, chunks[0], &app);
                render_main_area(f, chunks[1], &mut app);
                render_input_box(f, chunks[2], &mut app);
                render_bottom_bar(f, chunks[3], &app);
                if app.input_mode == InputMode::Palette {
                    render_command_palette(f, size, &app);
                }
                if app.input_mode == InputMode::Select {
                    render_select_popup(f, size, &app);
                }
                if app.input_mode == InputMode::FilePicker {
                    render_file_picker(f, size, &app);
                }
                if app.slash_command.active {
                    render_slash_command_popup(f, size, &app);
                }
            })?;
            // Clear dirty flag after painting; next frame only repaints when state changes.
            app.dirty = false;
        }

        // Done state highlight: auto-revert to Idle after 2s so shortcut hints aren't
        // permanently hidden
        if let Status::Done = app.status
            && let Some(done_time) = app.task_done_time
            && chrono::Local::now()
                .signed_duration_since(done_time)
                .num_seconds()
                >= 2
        {
            app.status = Status::Idle;
            app.task_done_time = None;
            app.dirty = true; // status bar needs repaint to show Idle
        }

        // flash_msg auto-clears after 3s
        if app
            .flash_msg
            .as_ref()
            .map(|(_, t)| t.elapsed().as_secs() >= 3)
            .unwrap_or(false)
        {
            app.flash_msg = None;
            app.dirty = true;
        }

        // Adaptive idle polling interval: adjust the event wait timeout based on state.
        // - Done state: 200ms, frequently check the 2s → Idle transition
        // - Dirty flag set: 10ms, quickly trigger a rerender
        // - Active (Planning/Executing/WaitingForUser): 150ms to animate spinner
        // - Fully idle: 1000ms, reduce CPU wake frequency
        let idle_ms = if matches!(app.status, Status::Done) || app.flash_msg.is_some() {
            200u64
        } else if app.dirty {
            10u64
        } else if !matches!(app.status, Status::Idle) {
            150u64
        } else {
            1000u64
        };
        tokio::select! {
            _ = balance_timer.as_mut() => {
                // Periodic DeepSeek balance query (random 5–15 second interval)
                let _ = user_cmd_tx.send(UserCommand::QueryBalance);
                balance_timer = Box::pin(tokio::time::sleep(random_balance_duration()));
            }
            event = event_rx.recv() => {
                match event {
                    Some(event) => {
                        app.dirty = true;
                match event {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        // Global shortcuts: active in any input mode
                        if key.modifiers.contains(KeyModifiers::CONTROL) {
                            match key.code {
                                KeyCode::Char('c') => {
                                    app.should_quit = true;
                                }
                                KeyCode::Char('h') => {
                                    app.show_history = !app.show_history;
                                    app.show_help = false;
                                }
                                KeyCode::Char('t') => {
                                    app.toggle_theme();
                                }
                                KeyCode::Char('l') => {
                                    app.toggle_language();
                                }
                                KeyCode::Char('?') => {
                                    app.show_help = !app.show_help;
                                    app.show_history = false;
                                }
                                _ => {}
                            }
                        } else if app.tools.popup.is_some() {
                            // Keyboard handling for file diff popup
                            match key.code {
                                KeyCode::Esc => {
                                    app.close_diff_popup();
                                }
                                KeyCode::Char('y') => {
                                    app.copy_diff_popup();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    app.diff_popup_scroll_down();
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.diff_popup_scroll_up();
                                }
                                _ => {}
                            }
                        } else if app.code_popup.is_some() {
                            // Keyboard handling for code block popup
                            match key.code {
                                KeyCode::Esc => {
                                    app.close_code_popup();
                                }
                                KeyCode::Char('y') => {
                                    app.copy_code_popup();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    app.code_popup_scroll_down();
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.code_popup_scroll_up();
                                }
                                KeyCode::Char('G') => {
                                    // Jump to bottom — use a large value; rendering will clamp
                                    if let Some(ref mut p) = app.code_popup {
                                        p.scroll = u16::MAX;
                                    }
                                }
                                KeyCode::Char('g') => {
                                    if let Some(ref mut p) = app.code_popup {
                                        p.scroll = 0;
                                    }
                                }
                                _ => {}
                            }
                        } else if app.thinking.popup.is_some() {
                            // Keyboard handling for thinking popup
                            match key.code {
                                KeyCode::Esc => {
                                    app.close_thinking_popup();
                                }
                                KeyCode::Char('y') => {
                                    app.copy_thinking_popup();
                                }
                                KeyCode::Char('j') | KeyCode::Down => {
                                    app.thinking_popup_scroll_down();
                                }
                                KeyCode::Char('k') | KeyCode::Up => {
                                    app.thinking_popup_scroll_up();
                                }
                                _ => {}
                            }
                        } else if key.code == KeyCode::Tab {
                            app.focused_panel = match app.focused_panel {
                                crate::widgets::state::FocusedPanel::Log => crate::widgets::state::FocusedPanel::Plan,
                                crate::widgets::state::FocusedPanel::Plan => crate::widgets::state::FocusedPanel::Log,
                            };
                        } else if (app.show_help || app.show_history) && key.code == KeyCode::Esc {
                            app.show_help = false;
                            app.show_history = false;
                        } else {
                            // === Konami Code easter egg detection ===
                            // ↑ ↑ ↓ ↓ ← → ← → b a
                            let konami_seq: &[KeyCode] = &[
                                KeyCode::Up,
                                KeyCode::Up,
                                KeyCode::Down,
                                KeyCode::Down,
                                KeyCode::Left,
                                KeyCode::Right,
                                KeyCode::Left,
                                KeyCode::Right,
                                KeyCode::Char('b'),
                                KeyCode::Char('a'),
                            ];
                            let expected = konami_seq[app.konami_progress as usize];
                            if key.code == expected && key.modifiers.is_empty() {
                                app.konami_progress += 1;
                                if app.konami_progress >= 10 {
                                    app.toggle_party_mode();
                                    app.konami_progress = 0;
                                }
                            } else if key.code != KeyCode::Up
                                && key.code != KeyCode::Down
                                && key.code != KeyCode::Left
                                && key.code != KeyCode::Right
                            {
                                // Non-arrow key input interrupts the sequence
                                app.konami_progress = 0;
                            }
                            // Dispatch to the key handler for the current input mode
                            match app.input_mode {
                                InputMode::Normal => {
                                    handle_normal_mode(&mut app, key, &user_cmd_tx)
                                }
                                InputMode::Insert => {
                                    handle_insert_mode(&mut app, key, &user_cmd_tx)
                                }
                                InputMode::Search => handle_search_mode(&mut app, key),
                                InputMode::Palette => handle_palette_mode(&mut app, key),
                                InputMode::Select => handle_select_mode(&mut app, key),
                                InputMode::FilePicker => {
                                    handle_file_picker_mode(&mut app, key)
                                }
                            }
                        }
                    }
                    Event::Mouse(mouse) => {
                        let in_log = mouse.column >= app.mouse.log_area.x
                            && mouse.column < app.mouse.log_area.x + app.mouse.log_area.width
                            && mouse.row >= app.mouse.log_area.y
                            && mouse.row < app.mouse.log_area.y + app.mouse.log_area.height;
                        let in_plan = mouse.column >= app.mouse.plan_area.x
                            && mouse.column < app.mouse.plan_area.x + app.mouse.plan_area.width
                            && mouse.row >= app.mouse.plan_area.y
                            && mouse.row < app.mouse.plan_area.y + app.mouse.plan_area.height;
                        let in_divider = mouse.column >= app.mouse.divider_area.x
                            && mouse.column < app.mouse.divider_area.x + app.mouse.divider_area.width
                            && mouse.row >= app.mouse.divider_area.y
                            && mouse.row < app.mouse.divider_area.y + app.mouse.divider_area.height;
                        match mouse.kind {
                            MouseEventKind::ScrollUp => {
                                if app.thinking.popup.is_some() {
                                    app.thinking_popup_scroll_up();
                                } else if app.tools.popup.is_some() {
                                    app.diff_popup_scroll_up();
                                } else if app.code_popup.is_some() {
                                    app.code_popup_scroll_up();
                                } else if in_log && app.log_scroll.offset > 0 {
                                    app.log_scroll.offset -= 1;
                                } else if in_plan && app.plan.selected > 0 {
                                    app.plan.selected -= 1;
                                    app.plan.list_state.select(Some(app.plan.selected));
                                }
                            }
                            MouseEventKind::ScrollDown => {
                                if app.thinking.popup.is_some() {
                                    app.thinking_popup_scroll_down();
                                } else if app.tools.popup.is_some() {
                                    app.diff_popup_scroll_down();
                                } else if app.code_popup.is_some() {
                                    app.code_popup_scroll_down();
                                } else if in_log {
                                    app.log_scroll.offset = app.log_scroll.offset.saturating_add(1);
                                } else if in_plan
                                    && !app.plan.steps.is_empty()
                                    && app.plan.selected + 1 < app.plan.steps.len()
                                {
                                    app.plan.selected += 1;
                                    app.plan.list_state.select(Some(app.plan.selected));
                                }
                            }
                            MouseEventKind::Down(MouseButton::Left) => {
                                if app.thinking.popup.is_some() {
                                    let pa = app.mouse.thinking_popup_area;
                                    let in_popup = mouse.column >= pa.x
                                        && mouse.column < pa.x + pa.width
                                        && mouse.row >= pa.y
                                        && mouse.row < pa.y + pa.height;
                                    if !in_popup {
                                        app.close_thinking_popup();
                                    }
                                } else if app.tools.popup.is_some() {
                                    let pa = app.mouse.diff_popup_area;
                                    let in_popup = mouse.column >= pa.x
                                        && mouse.column < pa.x + pa.width
                                        && mouse.row >= pa.y
                                        && mouse.row < pa.y + pa.height;
                                    if !in_popup {
                                        app.close_diff_popup();
                                    }
                                } else if app.code_popup.is_some() {
                                    let pa = app.mouse.code_popup_area;
                                    let in_popup = mouse.column >= pa.x
                                        && mouse.column < pa.x + pa.width
                                        && mouse.row >= pa.y
                                        && mouse.row < pa.y + pa.height;
                                    if !in_popup {
                                        app.close_code_popup();
                                    }
                                } else if in_divider {
                                    // Start panel drag resize
                                    app.mouse.is_resizing_panel = true;
                                } else if in_log {
                                    app.focused_panel = FocusedPanel::Log;
                                    // Mouse row → visual line → logical line, accounting for wrapping
                                    let visual_base = app
                                        .log_scroll
                                        .visual_start
                                        .get(app.log_scroll.offset as usize)
                                        .copied()
                                        .unwrap_or(0);
                                    let visual_row = visual_base
                                        + mouse.row.saturating_sub(app.mouse.log_area.y + 1)
                                            as usize;
                                    let line_idx = app.logical_from_visual(visual_row);
                                    // Compute the column position within the text for word selection
                                    let col = mouse
                                        .column
                                        .saturating_sub(app.mouse.log_area.x + 1)
                                        as usize;
                                    // Track consecutive click count
                                    let now = std::time::Instant::now();
                                    let pos = (mouse.column, mouse.row);
                                    let is_same_click = app.mouse.last_click_pos == Some(pos)
                                        && app.mouse
                                            .last_click_time
                                            .map_or(false, |t| {
                                                now.duration_since(t).as_millis() < 500
                                            });
                                    if is_same_click {
                                        app.mouse.click_count = (app.mouse.click_count + 1).min(3);
                                    } else {
                                        app.mouse.click_count = 1;
                                    }
                                    app.mouse.last_click_time = Some(now);
                                    app.mouse.last_click_pos = Some(pos);
                                    if let Some(phys_idx) = app.visible_message_index(line_idx) {
                                        // Check if the click lands within a collapsed thinking card area
                                        let card_hit = app.thinking.blocks.iter().position(|b| {
                                            app.phys_to_logical_fast(b.title_idx)
                                                .zip(app.phys_to_logical_fast(b.end_idx + 1))
                                                .map_or(false, |(tl, bl)| {
                                                    line_idx >= tl && line_idx < bl
                                                })
                                        });
                                        if let Some(card_idx) = card_hit {
                                            if app.mouse.click_count == 1 {
                                                app.mouse.last_click_card = Some(card_idx);
                                                // Single click: remember the card for double-click open, don't select text
                                                app.mouse.log_word_selection = None;
                                                app.mouse.log_selection = None;
                                                app.mouse.dragging_log = false;
                                            } else if app.mouse.click_count == 2
                                                && app.mouse.last_click_card == Some(card_idx)
                                            {
                                                // Double click: open the thinking popup
                                                let block = &app.thinking.blocks[card_idx];
                                                app.open_thinking_popup(block.title_idx);
                                            } else if app.mouse.click_count >= 3 {
                                                // Triple click: select entire line
                                                app.mouse.log_word_selection = None;
                                                app.mouse.log_selection = Some((line_idx, line_idx));
                                                app.mouse.dragging_log = true;
                                            }
                                        } else {
                                            app.mouse.last_click_card = None;
                                        }
                                        // Check if clicked on a tool block (file write preview) → double-click opens popup
                                        if let Some((tool_idx, phys_idx, _, _)) =
                                            app.find_tool_at_logical(line_idx)
                                        {
                                            if app.mouse.click_count == 1 {
                                                app.mouse.last_click_tool = Some(tool_idx);
                                                app.mouse.log_word_selection = None;
                                                app.mouse.log_selection = None;
                                                app.mouse.dragging_log = false;
                                            } else if app.mouse.click_count == 2
                                                && app.mouse.last_click_tool == Some(tool_idx)
                                            {
                                                app.open_diff_popup(phys_idx);
                                            } else if app.mouse.click_count >= 3 {
                                                app.mouse.log_word_selection = None;
                                                app.mouse.log_selection = Some((line_idx, line_idx));
                                                app.mouse.dragging_log = true;
                                            }
                                        } else {
                                            app.mouse.last_click_tool = None;
                                            // Check if clicked on a code block → double-click opens popup
                                            let code_hit = app
                                                .code_blocks
                                                .iter()
                                                .enumerate()
                                                .find(|(_, b)| {
                                                    app.phys_to_logical_fast(b.start_idx)
                                                        .map_or(false, |si| line_idx >= si)
                                                        && app.phys_to_logical_fast(b.end_idx)
                                                            .map_or(false, |ei| line_idx < ei)
                                                });
                                            if let Some((code_idx, _block)) = code_hit {
                                                if app.mouse.click_count == 1 {
                                                    app.mouse.last_click_code = Some(code_idx);
                                                    app.mouse.log_word_selection = None;
                                                    app.mouse.log_selection = None;
                                                    app.mouse.dragging_log = false;
                                                } else if app.mouse.click_count == 2
                                                    && app.mouse.last_click_code == Some(code_idx)
                                                {
                                                    app.open_code_popup(code_idx);
                                                } else if app.mouse.click_count >= 3 {
                                                    app.mouse.log_word_selection = None;
                                                    app.mouse.log_selection = Some((line_idx, line_idx));
                                                    app.mouse.dragging_log = true;
                                                }
                                            } else {
                                                app.mouse.last_click_code = None;
                                                // Check if clicked on a thinking title line → open popup
                                                let thinking_title = app
                                                    .thinking
                                                    .blocks
                                                    .iter()
                                                    .any(|b| b.title_idx == phys_idx);
                                                if thinking_title {
                                                    app.open_thinking_popup(phys_idx);
                                                } else if app.mouse.click_count == 2 {
                                                    // Double click: select word
                                                    app.mouse.log_selection = Some((line_idx, line_idx));
                                                    app.mouse.log_word_selection =
                                                        app.find_word_bounds(line_idx, col);
                                                    app.mouse.dragging_log = true;
                                                } else if app.mouse.click_count >= 3 {
                                                    // Triple click: select entire line; inside a code block, select the whole block
                                                    app.mouse.log_word_selection = None;
                                                    if let Some((cb_start, cb_end)) =
                                                        app.find_code_block_containing_logical(line_idx)
                                                    {
                                                        app.mouse.log_selection = Some((cb_start, cb_end));
                                                    } else {
                                                        app.mouse.log_selection = Some((line_idx, line_idx));
                                                    }
                                                    app.mouse.dragging_log = true;
                                                } else {
                                                    // Single click: start selection for natural press-drag-copy behaviour
                                                    app.mouse.log_word_selection = None;
                                                    app.mouse.log_selection = Some((line_idx, line_idx));
                                                    app.mouse.dragging_log = true;
                                                }
                                            }
                                        }
                                    }
                                } else if in_plan {
                                    app.focused_panel = FocusedPanel::Plan;
                                    let item_idx =
                                        (mouse.row.saturating_sub(app.mouse.plan_area.y + 1))
                                            as usize;
                                    if item_idx < app.plan.steps.len() {
                                        app.plan.selected = item_idx;
                                        app.plan.list_state.select(Some(app.plan.selected));
                                        app.mouse.plan_selection = Some((item_idx, item_idx));
                                        app.mouse.dragging_plan = true;
                                    }
                                }
                            }
                            MouseEventKind::Drag(MouseButton::Left) => {
                                if app.mouse.is_resizing_panel {
                                    // Panel drag resize — convert mouse column to ratio
                                    let total_width = app.mouse.plan_area.width
                                        + app.mouse.divider_area.width
                                        + app.mouse.log_area.width;
                                    if total_width > 0 {
                                        let mouse_x = mouse.column.saturating_sub(app.mouse.plan_area.x);
                                        let new_ratio = mouse_x as f64 / total_width as f64;
                                        app.panel_split_ratio = new_ratio.clamp(0.10, 0.70);
                                    }
                                } else if app.mouse.dragging_log && in_log {
                                    let visual_base = app
                                        .log_scroll
                                        .visual_start
                                        .get(app.log_scroll.offset as usize)
                                        .copied()
                                        .unwrap_or(0);
                                    let visual_row = visual_base
                                        + mouse.row.saturating_sub(app.mouse.log_area.y + 1)
                                            as usize;
                                    let line_idx = app.logical_from_visual(visual_row);
                                    if line_idx < app.total_log_lines() {
                                        if let Some((start, _)) = app.mouse.log_selection {
                                            app.mouse.log_selection = Some((start, line_idx));
                                        }
                                    }
                                } else if app.mouse.dragging_plan && in_plan {
                                    let item_idx =
                                        (mouse.row.saturating_sub(app.mouse.plan_area.y + 1))
                                            as usize;
                                    if item_idx < app.plan.steps.len() {
                                        if let Some((start, _)) = app.mouse.plan_selection {
                                            app.mouse.plan_selection = Some((start, item_idx));
                                        }
                                    }
                                }
                            }
                            MouseEventKind::Up(MouseButton::Left) => {
                                app.mouse.dragging_log = false;
                                app.mouse.dragging_plan = false;
                                app.mouse.is_resizing_panel = false;
                            }
                            _ => {}
                        }
                    }
                    Event::Paste(data) => {
                        // Paste text (including newlines) into the current input buffer
                        match app.input_mode {
                            InputMode::Insert => {
                                // Multi-line edit: preserve newlines, insert at cursor position
                                let cursor =
                                    app.input.floor_char_boundary(app.input_cursor.min(app.input.len()));
                                let before = app.input[..cursor].to_string();
                                let after = app.input[cursor..].to_string();
                                app.input = before.clone() + &data + &after;
                                app.input_cursor = before.len() + data.len();
                            }
                            InputMode::Search | InputMode::Palette => {
                                // Single-line command: replace newlines with spaces, append to end
                                let text: String = data.replace(['\n', '\r'], " ");
                                app.cmd_line.push_str(&text);
                            }
                            _ => {}
                        }
                    }
                    Event::Resize(_, _new_height) => {}
                    _ => {}
                }
                    }
                    None => break, // event channel closed
                }
            }
            _ = tokio::time::sleep(Duration::from_millis(idle_ms)) => {
                // Wake from idle timeout: force repaint to advance spinner animation
                if !matches!(app.status, Status::Idle) {
                    app.dirty = true;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }

    // Restore terminal state before exiting
    let exit_msg = app.msgs().exit_bye.to_string();
    drop(app);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        PopKeyboardEnhancementFlags,
        DisableBracketedPaste,
    )?;
    terminal.show_cursor()?;
    println!("{}", exit_msg);
    Ok(())
}
