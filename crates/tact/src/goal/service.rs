//! The Goal domain, as a service a command can reach.
//!
//! Two of the three Goal operations are store work with no agent involved:
//! `create` mints and persists, and `accept` records a human decision. Only one
//! branch of `accept` has to *run* something — a rejected acceptance continues the
//! Goal — and that is the host's job, not the service's, because running an
//! activation needs the agent.
//!
//! [`GoalActivator`] is that seam. The service decides *whether* an activation is
//! needed; the host, which owns the agent, knows *how*.

use std::sync::Arc;

use tact_protocol::AgentUpdate;
use tact_protocol::events::UpdateSink;

use crate::goal::lifecycle::{emit_goal_error, emit_goals_changed, goal_event};
use crate::goal::{
    AcceptancePolicy, GoalCreate, GoalCriterion, GoalEventKind, GoalEventSource,
    GoalEvidenceUpdate, GoalStatus, GoalUpdate, GoalVerification, GoalVerificationKind,
    SharedGoalManager,
};
use crate::task::SharedTaskManager;

use super::limits::GoalLimits;

/// Runs one Goal activation, or reports why it could not.
///
/// Implemented by the composition root, which holds the agent. Boxed future rather
/// than `async fn` so an implementor needs no `async_trait` — the only one lives in
/// a frontend crate that has no other reason to depend on it.
pub trait GoalActivator: Send + Sync {
    /// Run `goal_id` to completion.
    fn activate<'a>(
        &'a self,
        goal_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>>;
}

/// What a `/goal` verb needs from the runtime.
#[async_trait::async_trait]
pub trait GoalService: Send + Sync {
    /// Create a Goal and start its first run.
    ///
    /// Answers the new id, or `None` when creation was refused — the reason has
    /// already been reported, because a refusal here is usually a validation rule
    /// the user has to see in context.
    async fn create(
        &self,
        title: String,
        objective: String,
        criteria: Vec<String>,
    ) -> Option<String>;

    /// Record a human acceptance, continuing the Goal when the decision is not
    /// final.
    async fn accept(&self, goal_id: &str, text: &str);
}

/// The built-in service.
pub struct RuntimeGoalService {
    goal_manager: SharedGoalManager,
    task_manager: SharedTaskManager,
    limits: GoalLimits,
    events: UpdateSink,
    policy: Arc<dyn AcceptancePolicy>,
    activator: Arc<dyn GoalActivator>,
}

impl RuntimeGoalService {
    /// A service over the runtime's Goal state.
    pub fn new(
        goal_manager: SharedGoalManager,
        task_manager: SharedTaskManager,
        limits: GoalLimits,
        events: UpdateSink,
        policy: Arc<dyn AcceptancePolicy>,
        activator: Arc<dyn GoalActivator>,
    ) -> Self {
        Self {
            goal_manager,
            task_manager,
            limits,
            events,
            policy,
            activator,
        }
    }

    /// Re-project the panel after a change.
    async fn refresh(&self) {
        emit_goals_changed(
            &self.goal_manager,
            &self.task_manager,
            self.limits,
            &self.events,
        )
        .await;
    }
}

