use tact_desktop_core::{GoalView, InspectorTab, RowStatus, SubagentView, TaskView};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorRowKind {
    Goal,
    Task,
    Subagent,
    Background,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InspectorRow {
    pub id: String,
    pub kind: InspectorRowKind,
    pub title: String,
    pub status: RowStatus,
    pub detail: String,
}

pub fn tabs() -> [(InspectorTab, &'static str); 4] {
    [
        (InspectorTab::Run, "Run"),
        (InspectorTab::Goals, "Goals"),
        (InspectorTab::Subagents, "Subagents"),
        (InspectorTab::Context, "Context"),
    ]
}

pub fn goal_row(goal: &GoalView) -> InspectorRow {
    let snapshot = goal.snapshot.as_ref();
    InspectorRow {
        id: snapshot.map(|goal| goal.id.clone()).unwrap_or_default(),
        kind: InspectorRowKind::Goal,
        title: snapshot
            .map(|goal| goal.title.clone())
            .unwrap_or_else(|| "Goal".into()),
        status: snapshot
            .map(|goal| match goal.status {
                tact_desktop_core::GoalStatusSnapshot::Running => RowStatus::Running,
                tact_desktop_core::GoalStatusSnapshot::Queued
                | tact_desktop_core::GoalStatusSnapshot::Draft => RowStatus::Pending,
                tact_desktop_core::GoalStatusSnapshot::WaitingUser
                | tact_desktop_core::GoalStatusSnapshot::WaitingPermission
                | tact_desktop_core::GoalStatusSnapshot::Verifying => RowStatus::Running,
                tact_desktop_core::GoalStatusSnapshot::Paused => RowStatus::Pending,
                tact_desktop_core::GoalStatusSnapshot::Succeeded => RowStatus::Resolved,
                tact_desktop_core::GoalStatusSnapshot::Failed => RowStatus::Failed,
                tact_desktop_core::GoalStatusSnapshot::Cancelled => RowStatus::Cancelled,
            })
            .unwrap_or(RowStatus::Pending),
        detail: snapshot
            .map(|goal| goal.objective.clone())
            .unwrap_or_default(),
    }
}

pub fn subagent_row(agent: &SubagentView) -> InspectorRow {
    InspectorRow {
        id: agent.child_id.to_string(),
        kind: InspectorRowKind::Subagent,
        title: agent.child_id.to_string(),
        status: match agent.status {
            tact_desktop_core::SubagentStatusSnapshot::Running => RowStatus::Running,
            tact_desktop_core::SubagentStatusSnapshot::Completed => RowStatus::Success,
            tact_desktop_core::SubagentStatusSnapshot::Failed => RowStatus::Failed,
            tact_desktop_core::SubagentStatusSnapshot::Cancelled => RowStatus::Cancelled,
        },
        detail: agent.summary.to_string(),
    }
}

pub fn task_row(task: &TaskView) -> InspectorRow {
    InspectorRow {
        id: task.id.to_string(),
        kind: InspectorRowKind::Task,
        title: task.subject.to_string(),
        status: match task.status.marker() {
            "[x]" => RowStatus::Success,
            "[>]" => RowStatus::Running,
            _ => RowStatus::Pending,
        },
        detail: format!(
            "{} → {}",
            format_ids(&task.blocks),
            format_ids(&task.blocked_by)
        ),
    }
}

fn format_ids(ids: &[u64]) -> String {
    if ids.is_empty() {
        "—".into()
    } else {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    use super::*;

    #[gpui::test]
    fn goals_and_subagents_use_the_shared_row_grammar(_cx: &mut TestAppContext) {
        let goal = goal_row(&GoalView::default());
        let subagent = subagent_row(&SubagentView::default());
        assert_eq!(goal.kind, InspectorRowKind::Goal);
        assert_eq!(subagent.kind, InspectorRowKind::Subagent);
        assert_eq!(goal.title, "Goal");
        assert!(goal.status != RowStatus::Failed);
    }
}
