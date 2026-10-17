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
    format!(
        "{} {} · {}/{} · tasks {}/{} · accept {}/{}{}{}",
        goal.status.marker(),
        goal.title,
        status_label(goal.status),
        phase_label(goal.phase),
        goal.task_completed,
        goal.task_total,
        goal.acceptance.passed,
        goal.acceptance.required,
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
