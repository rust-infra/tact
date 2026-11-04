//! Sticky task strip: which task is current, and how the list is ordered.
//!
//! The ordering follows `agent_tui_kit::state::task_panel`, so the desktop strip
//! and the TUI sticky host rank tasks the same way:
//!
//! - Two groups only — in progress, then pending. Completed rows are hidden:
//!   the strip is about what is still moving, and a finished task is history the
//!   ledger already carries.
//! - In progress sorts by elapsed time ascending, so the most recently started
//!   task leads — that is the one the agent is on right now.
//! - Pending sorts by `created_at` descending, so the newest request leads.
//!
//! Both groups therefore lead with the newest activity, which is what makes the
//! strip readable at a glance: the top row is always what just happened.
//!
//! Everything here is pure so the ranking can be asserted without a window.

use tact_desktop_core::{
    BackgroundTaskStatusSnapshot, BackgroundTaskView, GateCard, GoalView, InspectorView,
    SubagentStatusSnapshot, SubagentView, TaskStatusSnapshot, TaskView,
};

/// The task the strip calls current when collapsed: the in-progress one, or
/// failing that the first task still waiting.
///
/// In-progress wins over pending because it is the work actually happening; a
/// pending row is only current when nothing is running.
pub fn active_task(inspector: &InspectorView) -> Option<&TaskView> {
    inspector
        .tasks
        .iter()
        .find(|task| task.status == TaskStatusSnapshot::InProgress)
        .or_else(|| {
            inspector
                .tasks
                .iter()
                .find(|task| task.status == TaskStatusSnapshot::Pending)
        })
}

/// A status the strip paints a group for.
///
/// Deliberately narrower than [`TaskStatusSnapshot`]: completed work is not a
/// group, because `agent_tui_kit` hides it from the sticky and the panel's
/// `n/m` counter is what reports it. Owning a type that cannot name `Completed`
/// keeps every downstream `match` exhaustive in two arms instead of carrying an
/// arm no run can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupStatus {
    InProgress,
    Pending,
}

impl GroupStatus {
    /// The group a task belongs to, or `None` when the strip does not show it.
    pub fn of(status: TaskStatusSnapshot) -> Option<Self> {
        match status {
            TaskStatusSnapshot::InProgress => Some(Self::InProgress),
            TaskStatusSnapshot::Pending => Some(Self::Pending),
            TaskStatusSnapshot::Completed => None,
        }
    }
}

/// The groups the strip paints, in the order it paints them: in progress first,
/// then pending.
pub const GROUPS: [GroupStatus; 2] = [GroupStatus::InProgress, GroupStatus::Pending];

/// One painted group: the status it covers and that status's rows, already
/// ordered the way the group paints them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskGroup<'a> {
    pub status: GroupStatus,
    pub tasks: Vec<&'a TaskView>,
}

/// Groups `tasks` by status in [`GROUPS`] order, dropping empty groups.
///
/// `now_ms` is passed in rather than read here so a test can pin the elapsed
/// ranking. Callers pass the current wall clock.
pub fn group_tasks<'a>(tasks: &[&'a TaskView], now_ms: i64) -> Vec<TaskGroup<'a>> {
    GROUPS
        .into_iter()
        .filter_map(|status| {
            let mut matching: Vec<&'a TaskView> = tasks
                .iter()
                .copied()
                .filter(|task| GroupStatus::of(task.status) == Some(status))
                .collect();
            match status {
                // Ascending elapsed: the most recently started task leads.
                // `unwrap_or(0)` matches the kit on purpose — a row that never
                // recorded a start has an unbounded-looking elapsed time and
                // sinks to the bottom instead of leading the list.
                GroupStatus::InProgress => {
                    matching.sort_by_key(|task| (now_ms - task.started_at.unwrap_or(0)).max(0))
                }
                // Newest request first.
                GroupStatus::Pending => {
                    matching.sort_by_key(|task| std::cmp::Reverse(task.created_at))
                }
            }
            (!matching.is_empty()).then_some(TaskGroup {
                status,
                tasks: matching,
            })
        })
        .collect()
}

