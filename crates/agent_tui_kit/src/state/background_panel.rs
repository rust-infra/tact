//! Background sticky-overview panel state and pure format helpers.
//!
//! Third sticky domain, after `task_panel.rs` (Tasks) and `subagent_panel.rs`
//! (Subagent). Unlike those two it has **no protocol snapshot event**: a
//! background task starts through exactly one tool (`background_run`), and the
//! live tool card already carries everything the strip shows —
//!
//! - the task id (`RuntimeEvent::ToolMeta { task_id }`, sent once the task
//!   exists and the invocation is about to return),
//! - the command (the card's argument summary; the tool's metadata uses
//!   `ArgumentSummaryPolicy::Command { field: "command" }`),
//! - the start instant ([`crate::state::tool_state::ActiveToolBlock::started_at`]).
//!
//! So the rows are *derived* from the live tool cards on every tick
//! (`App::sync_background_sticky`) instead of being pushed, and this module owns
//! only the presentation state the user toggles (visible/expanded/scroll).
//! "Active card with a task id" is exactly "task still running" because the
//! `background_run` card is `keep_live`: it stays in `ToolState::active` until
//! `BackgroundTaskFinished` closes it — including the `wait_ms` variant.
//!
//! Visibility rule (mirrors [`crate::state::SubagentPanelState`]): the domain
//! shows while at least one task is running or lingering, expands on first
//! appearance, and collapses once the last row is gone. A task that finishes
//! keeps a row for [`BACKGROUND_LINGER`] — a row that vanishes the instant the
//! task ends reads as "the strip lost a task", which is exactly the report this
//! window exists to prevent. Its output still lives on the tool card and in
//! `/background`.

use std::time::{Duration, Instant};

use crate::{i18n::Messages, state::ToolState};

/// How long a finished task keeps its row on the strip.
///
/// Chosen to survive one idle-tick repaint cadence (~1/s) with room to spare,
/// so the row is always readable before it drops.
pub const BACKGROUND_LINGER: Duration = Duration::from_secs(8);

/// Cap on remembered finished rows: a burst of completions must not grow the
/// strip (or this vector) without bound.
const MAX_FINISHED_ROWS: usize = 20;

/// One live background task, reduced to what the sticky row needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundTaskRow {
    /// Short id the agent uses to poll the task (`/background <id>`).
    pub task_id: String,
    /// The command, first line only.
    pub command: String,
    pub elapsed_secs: u64,
}

/// A task that ended recently enough to keep showing on the strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinishedBackgroundTask {
    /// Same id the running row showed (`/background <id>` still finds it).
    pub task_id: String,
    /// The command, first line only.
    pub command: String,
    pub success: bool,
    pub finished_at: Instant,
}

impl FinishedBackgroundTask {
    /// Still inside the linger window at `now`.
    pub fn is_live(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.finished_at) < BACKGROUND_LINGER
    }

    fn ago_secs(&self, now: Instant) -> u64 {
        now.saturating_duration_since(self.finished_at).as_secs()
    }
}

/// Rows for the background sticky: one per live `background_run` card, in
/// start order (the order `ToolState::active` keeps).
pub fn running_background_tasks(tools: &ToolState) -> Vec<BackgroundTaskRow> {
    tools
        .active
        .iter()
        .filter_map(|active| {
            // Only `background_run` publishes a `task_id` on its card; the
            // `check_background` / `wait_background` cards merely take one as
            // an argument, and never set this field.
            let task_id = active.output.task_id.as_deref()?;
            Some(BackgroundTaskRow {
                task_id: task_id.to_string(),
                command: first_line(&active.output.arg_summary),
                elapsed_secs: active.started_at.elapsed().as_secs(),
            })
        })
        .collect()
}

/// Panel state the user toggles. Everything else about this domain is derived.
#[derive(Debug, Clone)]
pub struct BackgroundPanelState {
    /// Set by [`Self::apply_running`]; the host draws nothing while false.
    pub visible: bool,
    pub expanded: bool,
    pub scroll: usize,
    pub max_visible: usize,
    /// Tasks that finished inside the linger window, oldest first. Fed by the
    /// shell (`BackgroundTaskFinished` + the finalized card), pruned by
    /// [`Self::prune_finished`].
    pub finished: Vec<FinishedBackgroundTask>,
}

