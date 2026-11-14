//! The trajectory page's app-side state and keys.
//!
//! The ledger and the renderer are pure and live in `agent_tui_kit`; this owns
//! the flag that selects the page, the reader's place in it, and what each key
//! does. Opened and closed through one function each, because a half-closed
//! page is how the subagent view used to badge a surface that was already gone.

use agent_tui_kit::{
    render::trajectory::{TrajectoryPageState, ledger_rows, visible_ledger_rows},
    state::trajectory::TrajectoryLog,
};

use super::super::{App, InputMode, SurfaceId};

impl App {
    /// Open the page, taking the main area and the keyboard.
    pub(crate) fn open_trajectory_page(&mut self) {
        self.show_trajectory = true;
        // One page at a time: these all return early from `render_main_area`,
        // so leaving another set would decide the surface by ordering.
        self.show_help = false;
        self.show_history = false;
        self.show_subagent_view = false;
        self.trajectory_page.filter_editing = false;
        // The page's `j`/`k`/`g`/`G` are dispatched by the Normal-mode handler,
        // and a session starts in Insert where those letters type into the
        // input box instead.
        self.input_mode = InputMode::Normal;
        self.select_latest_trajectory_record();
        self.dirty = true;
    }

    /// Close the page and hand the keyboard back.
    pub(crate) fn close_trajectory_page(&mut self) {
        self.show_trajectory = false;
        self.trajectory_page.filter_editing = false;
        self.input_mode = InputMode::Insert;
        // A closed page must not keep its hit rect: the wheel would route to a
        // surface that is no longer drawn, and `trajectory_content_rows` would
        // size the ledger from the last frame it was open.
        self.mouse.clear_area(SurfaceId::TrajectoryPage);
        self.dirty = true;
    }

    /// Land on the newest record, which is what a reader opening the page mid
    /// run wants to see.
    pub(crate) fn select_latest_trajectory_record(&mut self) {
        let last = self.trajectory.records().last().map(|record| record.id);
        self.trajectory_page.selected = last;
        self.trajectory_page.follow = true;
        self.scroll_trajectory_to_selection();
    }

    /// Move the selection by `delta` records, clamped to the ledger.
    pub(crate) fn move_trajectory_selection(&mut self, delta: isize) {
        // Leaving the tail has to pin the offset the view was actually at. While
        // following, `scroll` is meaningless and the renderer resolves it from
        // the content; without this, the first `k` would jump to the top.
        self.materialize_trajectory_offset();
        let rows = ledger_rows(&self.trajectory, &self.trajectory_page.filter);
        let order: Vec<usize> = rows.iter().filter_map(|row| row.record).collect();
        if order.is_empty() {
            return;
        }
        let current = self
            .trajectory_page
            .selected
            .and_then(|id| self.trajectory.position_of(id));
        let position = current
            .and_then(|index| order.iter().position(|candidate| *candidate == index))
            .unwrap_or(0);
        let next = if delta.is_negative() {
            position.saturating_sub(delta.unsigned_abs())
        } else {
            (position + delta as usize).min(order.len() - 1)
        };
        self.trajectory_page.selected = self.trajectory.get(order[next]).map(|record| record.id);
        self.trajectory_page.follow = false;
        self.scroll_trajectory_to_selection();
        self.dirty = true;
    }

    /// Wheel scrolling over the ledger.
    ///
    /// The wheel moves the same thing `j`/`k` does — the selection, one record
    /// per notch — rather than the raw painted offset. Two reasons: a record is
    /// the unit the reader is scanning (the log's line stepper exists because
    /// prose is read line by line, a ledger is not), and keeping one cursor
    /// means the wheel cannot desynchronize the selection from what is on
    /// screen. `scroll_trajectory_to_selection` already handles the viewport.
    ///
    /// Down at the last record re-arms `follow`: the reader has arrived at the
    /// live tail, which is exactly what `G` means, so the page should resume
    /// tracking the run instead of freezing one row short of the next append.
    pub(crate) fn trajectory_scroll_by(&mut self, delta: isize) {
        if delta == 0 {
            return;
        }
        let rows = ledger_rows(&self.trajectory, &self.trajectory_page.filter);
        let order: Vec<usize> = rows.iter().filter_map(|row| row.record).collect();
        if order.is_empty() {
            return;
        }
        self.materialize_trajectory_offset();
        let position = self
            .trajectory_page
            .selected
            .and_then(|id| self.trajectory.position_of(id))
            .and_then(|index| order.iter().position(|candidate| *candidate == index))
            .unwrap_or(0);

        if delta.is_negative() {
            let next = position.saturating_sub(delta.unsigned_abs());
            self.trajectory_page.selected =
                self.trajectory.get(order[next]).map(|record| record.id);
            self.trajectory_page.follow = false;
        } else {
            let last = order.len() - 1;
            let next = (position + delta as usize).min(last);
            self.trajectory_page.selected =
                self.trajectory.get(order[next]).map(|record| record.id);
            if next == last {
                // Arriving at the tail re-arms following, the same contract as
                // the subagent view's wheel and the log's bottom pill.
                self.trajectory_page.follow = true;
                self.trajectory_page.scroll = 0;
            } else {
                self.trajectory_page.follow = false;
            }
        }
        self.scroll_trajectory_to_selection();
        self.dirty = true;
    }

