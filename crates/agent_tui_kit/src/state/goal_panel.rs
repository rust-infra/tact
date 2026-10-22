//! Sticky Goal overview state and pure formatting helpers.

use crate::{
    i18n::Messages,
    protocol::{GoalCriterionStatusSnapshot, GoalPhaseSnapshot, GoalSnapshot, GoalStatusSnapshot},
};

#[derive(Debug, Clone)]
pub struct GoalPanelState {
    pub snapshot: Vec<GoalSnapshot>,
    pub selected: usize,
    pub session_seen: bool,
    pub visible: bool,
    pub expanded: bool,
    pub scroll: usize,
    pub max_visible: usize,
}

impl Default for GoalPanelState {
    fn default() -> Self {
        Self {
            snapshot: Vec::new(),
            selected: 0,
            session_seen: false,
            visible: false,
            expanded: false,
            scroll: 0,
            max_visible: 12,
        }
    }
}

impl GoalPanelState {
    pub fn apply_snapshot(&mut self, goals: Vec<GoalSnapshot>) {
        let was_visible = self.visible;
        let selected_id = self.snapshot.get(self.selected).map(|goal| goal.id.clone());
        self.snapshot = goals;
        self.selected = selected_id
            .as_deref()
            .and_then(|id| self.snapshot.iter().position(|goal| goal.id == id))
            .unwrap_or(0)
            .min(self.snapshot.len().saturating_sub(1));
        self.session_seen = true;
        self.scroll = 0;
        self.visible = !self.snapshot.is_empty();
        if self.visible && !was_visible {
            self.expanded = true;
        } else if !self.visible {
            self.expanded = false;
        }
    }

    pub fn selected_goal_id(&self) -> Option<&str> {
        self.snapshot
            .get(self.selected)
            .map(|goal| goal.id.as_str())
    }
}

fn status_label(status: GoalStatusSnapshot) -> &'static str {
    match status {
        GoalStatusSnapshot::Draft => "draft",
        GoalStatusSnapshot::Queued => "queued",
        GoalStatusSnapshot::Running => "running",
        GoalStatusSnapshot::WaitingUser => "waiting_user",
        GoalStatusSnapshot::WaitingPermission => "waiting_permission",
        GoalStatusSnapshot::Paused => "paused",
        GoalStatusSnapshot::Verifying => "verifying",
        GoalStatusSnapshot::Succeeded => "succeeded",
        GoalStatusSnapshot::Failed => "failed",
        GoalStatusSnapshot::Cancelled => "cancelled",
    }
}

fn phase_label(phase: GoalPhaseSnapshot) -> &'static str {
    match phase {
        GoalPhaseSnapshot::Planning => "planning",
        GoalPhaseSnapshot::Execution => "execution",
        GoalPhaseSnapshot::Verification => "verification",
    }
}

fn criterion_marker(status: GoalCriterionStatusSnapshot) -> &'static str {
    match status {
        GoalCriterionStatusSnapshot::Pending => "[ ]",
        GoalCriterionStatusSnapshot::Passed => "[x]",
        GoalCriterionStatusSnapshot::Rejected => "[!]",
    }
}

fn row_text(goal: &GoalSnapshot) -> String {
    let budget = goal
        .run_budget
        .as_ref()
        .map(|budget| {
            format!(
                " · turns {}/{} · tools {}",
                budget.turns_used, budget.turn_limit, budget.tool_calls_used
            )
        })
        .unwrap_or_default();
    // A `running` Goal with no run budget has no activation in flight: the
    // run budget is published exactly while one is (see
    // `GoalRunBudgetSnapshot`). That state is legitimate — a run that produced
    // no observable progress ends and leaves the Goal resumable (Ch 28 §7) —
    // but a row that just says "running" claims work that is not happening.
    let idle = goal.status == GoalStatusSnapshot::Running && goal.run_budget.is_none();
    // No required criteria means completion is self-attested, so there is no
    // acceptance gate to report: `accept 0/0` would suggest a gate that does
    // not exist (Ch 28 §6).
    let acceptance = if goal.acceptance.required == 0 {
        String::new()
    } else {
        format!(
            " · accept {}/{}",
            goal.acceptance.passed, goal.acceptance.required
        )
    };
    format!(
        "{} {} · {}{}/{} · tasks {}/{}{}{}{}",
        goal.status.marker(),
        goal.title,
        status_label(goal.status),
        if idle { " (idle)" } else { "" },
        phase_label(goal.phase),
        goal.task_completed,
        goal.task_total,
        acceptance,
        budget,
        // Only `/goal list --all` shows archived Goals, and this is what tells
        // them apart there; the default list never draws the tag.
        if goal.archived { " · archived" } else { "" }
    )
}

pub fn format_goal_lines(goals: &[GoalSnapshot], scroll: usize, max_visible: usize) -> Vec<String> {
    let mut lines = Vec::new();
    for goal in goals {
        lines.push(row_text(goal));
        if let Some(reason) = goal.wait_reason.as_deref() {
            lines.push(format!("  ↳ {reason}"));
        }
        for criterion in goal.criteria.iter().filter(|criterion| criterion.required) {
            lines.push(format!(
                "  {} {}{}",
                criterion_marker(criterion.status),
                criterion.description,
                if criterion.human { " (human)" } else { "" }
            ));
        }
    }
    super::sticky_panel::scroll_window(lines, scroll, max_visible)
}