impl Default for BackgroundPanelState {
    fn default() -> Self {
        Self::new()
    }
}

impl BackgroundPanelState {
    pub fn new() -> Self {
        Self {
            visible: false,
            expanded: false,
            scroll: 0,
            max_visible: 10,
            finished: Vec::new(),
        }
    }

    /// Remember a task that just ended so its row lingers for
    /// [`BACKGROUND_LINGER`]. The command is the card's argument summary.
    pub fn note_finished(&mut self, task_id: String, command: String, success: bool, at: Instant) {
        // A re-run of the same id (or a duplicate event) must not stack rows.
        self.finished.retain(|t| t.task_id != task_id);
        self.finished.push(FinishedBackgroundTask {
            task_id,
            command: first_line(&command),
            success,
            finished_at: at,
        });
        let overflow = self.finished.len().saturating_sub(MAX_FINISHED_ROWS);
        if overflow > 0 {
            self.finished.drain(..overflow);
        }
    }

    /// Drop rows whose linger window has passed. Returns whether anything was
    /// dropped, so the caller can re-apply visibility and repaint.
    pub fn prune_finished(&mut self, now: Instant) -> bool {
        let before = self.finished.len();
        self.finished.retain(|t| t.is_live(now));
        before != self.finished.len()
    }

    /// Reconcile the panel with `running` live tasks. Returns whether anything
    /// the host draws changed (so the caller can raise its dirty flag).
    ///
    /// A finished row keeps the strip visible on its own: the strip disappears
    /// only once the last row (running or lingering) is gone.
    pub fn apply_running(&mut self, running: usize) -> bool {
        let before = (self.visible, self.expanded);
        self.visible = running > 0 || !self.finished.is_empty();
        if self.visible {
            // Default expanded when the strip first appears (or reappears).
            if !before.0 {
                self.expanded = true;
                self.scroll = 0;
            }
        } else {
            self.expanded = false;
            self.scroll = 0;
        }
        (self.visible, self.expanded) != before
    }
}

fn first_line(command: &str) -> String {
    command.lines().next().unwrap_or("").trim().to_string()
}

/// `1s` / `2m 5s` / `1h 03m` — the shapes the subagent rows use.
fn format_elapsed(secs: u64) -> String {
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
    }
}

fn row_text(row: &BackgroundTaskRow) -> String {
    let command = if row.command.is_empty() {
        "(no command)"
    } else {
        row.command.as_str()
    };
    format!(
        "⏳ {} {command}  ⏱ {}",
        row.task_id,
        format_elapsed(row.elapsed_secs)
    )
}

/// `✓ 018f3a2c cargo build  ⏱ 3s` — a task whose row is lingering since it
/// finished; the glyph carries the outcome and the elapsed reads as "how long
/// ago" (it stays in single-digit seconds for the whole window).
fn finished_row_text(row: &FinishedBackgroundTask, now: Instant) -> String {
    let command = if row.command.is_empty() {
        "(no command)"
    } else {
        row.command.as_str()
    };
    let glyph = if row.success { '✓' } else { '✗' };
    format!(
        "{glyph} {} {command}  ⏱ {}",
        row.task_id,
        format_elapsed(row.ago_secs(now))
    )
}