    pub(crate) fn move_trajectory_selection_to_edge(&mut self, last: bool) {
        self.trajectory_page.follow = last;
        self.trajectory_page.scroll = 0;
        if last {
            self.select_latest_trajectory_record();
        } else {
            let rows = ledger_rows(&self.trajectory, &self.trajectory_page.filter);
            let first = rows.iter().find_map(|row| row.record);
            self.trajectory_page.selected =
                first.and_then(|index| self.trajectory.get(index).map(|record| record.id));
        }
        self.dirty = true;
    }

    /// Resolve a following view's offset into `scroll`, then stop following.
    ///
    /// The renderer owns the follow arithmetic because only it knows the row
    /// count and the viewport. The app cannot recompute that without drifting,
    /// so it asks the same function and stores the answer once.
    pub(crate) fn materialize_trajectory_offset(&mut self) {
        if !self.trajectory_page.follow {
            return;
        }
        let rows = ledger_rows(&self.trajectory, &self.trajectory_page.filter);
        let visible = self.trajectory_visible_rows();
        self.trajectory_page.scroll =
            agent_tui_kit::render::trajectory::follow_offset(&rows, visible);
        self.trajectory_page.follow = false;
    }

    /// Keep the selected record inside the painted window.
    ///
    /// Scroll is in painted rows, not records, because a turn rule costs a row:
    /// a record's painted offset is not its index.
    pub(crate) fn scroll_trajectory_to_selection(&mut self) {
        let rows = ledger_rows(&self.trajectory, &self.trajectory_page.filter);
        let Some(selected) = self.trajectory_page.selected else {
            return;
        };
        let position = rows.iter().position(|row| {
            row.record
                .and_then(|index| self.trajectory.get(index))
                .is_some_and(|record| record.id == selected)
        });
        let Some(position) = position else {
            return;
        };
        let painted = agent_tui_kit::render::trajectory::painted_lines(&rows[..position]);
        let visible = self.trajectory_visible_rows() as usize;
        if visible == 0 {
            return;
        }
        if painted < self.trajectory_page.scroll {
            self.trajectory_page.scroll = painted;
        } else if painted + 1 > self.trajectory_page.scroll + visible {
            self.trajectory_page.scroll = painted + 1 - visible;
        }
    }

    /// How many ledger rows the page currently paints.
    pub(crate) fn trajectory_visible_rows(&self) -> u16 {
        let content = self.trajectory_content_rows();
        visible_ledger_rows(
            content,
            self.trajectory_page.overview_visible(content),
            self.trajectory_page.detail,
        )
    }

    /// The page's content height: what the main area gives it minus the box's
    /// own top and bottom borders.
    pub(crate) fn trajectory_content_rows(&self) -> u16 {
        self.mouse
            .area(SurfaceId::TrajectoryPage)
            .height
            .saturating_sub(2)
            .max(1)
    }

    pub(crate) fn toggle_trajectory_detail(&mut self) {
        self.trajectory_page.detail = !self.trajectory_page.detail;
        self.scroll_trajectory_to_selection();
        self.dirty = true;
    }

    pub(crate) fn cycle_trajectory_tab(&mut self) {
        self.trajectory_page.tab = self.trajectory_page.tab.next();
        self.dirty = true;
    }