#[async_trait::async_trait]
impl GoalService for RuntimeGoalService {
    async fn create(
        &self,
        title: String,
        objective: String,
        criteria: Vec<String>,
    ) -> Option<String> {
        // Criteria are optional on purpose. Injecting a required human criterion
        // here used to make every Goal end in `waiting_user` + `/goal accept`,
        // which turned the user's acceptance into a rubber stamp for work they had
        // not verified. A Goal with no criteria finishes on the model's proposal
        // and the runner records that as self-attested; declare criteria (API
        // callers can) when the result really needs a gate.
        let criteria = criteria
            .into_iter()
            .map(|description| GoalCriterion::new(description, true, GoalVerificationKind::Human))
            .collect::<Vec<_>>();
        let goal = match self
            .goal_manager
            .create(GoalCreate {
                title,
                objective,
                criteria,
            })
            .await
        {
            Ok(goal) => goal,
            Err(error) => {
                emit_goal_error(&self.events, format!("Goal creation failed: {error:#}"));
                return None;
            }
        };
        let criteria_note = if goal.has_required_criteria() {
            ""
        } else {
            " - no acceptance criteria: completion will be self-attested"
        };
        self.refresh().await;
        self.events.send(AgentUpdate::Info(format!(
            "🎯 Goal {} created ({}) - starting run{criteria_note}",
            goal.id, goal.title,
        )));
        let id = goal.id.clone();
        // The activation is the host's, not ours: only it holds the agent.
        if let Err(error) = self.activator.activate(&id).await {
            emit_goal_error(
                &self.events,
                format!("Goal {id} activation failed: {error:#}"),
            );
        }
        Some(id)
    }

    async fn accept(&self, goal_id: &str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            emit_goal_error(
                &self.events,
                format!("Goal {goal_id} acceptance text must not be empty"),
            );
            return;
        }
        let goal = match self.goal_manager.get(goal_id).await {
            Ok(goal) => goal,
            Err(error) => {
                emit_goal_error(&self.events, format!("Goal {goal_id} not found: {error:#}"));
                return;
            }
        };
        // Acceptance is a human decision, not a race with the runner. A message
        // sent while the Goal was `waiting_user` can wake the runner and leave it
        // `running` after a no-op activation, which used to make the hinted
        // `/goal accept` fail. Accept the stranded `running` state too, but only
        // when no activation is actually in flight.
        let active_run = match self.goal_manager.list_running_runs().await {
            Ok(runs) => runs.iter().any(|run| run.goal_id == goal.id),
            Err(error) => {
                emit_goal_error(
                    &self.events,
                    format!("Goal {goal_id} acceptance failed: {error:#}"),
                );
                return;
            }
        };
        // `paused` is equally acceptable: recovery (`Running` with no active run)
        // parks a Goal there, and a paused Goal is still a human decision away from
        // done. Only an actual activation in flight blocks acceptance.
        let acceptable = matches!(goal.status, GoalStatus::WaitingUser | GoalStatus::Paused)
            || (goal.status == GoalStatus::Running && !active_run);
        if !acceptable {
            let suffix = if active_run {
                " (an activation is in flight)"
            } else {
                ""
            };
            emit_goal_error(
                &self.events,
                format!(
                    "Goal {goal_id} is {:?}{suffix}; acceptance requires waiting_user, paused, or an idle running Goal",
                    goal.status
                ),
            );
            return;
        }
        // Every pending required criterion is covered by this one decision.
        // Automatic criteria are reserved (creation is refused by
        // `GoalManager::create`), but legacy rows can still exist, so an explicit
        // human acceptance is the only supported way to close them.
        let evidence: Vec<_> = goal
            .criteria
            .iter()
            .filter(|criterion| {
                criterion.required && !criterion.has_valid_evidence(goal.criteria_version)
            })
            .map(|criterion| GoalEvidenceUpdate {
                criterion_id: criterion.id.clone(),
                value: text.to_string(),
            })
            .collect();
        if evidence.is_empty() {
            let hint = if goal.has_required_criteria() {
                "already accepted, or none pending"
            } else {
                "a Goal without acceptance criteria completes on its own"
            };
            emit_goal_error(
                &self.events,
                format!("Goal {goal_id} has no pending required criterion ({hint})"),
            );
            return;
        }
        let first = GoalUpdate {
            evidence,
            // `waiting_user` / `paused` re-enter `running`; a stranded `running`
            // Goal stays there (Running -> Running is not a transition).
            status: matches!(goal.status, GoalStatus::WaitingUser | GoalStatus::Paused)
                .then_some(GoalStatus::Running),
            ..GoalUpdate::default()
        };
        // Verify a candidate before the transaction so the update chain can be
        // computed up front: evidence + Running + Verifying + Succeeded must be
        // one durable commit, otherwise a failure between the last two leaves the
        // Goal stranded in `verifying`.
        let mut candidate = goal.clone();
        if let Err(error) = candidate.update(first.clone()) {
            emit_goal_error(
                &self.events,
                format!("Goal {goal_id} acceptance failed: {error:#}"),
            );
            return;
        }
        let verification = self.policy.verify(&candidate);
        let mut updates = vec![(
            first,
            goal_event(
                &goal,
                GoalEventKind::UserAccepted,
                Some(GoalStatus::Running),
                text.to_string(),
                GoalEventSource::User,
            ),
        )];
        if verification == GoalVerification::Verified {
            updates.push((
                GoalUpdate {
                    status: Some(GoalStatus::Verifying),
                    ..GoalUpdate::default()
                },
                goal_event(
                    &candidate,
                    GoalEventKind::StatusChanged,
                    Some(GoalStatus::Verifying),
                    "human acceptance verified".into(),
                    GoalEventSource::Runner,
                ),
            ));
            updates.push((
                GoalUpdate {
                    status: Some(GoalStatus::Succeeded),
                    ..GoalUpdate::default()
                },
                goal_event(
                    &candidate,
                    GoalEventKind::StatusChanged,
                    Some(GoalStatus::Succeeded),
                    "accepted by user".into(),
                    GoalEventSource::Runner,
                ),
            ));
        }
        if let Err(error) = self
            .goal_manager
            .update_chain(goal_id, goal.version, updates, None)
            .await
        {
            emit_goal_error(
                &self.events,
                format!("Goal {goal_id} acceptance failed: {error:#}"),
            );
            return;
        }