/// Sticky-body lines: running rows first (start order), then the rows of tasks
/// that finished inside the linger window (finish order). Scroll-clamped like
/// the sibling domains.
pub fn format_background_lines(
    running: &[BackgroundTaskRow],
    finished: &[FinishedBackgroundTask],
    now: Instant,
    scroll: usize,
    max_visible: usize,
) -> Vec<String> {
    let mut all: Vec<String> = running.iter().map(row_text).collect();
    all.extend(
        finished
            .iter()
            .filter(|row| row.is_live(now))
            .map(|row| finished_row_text(row, now)),
    );
    if all.is_empty() {
        return Vec::new();
    }
    let total = all.len();
    if total <= max_visible {
        return all;
    }
    let scroll = scroll.min(total.saturating_sub(max_visible));
    let mut visible: Vec<String> = all.iter().skip(scroll).take(max_visible).cloned().collect();
    let remaining = total.saturating_sub(scroll + max_visible);
    if remaining > 0 {
        visible.push(format!("⋯ +{remaining} more · scroll ▼"));
    } else if scroll > 0 {
        visible.push("⋯ scroll ▲".into());
    }
    visible
}

/// Title row text: `▸ Background 2 · 018f3a2c cargo build  ▼`. The host strips
/// the leading `▸ ` and the repeated label, so the row reads
/// `[Background] 2 · …`. Lingering rows are folded into the same segment, so a
/// finish never makes the strip look like it lost a task:
/// `[Background] 1 · 2 done · …`.
pub fn format_sticky_title_line(
    msgs: &Messages,
    running: &[BackgroundTaskRow],
    finished: &[FinishedBackgroundTask],
    now: Instant,
) -> String {
    let title = msgs.background_sticky_title;
    let done: Vec<&FinishedBackgroundTask> =
        finished.iter().filter(|row| row.is_live(now)).collect();
    let head = match (running.is_empty(), done.is_empty()) {
        (_, true) => running.len().to_string(),
        (true, false) => format!("{} {}", done.len(), msgs.background_sticky_done),
        (false, false) => format!(
            "{} · {} {}",
            running.len(),
            done.len(),
            msgs.background_sticky_done
        ),
    };
    let focus = running
        .first()
        .map(row_text)
        .or_else(|| done.first().map(|row| finished_row_text(row, now)))
        .unwrap_or_default();
    if focus.is_empty() {
        format!("▸ {title} {head}  ▼")
    } else {
        format!("▸ {title} {head} · {focus}  ▼")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        state::{ActiveToolBlock, ToolState},
        widgets::tool_widget::ToolWidget,
    };

    fn live_card(tool_id: &str, task_id: Option<&str>, command: &str) -> ActiveToolBlock {
        let output = ToolWidget::new()
            .with_tool("background_run")
            .with_arg_summary(command)
            .with_task_id(task_id.map(str::to_string))
            .build();
        ActiveToolBlock {
            phys_idx: 0,
            tool_id: tool_id.into(),
            output,
            live_output: tact_protocol::ToolOutputBuffer::new(1_000),
            started_at: std::time::Instant::now(),
            subagent_child_id: None,
        }
    }

    fn tools_with(cards: Vec<ActiveToolBlock>) -> ToolState {
        ToolState {
            active: cards,
            ..Default::default()
        }
    }

    #[test]
    fn a_live_card_with_a_task_id_is_a_running_row() {
        let tools = tools_with(vec![live_card("bg1", Some("018f3a2c"), "cargo build")]);
        let rows = running_background_tasks(&tools);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].task_id, "018f3a2c");
        assert_eq!(rows[0].command, "cargo build");
    }

    #[test]
    fn cards_without_a_task_id_are_not_background_rows() {
        // The invocation has started but `background_run` has not sent its
        // `ToolMeta` yet — there is no id to show, so the domain stays empty.
        let tools = tools_with(vec![live_card("bg1", None, "cargo build")]);
        assert!(running_background_tasks(&tools).is_empty());
    }

    #[test]
    fn only_the_first_command_line_reaches_the_row() {
        let tools = tools_with(vec![live_card(
            "bg1",
            Some("018f3a2c"),
            "cargo build\ncargo test",
        )]);
        let rows = running_background_tasks(&tools);
        assert_eq!(rows[0].command, "cargo build");
        let lines = format_background_lines(&rows, &[], Instant::now(), 0, 10);
        assert!(!lines[0].contains("cargo test"), "row: {}", lines[0]);
    }

    #[test]
    fn apply_running_expands_on_first_appearance_and_collapses_at_the_end() {
        let mut panel = BackgroundPanelState::new();
        assert!(!panel.visible && !panel.expanded);

        assert!(panel.apply_running(1), "appearing changes what is drawn");
        assert!(panel.visible && panel.expanded);

        // A second task starting must not re-collapse or re-expand the strip.
        assert!(!panel.apply_running(2));
        assert!(panel.visible && panel.expanded);

        assert!(panel.apply_running(0));
        assert!(!panel.visible && !panel.expanded);
    }

    #[test]
    fn a_collapsed_strip_stays_collapsed_while_tasks_keep_running() {
        let mut panel = BackgroundPanelState::new();
        panel.apply_running(1);
        panel.expanded = false;
        assert!(!panel.apply_running(2), "no visibility change to report");
        assert!(!panel.expanded, "the user's collapse must survive");
    }

    #[test]
    fn format_lines_clamp_scroll_and_announce_the_tail() {
        let rows: Vec<BackgroundTaskRow> = (0..12)
            .map(|i| BackgroundTaskRow {
                task_id: format!("id{i}"),
                command: format!("cmd{i}"),
                elapsed_secs: i,
            })
            .collect();
        let now = Instant::now();
        let all = format_background_lines(&rows, &[], now, 0, 20);
        assert_eq!(all.len(), 12);

        let page = format_background_lines(&rows, &[], now, 0, 10);
        assert_eq!(page.len(), 11);
        assert!(page[0].contains("id0"));
        assert_eq!(page[10], "⋯ +2 more · scroll ▼");

        let tail = format_background_lines(&rows, &[], now, 99, 10);
        assert!(tail[0].contains("id2"), "scroll clamps to the last page");
        assert_eq!(tail[10], "⋯ scroll ▲");
    }

    #[test]
    fn elapsed_shapes_match_the_subagent_rows() {
        assert_eq!(format_elapsed(5), "5s");
        assert_eq!(format_elapsed(125), "2m 5s");
        assert_eq!(format_elapsed(3785), "1h 03m");
    }

    #[test]
    fn sticky_title_carries_the_count_and_the_first_row() {
        let msgs = Messages::by_language(crate::i18n::Language::English);
        let rows = vec![BackgroundTaskRow {
            task_id: "018f3a2c".into(),
            command: "cargo build".into(),
            elapsed_secs: 3,
        }];
        let title = format_sticky_title_line(&msgs, &rows, &[], Instant::now());
        assert!(title.starts_with("▸ Background 1 · "), "title: {title}");
        assert!(title.contains("018f3a2c"), "title: {title}");

        // The host strips `▸ ` plus the label word; what remains must read well.
        let rest = title.trim_start_matches('▸').trim_start();
        let rest = rest.strip_prefix(msgs.background_sticky_title).unwrap();
        assert!(rest.trim_start().starts_with("1 · "), "rest: {rest}");
    }

    // ── the linger window ───────────────────────────────────────────────

    fn finished_row(
        task_id: &str,
        command: &str,
        success: bool,
        at: Instant,
    ) -> FinishedBackgroundTask {
        FinishedBackgroundTask {
            task_id: task_id.into(),
            command: command.into(),
            success,
            finished_at: at,
        }
    }

    #[test]
    fn a_finished_task_keeps_its_row_inside_the_linger_window() {
        let now = Instant::now();
        let row = finished_row(
            "018f3a2c",
            "cargo build",
            true,
            now - Duration::from_secs(3),
        );
        let lines = format_background_lines(&[], &[row], now, 0, 10);
        assert_eq!(lines.len(), 1, "lines: {lines:?}");
        assert!(lines[0].starts_with('✓'), "row: {}", lines[0]);
        assert!(
            lines[0].contains("018f3a2c") && lines[0].contains("cargo build"),
            "row: {}",
            lines[0]
        );
        assert!(
            lines[0].contains("3s"),
            "elapsed reads as 'ago': {}",
            lines[0]
        );
    }

    #[test]
    fn a_failed_task_lingers_under_the_failure_glyph() {
        let now = Instant::now();
        let row = finished_row("018f3a2c", "cargo build", false, now);
        let lines = format_background_lines(&[], &[row], now, 0, 10);
        assert!(lines[0].starts_with('✗'), "row: {}", lines[0]);
    }

    #[test]
    fn rows_drop_once_the_linger_window_passes() {
        let now = Instant::now();
        let mut panel = BackgroundPanelState::new();
        let row = finished_row("018f3a2c", "cargo build", true, now);
        assert!(
            !panel.prune_finished(now + Duration::from_secs(1)),
            "still fresh"
        );
        assert_eq!(
            format_background_lines(&[], &[row], now + Duration::from_secs(1), 0, 10).len(),
            1
        );

        let later = now + BACKGROUND_LINGER;
        let rows = [finished_row("018f3a2c", "cargo build", true, now)];
        assert!(
            format_background_lines(&[], &rows, later, 0, 10).is_empty(),
            "an expired row is never drawn, even before the prune"
        );
    }

    #[test]
    fn the_strip_stays_visible_for_a_lingering_row_only() {
        let now = Instant::now();
        let mut panel = BackgroundPanelState::new();
        panel.note_finished("018f3a2c".into(), "cargo build".into(), true, now);

        assert!(panel.apply_running(0), "a lingering row keeps the strip up");
        assert!(panel.visible && panel.expanded);

        assert!(
            panel.prune_finished(now + BACKGROUND_LINGER),
            "the last row expires"
        );
        assert!(
            panel.apply_running(0),
            "…and the strip then hides without any task event"
        );
        assert!(!panel.visible && !panel.expanded);
    }

    #[test]
    fn note_finished_replaces_a_repeat_and_bounds_the_list() {
        let now = Instant::now();
        let mut panel = BackgroundPanelState::new();
        panel.note_finished("a".into(), "first".into(), true, now);
        panel.note_finished("a".into(), "second".into(), false, now);
        assert_eq!(panel.finished.len(), 1, "one row per task id");
        assert_eq!(panel.finished[0].command, "second");
        assert!(!panel.finished[0].success);

        for i in 0..(MAX_FINISHED_ROWS + 5) {
            panel.note_finished(format!("id{i}"), "cmd".into(), true, now);
        }
        assert_eq!(
            panel.finished.len(),
            MAX_FINISHED_ROWS,
            "a burst of completions must not grow the strip without bound"
        );
    }

    #[test]
    fn running_rows_come_before_lingering_rows() {
        let now = Instant::now();
        let tools = tools_with(vec![live_card("bg1", Some("run-id"), "cargo test")]);
        let rows = running_background_tasks(&tools);
        let finished = [finished_row("done-id", "cargo build", true, now)];

        let lines = format_background_lines(&rows, &finished, now, 0, 10);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].starts_with('⏳') && lines[0].contains("run-id"));
        assert!(lines[1].starts_with('✓') && lines[1].contains("done-id"));
    }

    #[test]
    fn the_title_counts_running_and_done() {
        let msgs = Messages::by_language(crate::i18n::Language::English);
        let now = Instant::now();
        let running = vec![BackgroundTaskRow {
            task_id: "run-id".into(),
            command: "cargo test".into(),
            elapsed_secs: 3,
        }];
        let finished = [finished_row("done-id", "cargo build", true, now)];

        let both = format_sticky_title_line(&msgs, &running, &finished, now);
        assert!(
            both.starts_with("▸ Background 1 · 1 done · "),
            "title: {both}"
        );

        // Nothing running: the head must not read as a bare `0`.
        let only_done = format_sticky_title_line(&msgs, &[], &finished, now);
        assert!(
            only_done.starts_with("▸ Background 1 done · "),
            "title: {only_done}"
        );
        assert!(only_done.contains("done-id"), "title: {only_done}");

        // An expired row is not counted and does not become the focus.
        let expired = format_sticky_title_line(&msgs, &[], &finished, now + BACKGROUND_LINGER);
        assert_eq!(expired, "▸ Background 0  ▼");
    }
}