    /// Flip the overview. The first `o` resolves the height default into an
    /// explicit choice, so a second press flips it back.
    pub(crate) fn toggle_trajectory_overview(&mut self) {
        let content = self.trajectory_content_rows();
        let current = self.trajectory_page.overview_visible(content);
        self.trajectory_page.overview = Some(!current);
        self.scroll_trajectory_to_selection();
        self.dirty = true;
    }

    pub(crate) fn begin_trajectory_filter(&mut self) {
        self.trajectory_page.filter_editing = true;
        self.trajectory_page.filter.clear();
        self.dirty = true;
    }

    pub(crate) fn push_trajectory_filter(&mut self, c: char) {
        self.trajectory_page.filter.push(c);
        self.trajectory_page.scroll = 0;
        self.dirty = true;
    }

    pub(crate) fn pop_trajectory_filter(&mut self) {
        self.trajectory_page.filter.pop();
        self.trajectory_page.scroll = 0;
        self.dirty = true;
    }

    pub(crate) fn finish_trajectory_filter(&mut self) {
        self.trajectory_page.filter_editing = false;
        self.dirty = true;
    }

    pub(crate) fn clear_trajectory_filter(&mut self) {
        self.trajectory_page.filter.clear();
        self.trajectory_page.filter_editing = false;
        self.trajectory_page.scroll = 0;
        self.dirty = true;
    }

    pub(crate) fn trajectory_page_state(&self) -> &TrajectoryPageState {
        &self.trajectory_page
    }

    pub(crate) fn trajectory_log(&self) -> &TrajectoryLog {
        &self.trajectory
    }
}

impl App {
    /// Record one update in the ledger.
    ///
    /// Called before any handler, because the fields this keeps are the ones
    /// the handlers throw away.
    pub(crate) fn observe_for_trajectory(&mut self, update: &tact_protocol::AgentUpdate) {
        // The assistant record closes when its message does, so a second
        // message in the same turn starts its own row instead of appending to
        // the first.
        if matches!(
            update,
            tact_protocol::AgentUpdate::MessageFinished { .. }
                | tact_protocol::AgentUpdate::ToolTranscriptStream { .. }
        ) {
            self.trajectory.finish_assistant();
        }
        let before = self.trajectory.len();
        self.trajectory.observe(update);
        let appended = self.trajectory.len() != before;
        // A running call that only changed its live tail is not an append, but
        // it is still activity the page must repaint for.
        if self.show_trajectory && (appended || changed_running_record(update)) {
            if self.trajectory_page.follow {
                // Following means "the newest record is what I am reading", so
                // the selection moves with it. A reader who scrolled away keeps
                // both their place and their selection.
                self.trajectory_page.selected =
                    self.trajectory.records().last().map(|record| record.id);
                self.trajectory_page.scroll = 0;
            } else {
                self.scroll_trajectory_to_selection();
            }
        }
    }

    /// Start a turn in the ledger, at the same moment the status bar counts one.
    pub(crate) fn begin_trajectory_turn(&mut self, prompt: &str) {
        self.trajectory.begin_turn();
        self.trajectory
            .push_user(prompt.to_string(), Some(now_label()));
        if self.show_trajectory {
            self.select_latest_trajectory_record();
        }
    }

    /// Restore both views a resumed session needs: the log's cells and the
    /// trajectory ledger.
    ///
    /// One entry point rather than two calls at the startup site, because the
    /// two reads have to stay in step: the log renders the prose and the ledger
    /// the evidence, and restoring only one leaves a page that disagrees with
    /// the transcript. It is also what makes the wiring testable — `run_tui`
    /// itself needs a terminal.
    ///
    /// Two queries, not one merged read: `load_session` yields the legacy
    /// `Message` shape the log's renderer consumes, and `load_agent_session`
    /// the agent shape that still carries each tool result's status. Widening
    /// the log's input to carry a fact it never reads would touch every branch
    /// of that renderer for nothing.
    pub(crate) async fn restore_session_views(&mut self) {
        let Some(store) = self.session_store.clone() else {
            return;
        };
        if let Ok(messages) = store.load_session(&self.session_id).await
            && !messages.is_empty()
        {
            self.load_history(messages);
        }
        if let Ok(messages) = store.load_agent_session(&self.session_id).await
            && !messages.is_empty()
        {
            self.restore_trajectory(&messages);
        }
    }