/// How many task rows the expanded strip shows before it starts scrolling.
///
/// The same cap the TUI host uses (`TaskPanelState::max_visible`), so both
/// surfaces show the same number of rows.
pub const MAX_VISIBLE_ROWS: usize = 10;

/// The rows the expanded strip paints, given a scroll offset.
///
/// Returns the rows to paint plus the marker row that reports what is out of
/// view. Mirrors `agent_tui_kit::state::sticky_panel::scroll_window`: the
/// marker is *appended* and displaces the row it reports on, it does not add a
/// row. With everything visible and nothing scrolled there is no marker.
pub fn window_rows<'a>(
    groups: &[TaskGroup<'a>],
    scroll: usize,
) -> (Vec<(GroupStatus, &'a TaskView)>, Option<String>) {
    let flat: Vec<(GroupStatus, &'a TaskView)> = groups
        .iter()
        .flat_map(|group| group.tasks.iter().map(move |task| (group.status, *task)))
        .collect();
    let total = flat.len();
    if total <= MAX_VISIBLE_ROWS {
        return (flat, None);
    }
    let scroll = scroll.min(total.saturating_sub(MAX_VISIBLE_ROWS));
    let visible = flat
        .into_iter()
        .skip(scroll)
        .take(MAX_VISIBLE_ROWS)
        .collect();
    let remaining = total.saturating_sub(scroll + MAX_VISIBLE_ROWS);
    let marker = if remaining > 0 {
        Some(format!("+{remaining} more"))
    } else if scroll > 0 {
        Some("scroll up".to_string())
    } else {
        None
    };
    (visible, marker)
}

/// `"12s"` / `"3m 5s"` / `"2h 07m"` for a task's span, or `None` when there is
/// nothing to say.
///
/// Same shape and the same refusal as `agent_tui_kit::state::sticky_panel`:
/// a span that never started, or that is not over yet, is not a zero-second
/// label — it is a missing one, and the row omits it.
pub fn elapsed_label(started_at: Option<i64>, end_at: Option<i64>, now_ms: i64) -> Option<String> {
    let start = started_at?;
    let end = end_at.unwrap_or(now_ms);
    if end <= start {
        return None;
    }
    let secs = (end - start) / 1_000;
    if secs < 60 {
        Some(format!("{secs}s"))
    } else if secs < 3_600 {
        Some(format!("{}m {}s", secs / 60, secs % 60))
    } else {
        Some(format!("{}h {:02}m", secs / 3_600, (secs % 3_600) / 60))
    }
}

/// Whether the task card paints at all.
///
/// The TUI host hides its sticky when every domain is quiet, and the desktop
/// card merges those domains into one header, so the card stays up while any
/// of them would keep the TUI strip up:
///
/// - tasks: a pending or in-progress row — the kit's `has_open_items`. A list
///   that is all completed is history, and the ledger already carries it;
/// - goals: any at all;
/// - subagents / background: a still-running one.
///
/// A finished background task keeps its row only for the kit's linger window;
/// the desktop has no idle tick to prune on, so it drops out of this decision
/// the moment it ends. Its chip still shows while the card is up for another
/// reason.
pub fn card_visible(
    tasks: &[TaskView],
    goals: &[GoalView],
    subagents: &[SubagentView],
    background: &[BackgroundTaskView],
) -> bool {
    tasks.iter().any(|task| {
        matches!(
            task.status,
            TaskStatusSnapshot::Pending | TaskStatusSnapshot::InProgress
        )
    }) || !goals.is_empty()
        || subagents
            .iter()
            .any(|agent| agent.status == SubagentStatusSnapshot::Running)
        || background
            .iter()
            .any(|task| task.status == BackgroundTaskStatusSnapshot::Running)
}

pub fn pending_gate(inspector_pending: &[GateCard]) -> Option<&GateCard> {
    inspector_pending.first()
}


