//! Goal lifecycle requests: pause and cancel.
//!
//! These live in the extension crate rather than the view because a *command* has
//! to be able to ask for them: `/goal pause <id>` is vocabulary a user types, and
//! the view should not be the only thing that can say it. The pieces they need —
//! the managers, the limits, the event sink and the shared cancel flag — are all
//! available here, so nothing had to be threaded back to the frontend.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use chrono::Utc;
use tact_protocol::AgentErrorKind;
use tact_protocol::AgentUpdate;
use tact_protocol::events::UpdateSink;

use crate::goal::limits::GoalLimits;
use crate::goal::snapshots::snapshots as goal_snapshots_with;
use crate::goal::{
    Goal, GoalEvent, GoalEventKind, GoalEventSource, GoalStatus, GoalUpdate, SharedGoalManager,
    new_event_id,
};
use crate::store::goal_store::GoalStoreError;
use crate::task::SharedTaskManager;

/// Whether `error` is "another process owns this Goal's run".
///
/// A caller cannot fix it by retrying, so the lifecycle paths treat it as "leave
/// the run alone" rather than as a failure to report.
fn is_active_run_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        matches!(
            cause.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::ActiveRunExists)
        )
    })
}

pub fn goal_event(
    goal: &Goal,
    kind: GoalEventKind,
    status: Option<GoalStatus>,
    payload: String,
    source: GoalEventSource,
) -> GoalEvent {
    GoalEvent {
        id: new_event_id(),
        goal_id: goal.id.clone(),
        run_id: None,
        kind,
        source,
        status,
        outcome: None,
        payload: Some(payload),
        occurred_at: Utc::now(),
    }
}

pub fn emit_goal_error(events: &UpdateSink, message: String) {
    events.send(AgentUpdate::Error(AgentErrorKind::Other(message)));
}

pub async fn emit_goals_changed(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
) {
    match goal_snapshots_with(goal_manager, task_manager, limits, false).await {
        Ok(goals) => {
            events.send(AgentUpdate::GoalsChanged { goals });
        }
        Err(error) => emit_goal_error(events, format!("Goal snapshot failed: {error:#}")),
    }
}

/// `/goal pause` — a run-local stop request, not a status write racing the run.
///
/// The request is a durable event first, so it works even while another
/// process owns the Agent. If this session owns the run, setting the shared
/// cancel flag makes the current model/tool wave stop early; the runner maps
/// that bare cancel to `paused` (not terminal `cancelled`).
pub async fn request_pause(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    // `interrupt` is this process's cancel flag, when it also owns the run. `None`
    // means "durable request only": the pause is recorded and the owner applies it.
    // A caller that knows it owns the run passes its flag so the loop notices at its
    // next check instead of at its next store read.
    interrupt: Option<&Arc<AtomicBool>>,
    goal_id: &str,
) {
    let goal = match goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} not found: {error:#}"));
            return;
        }
    };
    if goal.status.is_terminal() {
        emit_goal_error(
            events,
            format!(
                "Goal {goal_id} is already {:?}; nothing to pause",
                goal.status
            ),
        );
        return;
    }
    if goal.status == GoalStatus::Paused {
        events.send(AgentUpdate::Info(format!(
            "Goal {goal_id} is already paused"
        )));
        return;
    }

    let runs = match goal_manager.list_running_runs().await {
        Ok(runs) => runs,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} pause failed: {error:#}"));
            return;
        }
    };
    let target_active = runs.iter().any(|run| run.goal_id == goal.id);

    if target_active {
        if let Err(error) = goal_manager
            .append_event(goal_event(
                &goal,
                GoalEventKind::PauseRequested,
                None,
                "paused by user".into(),
                GoalEventSource::User,
            ))
            .await
        {
            emit_goal_error(
                events,
                format!("Goal {goal_id} pause request failed: {error:#}"),
            );
            return;
        }
        if let Some(flag) = interrupt {
            flag.store(true, Ordering::Relaxed);
        }
        events.send(AgentUpdate::Info(format!(
            "🎯 Goal {goal_id} pause requested; stopping the current run at the next safe point"
        )));
    } else {
        match goal_manager
            .update(
                &goal.id,
                goal.version,
                GoalUpdate {
                    status: Some(GoalStatus::Paused),
                    ..GoalUpdate::default()
                },
                Some(goal_event(
                    &goal,
                    GoalEventKind::StatusChanged,
                    Some(GoalStatus::Paused),
                    "paused by user".into(),
                    GoalEventSource::User,
                )),
                None,
            )
            .await
        {
            Ok(_) => {
                events.send(AgentUpdate::Info(format!("🎯 Goal {goal_id} paused")));
            }
            // A run started between the probe and the write: the request event
            // must be timestamped after that run started, so append a fresh one.
            Err(error) if is_active_run_error(&error) => {
                if let Err(append) = goal_manager
                    .append_event(goal_event(
                        &goal,
                        GoalEventKind::PauseRequested,
                        None,
                        "paused by user".into(),
                        GoalEventSource::User,
                    ))
                    .await
                {
                    emit_goal_error(
                        events,
                        format!("Goal {goal_id} pause request failed: {append:#}"),
                    );
                    return;
                }
                if let Some(flag) = interrupt {
                    flag.store(true, Ordering::Relaxed);
                }
                events.send(AgentUpdate::Info(format!(
                    "🎯 Goal {goal_id} pause requested; stopping the current run at the next safe point"
                )));
            }
            Err(error) => {
                emit_goal_error(events, format!("Goal {goal_id} pause failed: {error:#}"))
            }
        }
    }
    emit_goals_changed(goal_manager, task_manager, limits, events).await;
}

