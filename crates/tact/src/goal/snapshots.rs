//! The Goal read-out: one projection per Goal, for the panel and the commands.
//!
//! This lived in `tact-ui` until the Goal verbs became things a command could
//! answer. It only ever needed the Goal and Task managers plus the run budget, so
//! keeping it in the frontend meant a command could not reach it without the
//! extension crate depending on the view — the wrong direction. It reads; it
//! never writes.

use std::collections::HashMap;

use anyhow::Result;
use tact_protocol::{
    GoalAcceptanceSnapshot, GoalCriterionSnapshot, GoalCriterionStatusSnapshot, GoalPhaseSnapshot,
    GoalRunBudgetSnapshot, GoalSnapshot, GoalStatusSnapshot,
};

use crate::goal::limits::GoalLimits;
use crate::goal::{
    GoalCriterionStatus, GoalEventKind, GoalPhase, GoalStatus, GoalVerificationKind,
};
use crate::task::TaskStatus;

/// Map one stored status onto its wire form.
///
/// One `match` per closed set, and the protocol adds no fallback arm: a new
/// status fails to compile here instead of reaching a frontend that has never
/// heard of it.
#[must_use]
pub fn goal_status_snapshot(status: GoalStatus) -> GoalStatusSnapshot {
    match status {
        GoalStatus::Draft => GoalStatusSnapshot::Draft,
        GoalStatus::Queued => GoalStatusSnapshot::Queued,
        GoalStatus::Running => GoalStatusSnapshot::Running,
        GoalStatus::WaitingUser => GoalStatusSnapshot::WaitingUser,
        GoalStatus::WaitingPermission => GoalStatusSnapshot::WaitingPermission,
        GoalStatus::Paused => GoalStatusSnapshot::Paused,
        GoalStatus::Verifying => GoalStatusSnapshot::Verifying,
        GoalStatus::Succeeded => GoalStatusSnapshot::Succeeded,
        GoalStatus::Failed => GoalStatusSnapshot::Failed,
        GoalStatus::Cancelled => GoalStatusSnapshot::Cancelled,
    }
}

/// Map one run phase onto its wire form.
#[must_use]
pub fn goal_phase_snapshot(phase: GoalPhase) -> GoalPhaseSnapshot {
    match phase {
        GoalPhase::Planning => GoalPhaseSnapshot::Planning,
        GoalPhase::Execution => GoalPhaseSnapshot::Execution,
        GoalPhase::Verification => GoalPhaseSnapshot::Verification,
    }
}

/// Map one criterion status onto its snapshot form.
///
/// A separate enum rather than reusing the stored one: the protocol's projection
/// is a wire type, so a new stored status must decide what it looks like on the
/// wire instead of silently gaining a variant the frontend does not know.
#[must_use]
pub fn criterion_status_snapshot(status: GoalCriterionStatus) -> GoalCriterionStatusSnapshot {
    match status {
        GoalCriterionStatus::Pending => GoalCriterionStatusSnapshot::Pending,
        GoalCriterionStatus::Passed => GoalCriterionStatusSnapshot::Passed,
        GoalCriterionStatus::Rejected => GoalCriterionStatusSnapshot::Rejected,
    }
}

