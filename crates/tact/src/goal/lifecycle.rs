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

/// File a finished Goal out of the panel, or bring it back.
///
/// Not a lifecycle change: `archived_at` is a stamp on the Goal, so a failed
/// Goal stays `failed` while it is off the list. Only a terminal Goal can be
/// archived — an archive that hid running work would take it off the panel
/// while it kept going — and repeating the verb reports the state instead of
/// erroring, the same rule `/skill disable` follows.
pub async fn set_archived(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    limits: GoalLimits,
    events: &UpdateSink,
    goal_id: &str,
    archived: bool,
) {
    let goal = match goal_manager.get(goal_id).await {
        Ok(goal) => goal,
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} not found: {error:#}"));
            return;
        }
    };
    if archived && !goal.status.is_terminal() {
        emit_goal_error(
            events,
            format!(
                "Goal {goal_id} is {:?}; only a finished Goal can be archived",
                goal.status
            ),
        );
        return;
    }
    if goal.is_archived() == archived {
        events.send(AgentUpdate::Info(format!(
            "Goal {goal_id} is {}",
            if archived {
                "already archived"
            } else {
                "not archived"
            }
        )));
        return;
    }
    let kind = if archived {
        GoalEventKind::Archived
    } else {
        GoalEventKind::Unarchived
    };
    let event = goal_event(
        &goal,
        kind,
        None,
        format!("{kind} by user"),
        // `/goal archive` is a user command; the event says who filed it.
        GoalEventSource::User,
    );
    match goal_manager
        .update(
            goal_id,
            goal.version,
            GoalUpdate {
                archived: Some(archived),
                ..GoalUpdate::default()
            },
            Some(event),
            None,
        )
        .await
    {
        Ok(updated) => {
            // The panel refresh goes first: the announcement names a Goal the
            // panel may no longer show.
            emit_goals_changed(goal_manager, task_manager, limits, events).await;
            events.send(AgentUpdate::Info(format!(
                "🎯 Goal {} {kind} ({})",
                updated.id, updated.title
            )));
        }
        Err(error) => {
            emit_goal_error(events, format!("Goal {goal_id} {kind} failed: {error:#}"));
        }
    }
}

/// Resolves and records `/goal steer [<id>] <text>`.
///
/// The TUI forwards an id only when it recognized one in its own snapshot; the
/// store is authoritative here, so an empty or lagging snapshot can neither
/// mis-target a Goal nor turn the instruction into a fake id. Without an
/// explicit id, a leading id/prefix in the text is accepted (the panel may not
/// have caught up), then the single running Goal, then the single non-terminal
/// Goal. An ambiguous target is an error, never a guess.
pub async fn steer(
    goal_manager: &SharedGoalManager,
    task_manager: &SharedTaskManager,
    events: &UpdateSink,
    limits: GoalLimits,
    requested: Option<&str>,
    text: &str,
) {
    let text = text.trim();
    if text.is_empty() {
        emit_goal_error(events, "Goal steering text must not be empty".to_string());
        return;
    }
    let goals = match goal_manager.list().await {
        Ok(goals) => goals,
        Err(error) => {
            emit_goal_error(events, format!("Goal steering failed: {error:#}"));
            return;
        }
    };

    let resolved = match requested {
        Some(prefix) => match unique_goal_matching(&goals, prefix) {
            Ok(Some(goal)) if !goal.status.is_terminal() => Ok((goal, text.to_string())),
            Ok(Some(goal)) => Err(format!(
                "Goal {} is terminal ({:?}); there is nothing left to steer",
                goal.id, goal.status
            )),
            Ok(None) => Err(format!("Goal {prefix} not found")),
            Err(count) => Err(format!(
                "Goal id prefix {prefix} matches {count} goals; type more characters"
            )),
        },
        None => {
            let mut parts = text.splitn(2, char::is_whitespace);
            let first = parts.next().unwrap_or_default();
            let rest = parts.next().unwrap_or_default().trim();
            match unique_goal_matching(&goals, first) {
                // An explicit id that the TUI could not resolve (empty or
                // stale snapshot) is still honored.
                Ok(Some(goal)) if !rest.is_empty() && !goal.status.is_terminal() => {
                    Ok((goal, rest.to_string()))
                }
                // A known id with nothing after it is a missing instruction,
                // not an instruction that happens to be the id itself.
                Ok(Some(_)) => Err("goal steering text must not be empty".to_string()),
                // An ambiguous first token is an id/prefix the user tried to
                // address, not ordinary instruction text.
                Err(count) => Err(format!(
                    "Goal id prefix {first} matches {count} goals; type more characters"
                )),
                Ok(None) => match implicit_steer_target(&goals) {
                    Ok(Some(goal)) => Ok((goal, text.to_string())),
                    Ok(None) => {
                        Err("no running Goal to steer; use /goal steer <goal-id> <text>"
                            .to_string())
                    }
                    Err(count) => Err(format!(
                        "{count} goals are open and none is uniquely running; use /goal steer <goal-id> <text>"
                    )),
                },
            }
        }
    };
    let (goal, instruction) = match resolved {
        Ok(pair) => pair,
        Err(message) => {
            emit_goal_error(events, message);
            return;
        }
    };

    match goal_manager
        .append_event(goal_event(
            goal,
            GoalEventKind::Steering,
            None,
            instruction,
            GoalEventSource::User,
        ))
        .await
    {
        Ok(_) => {
            emit_goals_changed(goal_manager, task_manager, limits, events).await;
        }
        Err(error) => emit_goal_error(
            events,
            format!("Goal {} steering failed: {error:#}", goal.id),
        ),
    }
}

/// Exact id or unique prefix; `Err(n)` means the prefix is ambiguous.
pub fn unique_goal_matching<'a>(
    goals: &'a [Goal],
    prefix: &str,
) -> Result<Option<&'a Goal>, usize> {
    let mut matches = goals
        .iter()
        .filter(|goal| goal.id == prefix || goal.id.starts_with(prefix));
    match (matches.next(), matches.next()) {
        (None, _) => Ok(None),
        (Some(goal), None) => Ok(Some(goal)),
        (Some(_), Some(_)) => {
            let count = goals
                .iter()
                .filter(|goal| goal.id == prefix || goal.id.starts_with(prefix))
                .count();
            Err(count)
        }
    }
}

/// The single running Goal, or the single non-terminal Goal when none is
/// running (a fresh activation can still be reported as `draft`/`queued`).
/// `Err(n)` means several candidates exist.
pub fn implicit_steer_target(goals: &[Goal]) -> Result<Option<&Goal>, usize> {
    let running: Vec<&Goal> = goals
        .iter()
        .filter(|goal| goal.status == GoalStatus::Running)
        .collect();
    match running.len() {
        1 => Ok(Some(running[0])),
        0 => {
            let open: Vec<&Goal> = goals
                .iter()
                .filter(|goal| !goal.status.is_terminal())
                .collect();
            match open.len() {
                0 => Ok(None),
                1 => Ok(Some(open[0])),
                count => Err(count),
            }
        }
        count => Err(count),
    }
}
