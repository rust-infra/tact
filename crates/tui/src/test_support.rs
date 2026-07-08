//! Public test helpers for cross-crate integration (e.g. tact-ui → App bridge).

use crate::render::test_harness::{make_app, render_app_text, render_main_area_text};
use crate::widgets::state::{App, Status};
use std::path::PathBuf;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::unbounded_channel;

/// Thin wrapper around [`App`] for integration tests outside the `tui` crate.
pub struct TestApp(App);

impl TestApp {
    pub fn new() -> Self {
        Self(make_app())
    }

    pub fn new_in_dir(work_dir: PathBuf) -> Self {
        let (_agent_tx, agent_rx) = unbounded_channel();
        let (user_cmd_tx, _user_cmd_rx) = unbounded_channel();
        let (history_tx, _history_rx) = unbounded_channel();
        Self(App::new(
            agent_rx,
            user_cmd_tx,
            work_dir,
            Vec::new(),
            "bridge-test".into(),
            history_tx,
            "retro".into(),
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
}