pub async fn snapshots(
    goal_manager: &crate::goal::SharedGoalManager,
    task_manager: &crate::task::SharedTaskManager,
    limits: GoalLimits,
    include_archived: bool,
) -> Result<Vec<GoalSnapshot>> {
    let running_by_goal: HashMap<_, _> = goal_manager
        .list_running_runs()
        .await?
        .into_iter()
        .map(|run| (run.goal_id.clone(), run))
        .collect();
    let mut snapshots = Vec::new();
    for goal in goal_manager.list().await? {
        if goal.is_archived() && !include_archived {
            continue;
        }
        let tasks = task_manager.list_for_goal(&goal.id).await?;
        // The wait reason is only ever asked of a Goal that is *in* a waiting
        // or paused state, and the answer is one targeted query — so a Goal
        // that is running costs no event read at all, and a waiting one costs
        // one row instead of its whole history (2026-10-16).
        let wait_reason = if matches!(
            goal.status,
            GoalStatus::WaitingUser | GoalStatus::WaitingPermission | GoalStatus::Paused
        ) {
            let status_reason = goal_manager
                .latest_event_with_status(&goal.id, goal.status)
                .await?
                .and_then(|event| event.payload);
            if goal.status == GoalStatus::WaitingUser {
                // A `goal_request_input` question is more useful than the
                // generic "run ended as NeedsUser" status payload. Only use
                // it when it was asked after the latest Running transition;
                // an old question from a previous wait must not masquerade as
                // the reason the Goal is waiting for acceptance now.
                let question = goal_manager
                    .latest_event_of_kind(&goal.id, GoalEventKind::NeedsUser)
                    .await?;
                let last_running = goal_manager
                    .latest_event_with_status(&goal.id, GoalStatus::Running)
                    .await?;
                match (question, last_running) {
                    (Some(question), Some(running))
                        if question.occurred_at >= running.occurred_at =>
                    {
                        question.payload.or(status_reason)
                    }
                    (Some(question), None) => question.payload.or(status_reason),
                    _ => status_reason,
                }
            } else {
                status_reason
            }
        } else {
            None
        };
        let criteria: Vec<_> = goal
            .criteria
            .iter()
            .map(|criterion| GoalCriterionSnapshot {
                id: criterion.id.clone(),
                description: criterion.description.clone(),
                required: criterion.required,
                human: criterion.verification_kind == GoalVerificationKind::Human,
                status: criterion_status_snapshot(criterion.status),
                evidence_count: criterion.evidence.len(),
            })
            .collect();
        let required = goal.criteria.iter().filter(|criterion| criterion.required);
        let acceptance = GoalAcceptanceSnapshot {
            required: required.clone().count(),
            passed: required
                .clone()
                .filter(|criterion| criterion.status == GoalCriterionStatus::Passed)
                .count(),
            pending: required
                .clone()
                .filter(|criterion| criterion.status == GoalCriterionStatus::Pending)
                .count(),
            rejected: required
                .filter(|criterion| criterion.status == GoalCriterionStatus::Rejected)
                .count(),
        };
        let run_budget = running_by_goal
            .get(&goal.id)
            .map(|run| GoalRunBudgetSnapshot {
                turns_used: run.turns_used.min(u32::MAX as u64) as u32,
                turn_limit: limits.max_turns,
                tool_calls_used: run.tool_calls_used.min(u32::MAX as u64) as u32,
                elapsed_ms: run.elapsed_ms,
                wall_clock_limit_ms: limits.max_elapsed.as_millis().min(u64::MAX as u128) as u64,
            });
        let archived = goal.is_archived();
        snapshots.push(GoalSnapshot {
            id: goal.id.clone(),
            title: goal.title,
            objective: goal.objective,
            status: goal_status_snapshot(goal.status),
            phase: goal_phase_snapshot(goal.phase),
            criteria,
            task_total: tasks.len(),
            task_completed: tasks
                .iter()
                .filter(|task| task.status == TaskStatus::Completed)
                .count(),
            wait_reason,
            acceptance,
            run_budget,
            archived,
        });
    }
    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::goal::GoalLimits;
    use std::time::Duration;

    #[test]
    fn every_status_maps_to_its_wire_form() {
        // A new stored status must fail to compile here rather than reach a
        // frontend that has never heard of it.
        for status in [
            GoalStatus::Draft,
            GoalStatus::Queued,
            GoalStatus::Running,
            GoalStatus::WaitingUser,
            GoalStatus::WaitingPermission,
            GoalStatus::Paused,
            GoalStatus::Verifying,
            GoalStatus::Succeeded,
            GoalStatus::Failed,
            GoalStatus::Cancelled,
        ] {
            // The mapping is total: reaching this line for every variant is the
            // assertion, since the match has no fallback arm.
            let _ = goal_status_snapshot(status);
        }
    }

    #[test]
    fn every_phase_maps_to_its_wire_form() {
        for phase in [
            GoalPhase::Planning,
            GoalPhase::Execution,
            GoalPhase::Verification,
        ] {
            let _ = goal_phase_snapshot(phase);
        }
    }

    #[test]
    fn every_criterion_status_maps_to_its_wire_form() {
        assert_eq!(
            criterion_status_snapshot(GoalCriterionStatus::Pending),
            GoalCriterionStatusSnapshot::Pending
        );
        assert_eq!(
            criterion_status_snapshot(GoalCriterionStatus::Passed),
            GoalCriterionStatusSnapshot::Passed
        );
        assert_eq!(
            criterion_status_snapshot(GoalCriterionStatus::Rejected),
            GoalCriterionStatusSnapshot::Rejected
        );
    }

    #[test]
    fn a_zero_budget_permits_nothing() {
        // The projection and the runner read the same struct, so the rule has
        // one definition. `permit_activation` is where it lives.
        let limits = GoalLimits {
            max_turns: 0,
            max_elapsed: Duration::from_secs(0),
        };
        assert!(!limits.permit_activation(0, Duration::from_secs(0)));
    }

    #[test]
    fn a_run_inside_its_budget_is_permitted() {
        let limits = GoalLimits {
            max_turns: 10,
            max_elapsed: Duration::from_secs(60),
        };
        assert!(limits.permit_activation(0, Duration::from_secs(1)));
        assert!(limits.permit_activation(9, Duration::from_secs(59)));
    }

    #[test]
    fn a_run_at_its_turn_limit_is_not_permitted() {
        // The comparison is strict: the limit is the count a run may *use*, so
        // reaching it stops the run rather than allowing one more turn.
        let limits = GoalLimits {
            max_turns: 10,
            max_elapsed: Duration::from_secs(60),
        };
        assert!(!limits.permit_activation(10, Duration::from_secs(1)));
    }

    #[test]
    fn a_run_past_its_wall_clock_is_not_permitted() {
        let limits = GoalLimits {
            max_turns: 10,
            max_elapsed: Duration::from_secs(60),
        };
        assert!(!limits.permit_activation(0, Duration::from_secs(60)));
    }
}
