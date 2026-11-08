//! Headless `App` helpers: build an app without a terminal and auto-confirm selects.

use tact_protocol::{UiResponse, UpdateReceiver};

use crate::widgets::state::{App, InputMode, SelectKind};

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

/// Build an `App` wired to the given agent channel (no startup logo/messages).
pub fn make_headless_app(agent_rx: impl Into<UpdateReceiver>, work_dir: std::path::PathBuf) -> App {
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
