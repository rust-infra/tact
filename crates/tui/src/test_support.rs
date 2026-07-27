//! Public test helpers for cross-crate integration (e.g. tact-ui → App bridge).

use std::{path::PathBuf, time::Duration};

use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{
    headless_loop::{auto_confirm_select, drain_agent_updates, make_headless_app},
    render::test_harness::{make_app, render_app_text, render_main_area_text},
    widgets::state::{App, InputMode, Status},
};

/// Thin wrapper around [`App`] for integration tests outside the `tui` crate.
pub struct TestApp(App);

impl Default for TestApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TestApp {
    pub fn new() -> Self {
        Self(make_app())
    }

    pub fn new_in_dir(work_dir: PathBuf) -> Self {
        let (_agent_tx, agent_rx) = unbounded_channel();
        let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
        let (plugin_tx, _plugin_request_rx) = unbounded_channel();
        let (_plugin_event_tx, plugin_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        Self(App::new(
            agent_rx,
            None,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            work_dir,
            Vec::new(),
            "bridge-test".into(),
            history_tx,
            "retro".into(),
            String::new(),
            Vec::new(),
        ))
    }

    pub fn feed(&mut self, update: AgentUpdate) {
        self.0.handle_agent_update(update);
    }

    pub fn feed_all(&mut self, updates: impl IntoIterator<Item = AgentUpdate>) {
        for update in updates {
            self.feed(update);
        }
    }

    pub fn render(&mut self, width: u16, height: u16) -> String {
        render_app_text(&mut self.0, width, height)
    }

    pub fn render_main(&mut self, width: u16, height: u16) -> String {
        render_main_area_text(&mut self.0, width, height)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.0.status, Status::Done)
    }

    pub fn open_last_tool_popup(&mut self) -> bool {
        let Some(idx) = self.0.tools.blocks.last().map(|b| b.phys_idx) else {
            return false;
        };
        self.0.open_diff_popup(idx);
        self.0.tools.popup.is_some()
    }

    pub fn has_diff_popup(&self) -> bool {
        self.0.tools.popup.is_some()
    }

    pub fn diff_popup_content(&self) -> Option<String> {
        self.0.tools.popup.as_ref().and_then(|p| {
            p.cached_content
                .clone()
                .or_else(|| {
                    p.file_path
                        .as_ref()
                        .and_then(|path| std::fs::read_to_string(path).ok())
                })
                .or_else(|| p.inline_content.clone())
        })
    }

    pub fn close_diff_popup(&mut self) {
        self.0.close_diff_popup();
    }

    pub fn tool_block_count(&self) -> usize {
        self.0.tools.blocks.len()
    }

    pub fn is_help_visible(&self) -> bool {
        self.0.show_help
    }

    pub fn is_history_visible(&self) -> bool {
        self.0.show_history
    }

    pub fn toggle_help(&mut self) {
        self.0.show_help = !self.0.show_help;
    }

    pub fn toggle_history(&mut self) {
        self.0.show_history = !self.0.show_history;
    }

    pub fn is_select_mode(&self) -> bool {
        matches!(self.0.input_mode, InputMode::Select)
    }

    pub fn select_popup_options(&self) -> Vec<String> {
        self.0.select.options.clone()
    }

    pub fn open_code_popup(&mut self, idx: usize) -> bool {
        if idx < self.0.code_blocks.len() {
            self.0.open_code_popup(idx);
            true
        } else {
            false
        }
    }

    pub fn is_code_popup_open(&self) -> bool {
        self.0.code_popup.is_some()
    }

    pub fn close_code_popup(&mut self) {
        self.0.close_code_popup();
    }

    pub fn open_thinking_popup(&mut self, phys_idx: usize) -> bool {
        self.0.open_thinking_popup(phys_idx);
        self.0.thinking.popup.is_some()
    }

    pub fn is_thinking_popup_open(&self) -> bool {
        self.0.thinking.popup.is_some()
    }

    pub fn close_thinking_popup(&mut self) {
        self.0.close_thinking_popup();
    }
}

/// Headless App wired to a live agent channel (mirrors `run_tui` update drain).
pub struct HeadlessApp {
    inner: App,
    auto_select: Option<usize>,
    capture_frames: bool,
}

impl HeadlessApp {
    pub fn new(agent_rx: UnboundedReceiver<AgentUpdate>, work_dir: PathBuf) -> Self {
        Self {
            inner: make_headless_app(agent_rx, work_dir),
            auto_select: None,
            capture_frames: false,
        }
    }

    pub fn with_auto_select(mut self, choice: Option<usize>) -> Self {
        self.auto_select = choice;
        self
    }

    /// Capture every rendered frame while `run_while` is active.
    pub fn with_frame_capture(mut self) -> Self {
        self.capture_frames = true;
        self
    }

    pub fn poll(&mut self) {
        drain_agent_updates(&mut self.inner, self.auto_select);
    }

    pub fn poll_without_auto_confirm(&mut self) {
        while let Ok(update) = self.inner.agent_rx.try_recv() {
            self.inner.handle_agent_update(update);
        }
    }

    pub fn confirm_select(&mut self, choice: usize) {
        auto_confirm_select(&mut self.inner, choice);
    }

    pub fn render(&mut self, width: u16, height: u16) -> String {
        render_app_text(&mut self.inner, width, height)
    }

    pub fn is_done(&self) -> bool {
        matches!(self.inner.status, Status::Done)
    }

    pub fn is_executing(&self) -> bool {
        matches!(self.inner.status, Status::Executing { .. }) || !self.inner.tools.active.is_empty()
    }

    pub fn is_select_mode(&self) -> bool {
        matches!(self.inner.input_mode, InputMode::Select)
    }

    pub fn is_help_visible(&self) -> bool {
        self.inner.show_help
    }

    pub fn is_history_visible(&self) -> bool {
        self.inner.show_history
    }

    pub fn tool_block_count(&self) -> usize {
        self.inner.tools.blocks.len()
    }

    pub fn has_diff_popup(&self) -> bool {
        self.inner.tools.popup.is_some()
    }

    pub fn user_cmd_tx(&self) -> UnboundedSender<tact_protocol::UserCommand> {
        self.inner.user_cmd_tx.clone()
    }

    /// Poll while `is_running` returns true or until `timeout`.
    pub async fn run_while<F>(&mut self, mut is_running: F, timeout: Duration) -> HeadlessSnapshots
    where
        F: FnMut() -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        let mut snapshots = HeadlessSnapshots::default();

        while is_running() && tokio::time::Instant::now() < deadline {
            self.poll_without_auto_confirm();
            if snapshots.executing.is_none() && self.is_executing() {
                snapshots.executing = Some(self.render(120, 30));
            }
            if snapshots.select.is_none() && self.is_select_mode() {
                snapshots.select = Some(self.render(120, 30));
                if let Some(choice) = self.auto_select {
                    self.confirm_select(choice);
                }
            }
            self.poll();
            if self.capture_frames {
                snapshots.frames.push(self.render(120, 30));
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        self.poll();
        snapshots.final_render = Some(self.render(120, 30));

        if self.capture_frames {
            if let Some(ref frame) = snapshots.executing {
                snapshots.frames.push(frame.clone());
            }
            if let Some(ref frame) = snapshots.select {
                snapshots.frames.push(frame.clone());
            }
        }

        snapshots
    }
}

#[derive(Default)]
pub struct HeadlessSnapshots {
    pub executing: Option<String>,
    pub select: Option<String>,
    pub final_render: Option<String>,
    /// Every frame rendered during `run_while` when frame capture is enabled.
    pub frames: Vec<String>,
}