        match verification {
            GoalVerification::Verified => {
                // The acceptance is durable now; say so in the log. Without this
                // line the last visible message is still the "run /goal accept"
                // instruction, which reads as if acceptance were still pending.
                self.events.send(AgentUpdate::Info(format!(
                    "🎯 Goal {goal_id} accepted — succeeded"
                )));
            }
            GoalVerification::Rejected { .. } => {
                // The host clears this process's cancel flag before activating; the
                // service only decides that a continuation is needed.
                if let Err(error) = self.activator.activate(goal_id).await {
                    emit_goal_error(
                        &self.events,
                        format!("Goal {goal_id} continuation failed: {error:#}"),
                    );
                }
            }
            GoalVerification::NeedsUser { .. } => {
                emit_goal_error(
                    &self.events,
                    format!("Goal {goal_id} still needs additional human acceptance"),
                );
            }
        }
        self.refresh().await;
    }
}

/// An activator that records the request instead of running it.
///
/// Running an activation needs `&mut Agent`, and a command runs while its caller
/// already holds one — so an activator that tried to reach the agent here would
/// deadlock on its own lock. The request is queued instead, and the caller drains
/// the queue once the command has returned and the borrow is free.
pub struct QueueingActivator {
    queued: tokio::sync::mpsc::UnboundedSender<String>,
}

impl QueueingActivator {
    /// A queue draining into `queued`.
    pub fn new(queued: tokio::sync::mpsc::UnboundedSender<String>) -> Self {
        Self { queued }
    }
}

impl GoalActivator for QueueingActivator {
    fn activate<'a>(
        &'a self,
        goal_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>> + Send + 'a>> {
        Box::pin(async move {
            self.queued
                .send(goal_id.to_string())
                .map_err(|_| anyhow::anyhow!("the Goal activation queue is closed"))
        })
    }
}

/// The Goal service a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `GoalPlugin` was installed. A caller about to create or
/// accept a Goal must be told it has nowhere to record it rather than silently
/// dropping the decision.
pub fn service(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<Arc<dyn GoalService>> {
    services
        .get::<GoalServiceHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no goal service installed; install GoalPlugin"))
}

/// The service a [`crate::goal::GoalPlugin`] provided.
#[derive(Clone)]
pub struct GoalServiceHandle(pub Arc<dyn GoalService>);
