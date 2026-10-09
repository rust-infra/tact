//! Building a test `App`.
//!
//! Six modules under `handlers/` each grew their own copy of this: five
//! `unbounded_channel`s, then a twelve-argument `App::new`. They differed only
//! in which receiver the test wanted back, so [`TestApp`] hands back both and
//! the caller splits off the one it asserts on.
//!
//! `render`'s harness builds the same app under the `ink` theme, because its
//! tests assert the colours a theme produces and cannot take the default. That
//! is the only reason there are two constructors.

#![allow(dead_code)]

use std::path::PathBuf;

use tact_extensions::plugin::{PluginEvent, PluginRequest};
use tact_protocol::{RuntimeEvent, StepResult, StepStatus, ToolPresentationInfo};
use tact_view::UserCommand;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use crate::widgets::state::App;

/// An app under test, still holding the channels it talks through.
pub struct TestApp {
    /// The app itself. Public so a render test can take it and drop the rest.
    pub app: App,
    /// What the app asked the agent to do (`/mcp`, `/hooks`, `/skill`, …).
    user_cmds: UnboundedReceiver<UserCommand>,
    /// What the app asked the plugin host to do (`/plugin reload`, …).
    plugin_requests: UnboundedReceiver<PluginRequest>,
}

impl Default for TestApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TestApp {
    /// The `handlers` fixture: session `test-session`, theme `retro`.
    pub fn new() -> Self {
        Self::with_identity("test-session", "retro")
    }

    /// The same app under a named session and theme.
    pub fn with_identity(session_id: &str, theme: &str) -> Self {
        // Both senders are dropped here: the receivers are what the app holds,
        // and a fixture that kept an agent channel alive would let a test hang
        // waiting on a stream nothing can close.
        let (_agent_tx, agent_rx) = unbounded_channel::<RuntimeEvent>();
        let (user_cmd_tx, user_cmds) = unbounded_channel::<UserCommand>();
        let (plugin_tx, plugin_requests) = unbounded_channel::<PluginRequest>();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel::<PluginEvent>();
        let (history_tx, _history_rx) = unbounded_channel::<(String, String)>();
        Self {
            app: App::new(
                agent_rx,
                None,
                plugin_rx,
                plugin_tx,
                user_cmd_tx,
                PathBuf::from("."),
                Vec::new(),
                session_id.to_string(),
                history_tx,
                theme.to_string(),
                String::new(),
                Vec::new(),
            ),
            user_cmds,
            plugin_requests,
        }
    }

    /// Split into the app and the receiver of the commands it emitted.
    pub fn into_commands(self) -> (App, UnboundedReceiver<UserCommand>) {
        (self.app, self.user_cmds)
    }

    /// Split into the app and the receiver of the plugin requests it emitted.
    pub fn into_plugin_requests(self) -> (App, UnboundedReceiver<PluginRequest>) {
        (self.app, self.plugin_requests)
    }
}

/// A tool call, as the host reports it: the `StepStarted` it emits when the
/// call begins and the `StepFinished` when it ends.
///
/// The literals this replaces ran to eight and sixteen lines and appeared 78
/// times across the render and handler tests. The defaults are the ones those
/// literals overwhelmingly used, so a test that does not care about a field
/// does not have to say anything about it — and a test that *does* care sets it
/// explicitly, which is the point: the field it cares about is the one visible
/// in the call.
pub struct StepCall {
    idx: usize,
    tool_id: String,
    tool: String,
    arg_summary: String,
    arg_full: Option<String>,
    status: StepStatus,
    message: String,
    detail: Option<String>,
    duration_us: u64,
    permission_label: Option<String>,
    presentation: Option<ToolPresentationInfo>,
}

impl StepCall {
    /// A call to `tool` identified as `tool_id`, at position `idx` in the turn.
    ///
    /// `arg` is both the summary and the full argument, which is what the host
    /// sends for every tool whose input is one value.
    pub fn new(
        idx: usize,
        tool_id: impl Into<String>,
        tool: impl Into<String>,
        arg: impl Into<String>,
    ) -> Self {
        let arg = arg.into();
        Self {
            idx,
            tool_id: tool_id.into(),
            tool: tool.into(),
            arg_summary: arg.clone(),
            arg_full: Some(arg),
            status: StepStatus::Success,
            message: "ok".to_string(),
            detail: None,
            duration_us: 1,
            permission_label: None,
            presentation: None,
        }
    }

    /// The full argument, when it differs from the summary.
    pub fn arg_full(mut self, arg_full: impl Into<String>) -> Self {
        self.arg_full = Some(arg_full.into());
        self
    }

    /// No full argument — what the host sends when the summary is all there is.
    pub fn no_arg_full(mut self) -> Self {
        self.arg_full = None;
        self
    }

    pub fn status(mut self, status: StepStatus) -> Self {
        self.status = status;
        self
    }

    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = message.into();
        self
    }

    /// The output the card renders, or its absence when a call produced none.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn duration_us(mut self, duration_us: u64) -> Self {
        self.duration_us = duration_us;
        self
    }

    pub fn permission_label(mut self, label: impl Into<String>) -> Self {
        self.permission_label = Some(label.into());
        self
    }

    /// The tool's presentation, when the generic one for `tool` is not what the
    /// test is about (`task_presentation()`, a subagent card, …).
    pub fn presentation(mut self, presentation: ToolPresentationInfo) -> Self {
        self.presentation = Some(presentation);
        self
    }

    pub fn started(&self) -> RuntimeEvent {
        RuntimeEvent::StepStarted {
            run_id: None,
            idx: self.idx,
            tool_id: self.tool_id.clone(),
            tool_name: self.tool.clone(),
            arg_summary: self.arg_summary.clone(),
            arg_full: self.arg_full.clone().unwrap_or_default(),
            presentation: self.presentation_info(),
        }
    }

    pub fn finished(&self) -> RuntimeEvent {
        RuntimeEvent::StepFinished {
            run_id: None,
            idx: self.idx,
            tool_id: self.tool_id.clone(),
            result: StepResult {
                tool: self.tool.clone(),
                arg_summary: self.arg_summary.clone(),
                arg_full: self.arg_full.clone(),
                status: self.status,
                message: self.message.clone(),
                detail: self.detail.clone(),
                duration_us: Some(self.duration_us),
                permission_label: self.permission_label.clone(),
                presentation: self.presentation_info(),
            },
        }
    }

    fn presentation_info(&self) -> ToolPresentationInfo {
        self.presentation
            .clone()
            .unwrap_or_else(|| ToolPresentationInfo::generic(&self.tool))
    }
}
