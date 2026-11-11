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

use super::super::{App, InputMode};

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
        let painted = rows[..position]
            .iter()
            .map(|row| if row.starts_turn { 2 } else { 1 })
            .sum::<usize>();
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
            .trajectory_page_area
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
        self.trajectory.observe(update);
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
}

/// `HH:MM` in local time, matching the rest of the TUI's stamps.
fn now_label() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}
