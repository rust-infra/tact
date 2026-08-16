//! Codex-style queued messages ("submit after next tool call").
//!
//! While the agent is busy (`Status::Planning` / `Status::Executing`), Enter
//! no longer flashes a "still processing" message: the typed text is queued
//! here, shown above the input box with a hint, and auto-submitted once the
//! current task finishes. Esc interrupts the running task and submits the
//! queue immediately (see `handlers::skills::interrupt_and_submit_pending`).

use crate::widgets::state::App;

/// A user message queued while the agent was busy.
#[derive(Debug, Clone)]
pub(crate) struct PendingMessage {
    /// Text shown in the pending block and later in the user bubble.
    pub display: String,
    /// Text dispatched to the agent as the `SubmitTask` payload.
    pub agent_task: String,
}

impl App {
    /// Append a message typed while the agent is busy.
    pub(crate) fn queue_pending_message(&mut self, display: String, agent_task: String) {
        self.pending_messages.push(PendingMessage {
            display,
            agent_task,
        });
        self.dirty = true;
    }

    /// Drop all queued messages (`/cancel` / Normal-mode `c`): cancel means
    /// "cancel everything", unlike Esc which submits the queue immediately.
    pub(crate) fn clear_pending_messages(&mut self) {
        if !self.pending_messages.is_empty() {
            self.pending_messages.clear();
            self.dirty = true;
        }
    }

    /// Record the `[Cancel]` button hit area (render-time; drops the queue
    /// without touching the running task, see the mouse handler).
    pub(crate) fn set_cancel_button_area(&mut self, area: ratatui::layout::Rect) {
        self.pending_cancel_btn_area = area;
    }

    /// Display rows the pending block needs: one hint line plus one row per
    /// queued message, capped so the log area is never starved.
    pub(crate) fn pending_display_lines(&self) -> u16 {
        if self.pending_messages.is_empty() {
            return 0;
        }
        (1 + self.pending_messages.len()).min(4) as u16
    }
}