/// `/goal cancel` — an explicit terminal decision.
///
/// The durable `cancel_requested` event is what distinguishes this from a
/// bare Esc interrupt: the runner maps the former to `cancelled` and the
/// latter to `paused`.
pub async fn request_cancel(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    interrupt: Option<&Arc<AtomicBool>>,
    goal_id: &str,
) {
    let goal = match goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} not found: {error:#}"));
            return;
        }
    };
    if goal.status.is_terminal() {
        events.send(AgentUpdate::Info(format!(
            "Goal {goal_id} is already {:?}",
            goal.status
        )));
        return;
    }

    let runs = match goal_manager.list_running_runs().await {
        Ok(runs) => runs,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} cancel failed: {error:#}"));
            return;
        }
    };
    let target_active = runs.iter().any(|run| run.goal_id == goal.id);

    if target_active {
        if let Err(error) = goal_manager
            .append_event(goal_event(
                &goal,
                GoalEventKind::CancelRequested,
                None,
                "cancelled by user".into(),
                GoalEventSource::User,
            ))
            .await
        {
            emit_goal_error(
                events,
                format!("Goal {goal_id} cancel request failed: {error:#}"),
            );
            return;
        }
        if let Some(flag) = interrupt {
            flag.store(true, Ordering::Relaxed);
        }
        events.send(AgentUpdate::Info(format!(
            "🎯 Goal {goal_id} cancel requested; stopping the current run"
        )));
    } else {
        match goal_manager
            .update(
                &goal.id,
                goal.version,
                GoalUpdate {
                    status: Some(GoalStatus::Cancelled),
                    ..GoalUpdate::default()
                },
                Some(goal_event(
                    &goal,
                    GoalEventKind::StatusChanged,
                    Some(GoalStatus::Cancelled),
                    "cancelled by user".into(),
                    GoalEventSource::User,
                )),
                None,
            )
            .await
        {
            Ok(_) => {
                events.send(AgentUpdate::Info(format!("🎯 Goal {goal_id} cancelled")));
            }
            Err(error) if is_active_run_error(&error) => {
                if let Err(append) = goal_manager
                    .append_event(goal_event(
                        &goal,
                        GoalEventKind::CancelRequested,
                        None,
                        "cancelled by user".into(),
                        GoalEventSource::User,
                    ))
                    .await
                {
                    emit_goal_error(
                        events,
                        format!("Goal {goal_id} cancel request failed: {append:#}"),
                    );
                    return;
                }
                if let Some(flag) = interrupt {
                    flag.store(true, Ordering::Relaxed);
                }
                events.send(AgentUpdate::Info(format!(
                    "🎯 Goal {goal_id} cancel requested; stopping the current run"
                )));
            }
            Err(error) => {
                emit_goal_error(events, format!("Goal {goal_id} cancel failed: {error:#}"))
            }
        }
    }
    emit_goals_changed(goal_manager, task_manager, limits, events).await;
}
