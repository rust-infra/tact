// TUI main module
// Initializes the terminal (raw mode + alternate screen), drives the render loop,
// and dispatches input events.
// Bridges Agent status updates and terminal events to the App state and
// submodule render/handler functions.

mod handlers;
mod i18n;
mod render;
pub(crate) mod system_prompt;
mod theme;
mod theme_detection;

mod widgets;

#[cfg(feature = "test-support")]
pub mod test_support;

#[cfg(feature = "test-support")]
mod headless_loop;

use std::{io, path::PathBuf, time::Duration};

use anyhow::Result;
use crossterm::{
    event::{
        DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, EventStream, KeyCode, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
        PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    widgets::ScrollbarState,
};
use tact::plugin::{PluginEvent, PluginRequest};
use tact_protocol::{AccountUpdate, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio_stream::StreamExt;

pub use crate::widgets::state::SkillEntry;
use crate::{
    handlers::{
        handle_file_picker_mode, handle_insert_mode, handle_mouse_event, handle_normal_mode,
        handle_overlay_key, handle_palette_mode, handle_select_mode,
    },
    render::{
        render_bottom_bar, render_command_palette, render_file_picker, render_input_box,
        render_main_area, render_select_popup, render_slash_command_popup, render_status_bar,
    },
    widgets::state::{App, InputMode, Status},
};

// ========== Main Loop ==========

/// Whether the main loop should repaint this frame (mirrors `run_tui` gate).
pub(crate) fn should_repaint(app: &App) -> bool {
    app.dirty || matches!(app.status, Status::Done) || !app.tools.active.is_empty()
}

/// Configuration for launching the TUI.
pub struct TuiConfig {
    pub agent_rx: UnboundedReceiver<AgentUpdate>,
    pub account_rx: Option<UnboundedReceiver<AccountUpdate>>,
    pub plugin_rx: UnboundedReceiver<PluginEvent>,
    pub plugin_tx: UnboundedSender<PluginRequest>,
    pub user_cmd_tx: UnboundedSender<UserCommand>,
    pub work_dir: PathBuf,
    pub input_history_entries: Vec<String>,
    pub session_id: String,
    pub history_save_tx: UnboundedSender<(String, String)>,
    pub theme: String,
    pub model_context_window: usize,
    /// Configured model name, shown in the bottom bar before the first LLM call.
    pub model_name: String,
    /// Configured max_tokens per LLM call (0 = hide).
    pub model_max_tokens: u32,
    /// Configured thinking budget in tokens (0 = hide).
    pub model_thinking_budget: usize,
    pub skills_description: String,
    pub skills_data: Vec<SkillEntry>,
    /// Shared session store used to inspect persisted request payloads.
    pub session_store: tact::store::DynSessionStore,
    pub skill_registry: tact::skill::SharedSkillRegistry,
}

/// TUI entry point: initializes the terminal, starts the event loop, runs until the user exits.
pub async fn run_tui(cfg: TuiConfig) -> Result<()> {
    let TuiConfig {
        agent_rx,
        account_rx,
        plugin_rx,
        plugin_tx,
        user_cmd_tx,
        work_dir,
        input_history_entries,
        session_id,
        history_save_tx,
        theme,
        model_context_window,
        model_name,
        model_max_tokens,
        model_thinking_budget,
        skills_description,
        skills_data,
        skill_registry,
        session_store,
    } = cfg;
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
        account_rx,
        plugin_rx,
        plugin_tx,
        user_cmd_tx.clone(),
        work_dir,
        input_history_entries,
        session_id,
        history_save_tx,
        theme,
        skills_description,
        skills_data,
    );
    app.skill_registry = skill_registry;
    app.session_store = Some(session_store);
    app.model_context_window = model_context_window;
    // Seed the bottom bar from config so model/token info renders at startup;
    // the first ModelInfo/TokenUsage updates will overwrite these.
    app.status_bar.model_name = model_name;
    app.status_bar.model_max_tokens = model_max_tokens;
    if model_thinking_budget > 0 {
        app.status_bar.model_thinking_budget = Some(model_thinking_budget as u32);
    }
    app.status_bar.model_reasoning_effort =
        tact_llm::current_reasoning_effort_from_budget(model_thinking_budget).map(str::to_string);
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
        let account_updates: Vec<AccountUpdate> = app
            .account_rx
            .as_mut()
            .map(|rx| std::iter::from_fn(|| rx.try_recv().ok()).collect())
            .unwrap_or_default();
        for update in account_updates {
            app.handle_account_update(update);
        }
        while let Ok(event) = app.plugin_rx.try_recv() {
            app.handle_plugin_event(event);
        }

        // Only repaint when the dirty flag is true or in Done state, avoiding pointless
        // high-frequency refreshes while idle.
        // Done state transitions to Idle after 2s timeout; must keep rendering to check
        // the clock.
        if should_repaint(&app) {
            // Advance spinner frame when in an active state
            if !matches!(app.status, Status::Idle | Status::Done) {
                app.spinner_frame = (app.spinner_frame + 1) % 10;
            }
            terminal.draw(|f| {
                let size = f.area();
                // Input box height auto-expands with content (1–3 lines of content + 2 for border)
                let input_lines = app.input.lines().count().clamp(1, 3) as u16;
                let input_height = input_lines + 2;
                let bottom_height = 2u16;
                let log_area = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(1),
                        Constraint::Min(1),
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
                        Constraint::Min(1),
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
        app.maybe_expire_done_status();

        app.maybe_clear_flash_msg();

        // Adaptive idle polling interval: adjust the event wait timeout based on state.
        // - Done state: 200ms, frequently check the 2s → Idle transition
        // - Dirty flag set: 10ms, quickly trigger a rerender
        // - Active (Planning/Executing): 150ms to animate spinner
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
                        } else if handle_overlay_key(&mut app, key) {
                            // Overlay popup consumed the key.
                        } else if key.code == KeyCode::Tab {
                            app.focused_panel = match app.focused_panel {
                                crate::widgets::state::FocusedPanel::Log => crate::widgets::state::FocusedPanel::Plan,
                                crate::widgets::state::FocusedPanel::Plan => crate::widgets::state::FocusedPanel::Log,
                            };
                        } else if (app.show_help || app.show_history) && key.code == KeyCode::Esc {
                            app.show_help = false;
                            app.show_history = false;
                        } else {
                            // Dispatch to the key handler for the current input mode
                            match app.input_mode {
                                InputMode::Normal => {
                                    handle_normal_mode(&mut app, key, &user_cmd_tx)
                                }
                                InputMode::Insert => {
                                    handle_insert_mode(&mut app, key, &user_cmd_tx)
                                }
                                InputMode::Palette => handle_palette_mode(&mut app, key),
                                InputMode::Select => handle_select_mode(&mut app, key),
                                InputMode::FilePicker => {
                                    handle_file_picker_mode(&mut app, key)
                                }
                            }
                        }
                    }
                    Event::Mouse(mouse) => {
                        handle_mouse_event(&mut app, mouse);
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
                            InputMode::Palette => {
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
