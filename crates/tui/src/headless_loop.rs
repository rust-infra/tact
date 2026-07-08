//! Headless agent-update loop (mirrors `run_tui` drain logic, no terminal).

use crate::widgets::state::{App, InputMode};
use std::time::Duration;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedReceiver;

/// Drain pending updates from `agent_rx`, optionally auto-confirm permission selects.
pub fn drain_agent_updates(app: &mut App, auto_select: Option<usize>) {
    while let Ok(update) = app.agent_rx.try_recv() {
        app.handle_agent_update(update);
        if matches!(app.input_mode, InputMode::Select) {
            if let Some(choice) = auto_select {
                auto_confirm_select(app, choice);
            }
        }
    }
}

/// Confirm the current select popup programmatically (headless substitute for Enter).
pub fn auto_confirm_select(app: &mut App, choice: usize) {
    if !matches!(app.input_mode, InputMode::Select) || app.select.options.is_empty() {
        return;
    }
    app.select.selected = choice.min(app.select.options.len().saturating_sub(1));
    app.select.confirm();
    app.input_mode = InputMode::Normal;
}

/// Poll until `should_continue` returns false, draining updates each tick.
pub async fn run_until<F>(mut app: App, mut should_continue: F, auto_select: Option<usize>) -> App
where
    F: FnMut(&App) -> bool,
{
    while should_continue(&app) {
        drain_agent_updates(&mut app, auto_select);
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    drain_agent_updates(&mut app, auto_select);
    app
}

/// Build an `App` wired to the given agent channel (no startup logo/messages).
pub fn make_headless_app(
    agent_rx: UnboundedReceiver<AgentUpdate>,
    work_dir: std::path::PathBuf,
) -> App {
    use tokio::sync::mpsc::unbounded_channel;
    let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
    let (history_tx, _history_rx) = unbounded_channel();
    App::new(
        agent_rx,
        user_cmd_tx,
        work_dir,
        Vec::new(),
        "headless-session".into(),
        history_tx,
        "retro".into(),
    )
}
