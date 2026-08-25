//! Headless agent-update loop (mirrors `run_tui` drain logic, no terminal).

use std::time::Duration;

use tact_protocol::{AgentUpdate, UiResponse, UserCommand};
use tokio::sync::mpsc::UnboundedReceiver;

use crate::widgets::state::{App, InputMode, SelectKind};

/// Drain pending updates from `agent_rx`, optionally auto-confirm permission selects.
pub fn drain_agent_updates(app: &mut App, auto_select: Option<usize>) {
    while let Ok(update) = app.agent_rx.try_recv() {
        app.handle_agent_update(update);
        if matches!(app.input_mode, InputMode::Select)
            && let Some(choice) = auto_select
        {
            auto_confirm_select(app, choice);
        }
    }
}

/// Build the response for auto-confirming the current select popup.
///
/// Consumes the popup's request id and leaves `InputMode::Normal`. Returns the
/// [`UiResponse`] to deliver, or `None` when the popup is a local flow (no
/// request id) or not actually open.
pub fn build_auto_confirm_response(app: &mut App, choice: usize) -> Option<UiResponse> {
    if !matches!(app.input_mode, InputMode::Select) || app.select.options.is_empty() {
        return None;
    }
    let idx = choice.min(app.select.options.len().saturating_sub(1));
    app.select.selected = idx;
    let request_id = app.select.take_request_id();
    let response = if app.select.multi {
        if let Some(slot) = app.select.checked.get_mut(idx) {
            *slot = true;
        }
        let idxs = app.select.confirm_multi();
        request_id.map(|id| UiResponse::MultiSelect {
            request_id: id,
            choices: Some(idxs),
        })
    } else {
        let _ = app.select.confirm();
        request_id.map(|id| UiResponse::Select {
            request_id: id,
            choice: Some(idx),
        })
    };
    app.select_kind = SelectKind::Agent;
    app.input_mode = InputMode::Normal;
    response
}

/// Confirm the current select popup programmatically, sending the response on
/// the App's command channel (headless substitute for Enter).
pub fn auto_confirm_select(app: &mut App, choice: usize) {
    if let Some(response) = build_auto_confirm_response(app, choice) {
        let _ = app.user_cmd_tx.send(UserCommand::UiResponse(response));
    }
}

/// Poll until `should_continue` returns false, draining updates each tick.
#[allow(dead_code)]
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
    let (plugin_tx, _plugin_request_rx) = unbounded_channel();
    let (_plugin_event_tx, plugin_rx) = unbounded_channel();
    let (history_tx, _history_rx) = unbounded_channel();
    App::new(
        agent_rx,
        None,
        plugin_rx,
        plugin_tx,
        user_cmd_tx,
        work_dir,
        Vec::new(),
        "headless-session".into(),
        history_tx,
        "retro".into(),
        String::new(),
        Vec::new(),
    )
}