    /// Rebuild the ledger from a resumed session's transcript.
    ///
    /// A resumed session replays stored messages rather than the live update
    /// stream, so without this the page opens empty while the transcript still
    /// holds every tool call the run made. The agent-shaped transcript is the
    /// input, not the legacy `Message`: only it carries each tool result's
    /// terminal status, and a restored row that guessed "success" would be
    /// worse than one that shows none.
    ///
    /// Turn numbering follows the prompts, exactly as
    /// [`Self::begin_trajectory_turn`] does live. Injected cells are evidence,
    /// not turns, so they are recorded and do not open one.
    pub(crate) fn restore_trajectory(&mut self, messages: &[tact::AgentMessage]) {
        use tact::{AgentContentBlock, AgentRole, MessageOrigin};

        for message in messages {
            if message.role == AgentRole::User
                && matches!(message.origin, MessageOrigin::InjectedContext { .. })
            {
                let text = plain_text(&message.content);
                if !text.trim().is_empty() {
                    self.trajectory.restore_context("hook context", &text);
                }
                continue;
            }
            if message.role == AgentRole::User
                && matches!(message.origin, MessageOrigin::GoalContext)
            {
                continue;
            }

            match message.role {
                AgentRole::User => {
                    // The prompt opens the turn; the tool results in the same
                    // logical message answer calls from the turn before it.
                    let prompt = message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            AgentContentBlock::Text { text } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    for block in &message.content {
                        if let AgentContentBlock::ToolResult {
                            tool_use_id,
                            content,
                            status,
                        } = block
                        {
                            self.trajectory.restore_tool_result(
                                tool_use_id,
                                content,
                                restore_status(*status),
                            );
                        }
                    }
                    if !prompt.trim().is_empty() {
                        self.begin_trajectory_turn(&prompt);
                    }
                }
                AgentRole::Assistant => {
                    for block in &message.content {
                        match block {
                            AgentContentBlock::Text { text } => {
                                if !text.trim().is_empty() {
                                    self.trajectory.restore_assistant(text);
                                }
                            }
                            AgentContentBlock::Thinking { thinking, .. } => {
                                self.trajectory.restore_thinking(thinking, false);
                            }
                            AgentContentBlock::RedactedThinking { .. } => {
                                self.trajectory.restore_thinking("", true);
                            }
                            AgentContentBlock::ToolUse { id, name, input } => {
                                let raw = input.to_string();
                                self.trajectory.restore_tool_use(id, name, Some(&raw));
                            }
                            // An assistant turn carries no results, and an
                            // image is not evidence a ledger row can hold.
                            AgentContentBlock::ToolResult { .. }
                            | AgentContentBlock::Image { .. } => {}
                        }
                    }
                }
            }
        }
        if self.show_trajectory {
            self.select_latest_trajectory_record();
        }
    }
}

/// The concatenated text blocks of a logical message.
fn plain_text(blocks: &[tact::AgentContentBlock]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            tact::AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The persisted status of one tool result, in the ledger's vocabulary.
///
/// `Unknown` — a row written before the status column existed — maps to `None`:
/// the ledger then shows no status rather than claiming the call succeeded.
fn restore_status(
    status: tact::ToolResultStatus,
) -> Option<agent_tui_kit::state::trajectory::TrajectoryStatus> {
    use agent_tui_kit::state::trajectory::TrajectoryStatus;
    match status {
        tact::ToolResultStatus::Success => Some(TrajectoryStatus::Success),
        tact::ToolResultStatus::Failed => Some(TrajectoryStatus::Failed),
        tact::ToolResultStatus::Cancelled => Some(TrajectoryStatus::Cancelled),
        tact::ToolResultStatus::Unknown => None,
    }
}

/// `HH:MM` in local time, matching the rest of the TUI's stamps.
fn now_label() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

/// Whether this update changed a record that is already in the ledger.
///
/// A `ToolProgress` rewrites a running record's live tail in place: no row is
/// appended, but the page has to repaint or the tail stays at its first chunk.
fn changed_running_record(update: &tact_protocol::AgentUpdate) -> bool {
    matches!(
        update,
        tact_protocol::AgentUpdate::ToolProgress { .. }
            | tact_protocol::AgentUpdate::ThinkingChunk(_)
            | tact_protocol::AgentUpdate::StreamChunk(_)
    )
}
