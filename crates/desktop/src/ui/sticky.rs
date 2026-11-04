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

use tact_desktop_core::{GateCard, InspectorView, TaskStatusSnapshot, TaskView};

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

pub fn pending_gate(inspector_pending: &[GateCard]) -> Option<&GateCard> {
    inspector_pending.first()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: u64, status: TaskStatusSnapshot) -> TaskView {
        TaskView {
            id,
            status,
            ..TaskView::default()
        }
    }

    #[test]
    fn groups_put_in_progress_first_and_hide_completed_work() {
        let done = task(1, TaskStatusSnapshot::Completed);
        let doing = task(2, TaskStatusSnapshot::InProgress);
        let todo = task(3, TaskStatusSnapshot::Pending);
        let all = [&done, &doing, &todo];

        let groups = group_tasks(&all, 0);
        assert_eq!(
            groups.iter().map(|group| group.status).collect::<Vec<_>>(),
            vec![GroupStatus::InProgress, GroupStatus::Pending],
            "completed is not a group the strip paints"
        );
        assert_eq!(groups[0].tasks.len(), 1);
        assert_eq!(groups[0].tasks[0].id, 2, "the in-progress task leads");
    }

    #[test]
    fn a_group_with_no_tasks_is_not_painted() {
        let done = task(1, TaskStatusSnapshot::Completed);
        assert!(
            group_tasks(&[&done], 0).is_empty(),
            "one finished task leaves nothing to paint"
        );

        let todo = task(2, TaskStatusSnapshot::Pending);
        let groups = group_tasks(&[&todo], 0);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].status, GroupStatus::Pending);
        assert!(group_tasks(&[], 0).is_empty());
    }

    #[test]
    fn in_progress_leads_with_the_most_recently_started_task() {
        let now = 1_000_000;
        let just_started = TaskView {
            started_at: Some(now - 1_000),
            ..task(10, TaskStatusSnapshot::InProgress)
        };
        let stuck = TaskView {
            started_at: Some(now - 900_000),
            ..task(11, TaskStatusSnapshot::InProgress)
        };
        let unstarted = TaskView {
            started_at: None,
            ..task(12, TaskStatusSnapshot::InProgress)
        };

        let groups = group_tasks(&[&stuck, &just_started, &unstarted], now);
        assert_eq!(
            groups[0]
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![10, 11, 12],
            "newest start first, unstarted last"
        );
    }

    #[test]
    fn pending_leads_with_the_newest_request() {
        let older = TaskView {
            created_at: Some(1_000),
            ..task(20, TaskStatusSnapshot::Pending)
        };
        let newer = TaskView {
            created_at: Some(5_000),
            ..task(21, TaskStatusSnapshot::Pending)
        };
        let undated = task(22, TaskStatusSnapshot::Pending);

        let groups = group_tasks(&[&older, &undated, &newer], 0);
        assert_eq!(
            groups[0]
                .tasks
                .iter()
                .map(|task| task.id)
                .collect::<Vec<_>>(),
            vec![21, 20, 22],
            "newest first, undated last"
        );
    }

    fn pending_tasks(count: u64) -> Vec<TaskView> {
        (1..=count)
            .map(|id| TaskView {
                created_at: Some(id as i64),
                ..task(id, TaskStatusSnapshot::Pending)
            })
            .collect()
    }

    #[test]
    fn a_list_that_fits_paints_every_row_and_no_marker() {
        let tasks = pending_tasks(3);
        let refs: Vec<&TaskView> = tasks.iter().collect();
        let groups = group_tasks(&refs, 0);
        let (rows, marker) = window_rows(&groups, 0);
        assert_eq!(rows.len(), 3);
        assert_eq!(marker, None);
    }

    #[test]
    fn overflow_is_capped_and_reports_what_is_left() {
        let tasks = pending_tasks(15);
        let refs: Vec<&TaskView> = tasks.iter().collect();
        let groups = group_tasks(&refs, 0);
        let (rows, marker) = window_rows(&groups, 0);
        assert_eq!(rows.len(), MAX_VISIBLE_ROWS);
        assert_eq!(marker.as_deref(), Some("+5 more"));
    }

    #[test]
    fn scrolling_past_the_end_clamps_and_flips_the_marker() {
        let tasks = pending_tasks(12);
        let refs: Vec<&TaskView> = tasks.iter().collect();
        let groups = group_tasks(&refs, 0);
        let (rows, marker) = window_rows(&groups, 99);
        assert_eq!(rows.len(), MAX_VISIBLE_ROWS);
        assert_eq!(marker.as_deref(), Some("scroll up"));
    }

    #[test]
    fn an_elapsed_label_needs_a_start_and_a_positive_span() {
        assert_eq!(elapsed_label(None, Some(5_000), 5_000), None, "no start");
        assert_eq!(
            elapsed_label(Some(5_000), Some(5_000), 5_000),
            None,
            "a zero span is a missing label, not 0s"
        );
        assert_eq!(elapsed_label(Some(6_000), Some(5_000), 5_000), None);
    }

    #[test]
    fn an_elapsed_label_shortens_by_scale_and_runs_up_to_now() {
        let start = 1_000_000_000_000;
        assert_eq!(
            elapsed_label(Some(start), Some(start + 12_000), start).as_deref(),
            Some("12s")
        );
        assert_eq!(
            elapsed_label(Some(start), Some(start + 185_000), start).as_deref(),
            Some("3m 5s")
        );
        assert_eq!(
            elapsed_label(Some(start), Some(start + 7_620_000), start).as_deref(),
            Some("2h 07m")
        );
        // A running task has no end yet and is measured against the clock.
        assert_eq!(
            elapsed_label(Some(start), None, start + 30_000).as_deref(),
            Some("30s")
        );
    }

    #[test]
    fn the_current_task_prefers_in_progress_over_pending() {
        let with = |tasks: [TaskView; 3]| InspectorView {
            tasks: std::sync::Arc::from(tasks),
            ..InspectorView::default()
        };
        let running = with([
            task(1, TaskStatusSnapshot::Pending),
            task(2, TaskStatusSnapshot::InProgress),
            task(3, TaskStatusSnapshot::Completed),
        ]);
        assert_eq!(active_task(&running).map(|task| task.id), Some(2));

        // With nothing running, the first row still waiting is current.
        let waiting = with([
            task(4, TaskStatusSnapshot::Pending),
            task(5, TaskStatusSnapshot::Completed),
            task(7, TaskStatusSnapshot::Completed),
        ]);
        assert_eq!(active_task(&waiting).map(|task| task.id), Some(4));

        // Everything done means no current task rather than a stale one.
        let finished = with([
            task(6, TaskStatusSnapshot::Completed),
            task(8, TaskStatusSnapshot::Completed),
            task(9, TaskStatusSnapshot::Completed),
        ]);
        assert!(active_task(&finished).is_none());
    }
}