pub fn format_sticky_title_line(msgs: &Messages, goals: &[GoalSnapshot]) -> String {
    let active = goals
        .iter()
        .filter(|goal| !goal.status.is_terminal())
        .count();
    let total = goals.len();
    let focus = goals
        .iter()
        .find(|goal| !goal.status.is_terminal())
        .or_else(|| goals.last())
        .map(|goal| goal.title.as_str())
        .unwrap_or_default();
    if focus.is_empty() {
        format!("▸ {} {active}/{total}  ▼", msgs.goals_sticky_title)
    } else {
        format!(
            "▸ {} {active}/{total} · {focus}  ▼",
            msgs.goals_sticky_title
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{GoalAcceptanceSnapshot, GoalRunBudgetSnapshot};

    #[test]
    fn waiting_and_terminal_goals_format_with_progress() {
        let waiting = GoalSnapshot {
            title: "waiting".into(),
            status: GoalStatusSnapshot::WaitingUser,
            phase: GoalPhaseSnapshot::Verification,
            task_total: 2,
            task_completed: 1,
            acceptance: GoalAcceptanceSnapshot {
                required: 1,
                passed: 0,
                pending: 1,
                rejected: 0,
            },
            run_budget: Some(GoalRunBudgetSnapshot {
                turns_used: 2,
                turn_limit: 4,
                tool_calls_used: 3,
                ..Default::default()
            }),
            wait_reason: Some("review".into()),
            ..Default::default()
        };
        let mut done = waiting.clone();
        done.title = "done".into();
        done.status = GoalStatusSnapshot::Succeeded;
        done.wait_reason = None;
        let lines = format_goal_lines(&[waiting, done], 0, 20).join("\n");
        assert!(lines.contains("waiting_user/verification"));
        assert!(lines.contains("turns 2/4"));
        assert!(lines.contains("review"));
        assert!(lines.contains("succeeded/verification"));
    }

    /// Only `/goal list --all` draws an archived Goal, and the tag is what
    /// tells it apart from a live one there.
    #[test]
    fn archived_goals_are_tagged_in_the_row() {
        let live = {
            let mut goal = GoalSnapshot {
                title: "live".into(),
                status: GoalStatusSnapshot::Succeeded,
                ..Default::default()
            };
            goal.archived = false;
            goal
        };
        let mut filed = live.clone();
        filed.title = "filed".into();
        filed.archived = true;

        let lines = format_goal_lines(&[live, filed], 0, 20).join("\n");
        assert_eq!(lines.matches(" · archived").count(), 1, "{lines}");
        assert!(
            lines.contains("filed · succeeded") && lines.contains(" · archived"),
            "the outcome stays readable next to the tag: {lines}"
        );
    }

    /// A Goal with no required criteria has no acceptance gate; the row omits
    /// it rather than printing a meaningless `accept 0/0`.
    #[test]
    fn goals_without_acceptance_criteria_omit_the_accept_segment() {
        let gated = GoalSnapshot {
            title: "gated".into(),
            status: GoalStatusSnapshot::Running,
            phase: GoalPhaseSnapshot::Execution,
            acceptance: GoalAcceptanceSnapshot {
                required: 1,
                passed: 0,
                pending: 1,
                rejected: 0,
            },
            ..Default::default()
        };
        let mut ungated = gated.clone();
        ungated.title = "ungated".into();
        ungated.acceptance = GoalAcceptanceSnapshot::default();

        let lines = format_goal_lines(&[gated, ungated], 0, 20).join("\n");
        assert!(
            lines.contains("accept 0/1"),
            "the gate is reported: {lines}"
        );
        assert_eq!(lines.matches("accept ").count(), 1, "{lines}");
        assert!(
            !lines.contains("accept 0/0"),
            "no gate means no gate line: {lines}"
        );
    }

    /// A `running` Goal is only "doing work" while an activation holds the
    /// lease; with no run budget it is idle between activations, and the row
    /// must not claim otherwise.
    #[test]
    fn running_goals_without_an_active_run_are_marked_idle() {
        let active = GoalSnapshot {
            title: "active".into(),
            status: GoalStatusSnapshot::Running,
            phase: GoalPhaseSnapshot::Execution,
            run_budget: Some(GoalRunBudgetSnapshot {
                turns_used: 1,
                turn_limit: 500,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut idle = active.clone();
        idle.title = "idle".into();
        idle.run_budget = None;

        let lines = format_goal_lines(&[active, idle], 0, 20).join("\n");
        assert_eq!(lines.matches("running (idle)").count(), 1, "{lines}");
        assert!(
            lines.contains("active · running/execution"),
            "an activation in flight stays plain running: {lines}"
        );
        assert!(
            lines.contains("idle · running (idle)/execution"),
            "the idle row says so: {lines}"
        );
    }

    #[test]
    fn snapshot_refresh_preserves_selected_goal_by_id() {
        let mut state = GoalPanelState::default();
        state.apply_snapshot(vec![
            GoalSnapshot {
                id: "one".into(),
                ..Default::default()
            },
            GoalSnapshot {
                id: "two".into(),
                ..Default::default()
            },
        ]);
        state.selected = 1;
        state.apply_snapshot(vec![
            GoalSnapshot {
                id: "two".into(),
                ..Default::default()
            },
            GoalSnapshot {
                id: "three".into(),
                ..Default::default()
            },
        ]);
        assert_eq!(state.selected_goal_id(), Some("two"));
        assert_eq!(state.selected, 0);
    }
}
