//! Storage-independent domain model for supervised, long-lived Goals.
//!
//! A Goal is intentionally distinct from a Task, Run, Turn, or ephemeral plan
//! step. This module contains only validated data and pure lifecycle rules;
//! persistence, scheduling, permissions, and presentation belong elsewhere.

mod verifier;

pub use verifier::{GoalVerification, GoalVerifier};

use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identifier for a Goal. Goal IDs are opaque short-id strings.
pub type GoalId = String;

/// Characters in a Goal-domain identifier (see [`new_id`]).
pub const ID_LEN: usize = 8;

/// New identifier for the Goal domain: Goals, criteria, runs and events share
/// one shape, so no surface has to explain two widths.
///
/// A full UUID v4 is 36 characters, and these ids are read (and typed) on every
/// user-visible surface: `/goal show`, the creation notice, and the
/// `<goal-context>` injected into each activation — which repeats the Goal id
/// once plus every criterion id the model has to quote back through
/// `goal_propose_completion`. Eight hex characters (`git log` style) keep that
/// readable. Goals are workspace-scoped and few, so the 32-bit space is ample,
/// and a collision fails loudly on the store's primary key rather than silently
/// merging two Goals. Legacy rows keep their longer UUIDs: ids are opaque
/// strings everywhere and are never parsed or length-checked.
pub fn new_id() -> String {
    Uuid::new_v4().simple().to_string()[..ID_LEN].to_string()
}

/// Lifecycle state of a Goal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatus {
    #[default]
    Draft,
    Queued,
    Running,
    WaitingUser,
    /// Waiting on the permission system for a decision.
    ///
    /// Reserved: nothing writes it today. The permission gate decides on its own
    /// when no UI is attached (`ask_user` denies high-risk capabilities and
    /// allows the rest), and an interactive prompt is answered inside the
    /// activation, so a blocked activation never parks the Goal here. It stays in
    /// the state machine for a future permission-aware stop reason
    /// (see `book/28_chapter_goal_zh.md` §12).
    WaitingPermission,
    Paused,
    Verifying,
    Succeeded,
    Failed,
    Cancelled,
}

impl GoalStatus {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// Broad execution phase, kept independent from [`GoalStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalPhase {
    #[default]
    Planning,
    Execution,
    Verification,
}

/// Explicit result of one bounded Goal run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalRunOutcome {
    /// The run advanced the Goal and left something observable behind (an
    /// executed tool call or a new Goal event). Followed by another cycle while
    /// the run's budget lasts.
    #[default]
    ProgressMade,
    /// A Goal tool asked the Runner to hand control back to the user.
    NeedsUser,
    /// Reserved: nothing produces it today. The permission gate decides on its
    /// own when no UI is attached, so an activation is never parked on a
    /// permission question (see `GoalStatus::WaitingPermission`).
    NeedsPermission,
    /// The model proposed completion; verification decides what happens next.
    CompletionProposed,
    /// The run's cumulative budget (turns / tool calls / wall clock) is spent.
    BudgetExhausted,
    /// The run ended without leaving anything observable behind: the activation
    /// only produced text, so another cycle would re-read the same context. The
    /// Goal stays `Running` for the next activation.
    NoProgress,
    /// A user cancel request stopped the run.
    Cancelled,
    /// An activation error that may pass on a retry; the Goal keeps its status.
    TransientError,
    /// A hard failure that cannot be retried, such as a hook blocking the
    /// activation before its first model call.
    FatalError,
}

/// How a criterion is verified.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalVerificationKind {
    #[default]
    Automatic,
    Human,
}

/// Current state of a criterion's verification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalCriterionStatus {
    #[default]
    Pending,
    Passed,
    Rejected,
}

/// Evidence attached to a criterion, tagged with the criteria version that
/// made it meaningful.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalEvidence {
    pub value: String,
    pub criteria_version: u64,
}

/// One acceptance criterion for a Goal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalCriterion {
    pub id: String,
    pub description: String,
    #[serde(default = "default_required_criterion")]
    pub required: bool,
    #[serde(default)]
    pub verification_kind: GoalVerificationKind,
    #[serde(default)]
    pub status: GoalCriterionStatus,
    #[serde(default)]
    pub evidence: Vec<GoalEvidence>,
}

impl GoalCriterion {
    pub fn new(
        description: impl Into<String>,
        required: bool,
        verification_kind: GoalVerificationKind,
    ) -> Self {
        Self {
            id: new_id(),
            description: description.into(),
            required,
            verification_kind,
            status: GoalCriterionStatus::Pending,
            evidence: Vec::new(),
        }
    }

    pub fn required(description: impl Into<String>) -> Self {
        Self::new(description, true, GoalVerificationKind::Automatic)
    }

    pub fn required_with_id(id: impl Into<String>, description: impl Into<String>) -> Self {
        let mut criterion = Self::required(description);
        criterion.id = id.into();
        criterion
    }

    pub fn record_evidence(&mut self, value: impl Into<String>, criteria_version: u64) {
        self.evidence.push(GoalEvidence {
            value: value.into(),
            criteria_version,
        });
        self.status = GoalCriterionStatus::Passed;
    }

    pub fn has_valid_evidence(&self, criteria_version: u64) -> bool {
        self.status == GoalCriterionStatus::Passed
            && !self.evidence.is_empty()
            && self
                .evidence
                .iter()
                .all(|evidence| evidence.criteria_version == criteria_version)
    }

    fn validate(&self) -> Result<(), GoalError> {
        if self.description.trim().is_empty() {
            return Err(GoalError::EmptyCriterion {
                criterion_id: self.id.clone(),
            });
        }
        if self.id.trim().is_empty() {
            return Err(GoalError::EmptyCriterionId);
        }
        Ok(())
    }
}

/// Input used to create a Goal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalCreate {
    pub title: String,
    pub objective: String,
    pub constraints: Vec<String>,
    pub criteria: Vec<GoalCriterion>,
}

/// One explicit evidence write applied to the current criteria version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GoalEvidenceUpdate {
    pub criterion_id: String,
    pub value: String,
}

/// Mutable fields accepted by a pure Goal update.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GoalUpdate {
    pub title: Option<String>,
    pub objective: Option<String>,
    pub constraints: Option<Vec<String>>,
    pub criteria: Option<Vec<GoalCriterion>>,
    pub evidence: Vec<GoalEvidenceUpdate>,
    /// Optional lifecycle transition, applied through the same legal-transition
    /// table as [`Goal::transition_to`] so a status change can be persisted
    /// atomically with the rest of the update (and with a fencing lease, when
    /// the caller supplies one).
    pub status: Option<GoalStatus>,
}

/// A durable Goal aggregate. Timestamps are domain metadata; no storage is
/// required to validate or transition this value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
/// Compatibility policy: identity (`id`), human-facing title and objective,
/// and creation/update timestamps remain required because no safe domain
/// default exists for them. Lifecycle fields, versions, collections, and
/// optional completion metadata have explicit serde defaults below.
pub struct Goal {
    pub id: GoalId,
    pub title: String,
    pub objective: String,
    #[serde(default)]
    pub constraints: Vec<String>,
    #[serde(default)]
    pub status: GoalStatus,
    #[serde(default)]
    pub phase: GoalPhase,
    #[serde(default)]
    pub criteria: Vec<GoalCriterion>,
    #[serde(default = "default_criteria_version")]
    pub criteria_version: u64,
    #[serde(default = "default_version")]
    pub version: u64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub completed_at: Option<DateTime<Utc>>,
}

impl Goal {
    pub fn new(input: GoalCreate) -> Result<Self, GoalError> {
        validate_text(&input.title, GoalError::EmptyTitle)?;
        validate_text(&input.objective, GoalError::EmptyObjective)?;
        for criterion in &input.criteria {
            criterion.validate()?;
        }

        let now = Utc::now();
        Ok(Self {
            id: new_id(),
            title: input.title,
            objective: input.objective,
            constraints: input.constraints,
            status: GoalStatus::Draft,
            phase: GoalPhase::Planning,
            criteria: input.criteria,
            criteria_version: 1,
            version: 1,
            created_at: now,
            updated_at: now,
            completed_at: None,
        })
    }

    /// Apply a validated update. Changing criteria always advances the
    /// criteria version and discards evidence from the previous definition;
    /// [`GoalUpdate::status`] performs one legal lifecycle transition against
    /// the fields this call just applied (so `succeed()` still has to pass
    /// [`Goal::validate_acceptance`]).
    pub fn update(&mut self, update: GoalUpdate) -> Result<(), GoalError> {
        if self.status.is_terminal() {
            return Err(GoalError::TerminalGoalMutation {
                status: self.status,
            });
        }

        // Build and validate an isolated candidate first. This gives callers
        // strong exception safety: an invalid multi-field update cannot
        // partially change the aggregate.
        let mut candidate = self.clone();
        if let Some(title) = update.title {
            validate_text(&title, GoalError::EmptyTitle)?;
            candidate.title = title;
        }
        if let Some(objective) = update.objective {
            validate_text(&objective, GoalError::EmptyObjective)?;
            candidate.objective = objective;
        }
        if let Some(constraints) = update.constraints {
            candidate.constraints = constraints;
        }
        if let Some(criteria) = update.criteria {
            for criterion in &criteria {
                criterion.validate()?;
            }
            candidate.criteria_version = candidate.criteria_version.saturating_add(1);
            candidate.criteria = criteria
                .into_iter()
                .map(|mut criterion| {
                    criterion.evidence.clear();
                    criterion.status = GoalCriterionStatus::Pending;
                    criterion
                })
                .collect();
        }
        for evidence in update.evidence {
            validate_text(
                &evidence.value,
                GoalError::EmptyEvidence {
                    criterion_id: evidence.criterion_id.clone(),
                },
            )?;
            let criterion = candidate
                .criteria
                .iter_mut()
                .find(|criterion| criterion.id == evidence.criterion_id)
                .ok_or_else(|| GoalError::CriterionNotFound {
                    criterion_id: evidence.criterion_id.clone(),
                })?;
            criterion.record_evidence(evidence.value, candidate.criteria_version);
        }
        if let Some(next) = update.status {
            // The transition re-validates against the fields applied above and
            // carries this update's version bump.
            candidate.transition_to(next)?;
        } else {
            candidate.bump_version();
        }
        *self = candidate;
        Ok(())
    }

    /// Perform one legal lifecycle transition. On error, this value is not
    /// mutated.
    pub fn transition_to(&mut self, next: GoalStatus) -> Result<(), GoalError> {
        if !is_legal_transition(self.status, next) {
            return Err(GoalError::InvalidTransition {
                from: self.status,
                to: next,
            });
        }

        if next == GoalStatus::Succeeded {
            self.validate_acceptance()?;
        }

        self.status = next;
        self.phase = phase_for(next, self.phase);
        if next.is_terminal() {
            self.completed_at = Some(Utc::now());
        } else {
            self.completed_at = None;
        }
        self.bump_version();
        Ok(())
    }

    pub fn pause(&mut self) -> Result<(), GoalError> {
        self.transition_to(GoalStatus::Paused)
    }

    pub fn resume(&mut self) -> Result<(), GoalError> {
        match self.status {
            GoalStatus::Paused | GoalStatus::WaitingUser | GoalStatus::WaitingPermission => {
                self.transition_to(GoalStatus::Running)
            }
            _ => Err(GoalError::InvalidTransition {
                from: self.status,
                to: GoalStatus::Running,
            }),
        }
    }

    /// Success is only legal after entering verification; a model's message,
    /// a normal loop return, or completed Tasks cannot call this from running.
    pub fn succeed(&mut self) -> Result<(), GoalError> {
        self.transition_to(GoalStatus::Succeeded)
    }

    /// Validate the evidence recorded by the runner for the current criteria
    /// version. Automatic checks are performed by the runner; this method only
    /// trusts their recorded, versioned result.
    pub fn validate_acceptance(&self) -> Result<(), GoalError> {
        if self.criteria.is_empty() {
            return Err(GoalError::NoAcceptanceCriteria);
        }
        self.criteria
            .iter()
            .filter(|criterion| criterion.required)
            .find(|criterion| !criterion.has_valid_evidence(self.criteria_version))
            .map_or(Ok(()), |criterion| {
                Err(GoalError::AcceptanceCriteriaNotMet {
                    criterion_id: criterion.id.clone(),
                })
            })
    }

    fn bump_version(&mut self) {
        self.version = self.version.saturating_add(1);
        self.updated_at = Utc::now();
    }
}

fn default_required_criterion() -> bool {
    true
}

fn default_criteria_version() -> u64 {
    1
}

fn default_version() -> u64 {
    1
}

fn validate_text(text: &str, error: GoalError) -> Result<(), GoalError> {
    if text.trim().is_empty() {
        Err(error)
    } else {
        Ok(())
    }
}

fn is_legal_transition(from: GoalStatus, to: GoalStatus) -> bool {
    use GoalStatus::*;
    match from {
        Draft => matches!(to, Queued | Cancelled),
        Queued => matches!(to, Running | Paused | Cancelled),
        Running => matches!(
            to,
            WaitingUser | WaitingPermission | Paused | Verifying | Failed | Cancelled
        ),
        WaitingUser | WaitingPermission => matches!(to, Running | Paused | Cancelled),
        Paused => matches!(to, Running | Queued | Cancelled),
        Verifying => matches!(to, Succeeded | Running | WaitingUser | Failed | Cancelled),
        Succeeded | Failed | Cancelled => false,
    }
}

fn phase_for(status: GoalStatus, previous: GoalPhase) -> GoalPhase {
    match status {
        GoalStatus::Draft | GoalStatus::Queued => GoalPhase::Planning,
        GoalStatus::Running
        | GoalStatus::WaitingUser
        | GoalStatus::WaitingPermission
        | GoalStatus::Paused => GoalPhase::Execution,
        GoalStatus::Verifying | GoalStatus::Succeeded => GoalPhase::Verification,
        GoalStatus::Failed | GoalStatus::Cancelled => previous,
    }
}

/// A bounded execution lease's run record. It is deliberately independent of
/// the agent loop and persistence implementation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalRun {
    pub id: String,
    pub goal_id: GoalId,
    #[serde(default)]
    pub outcome: Option<GoalRunOutcome>,
    pub started_at: DateTime<Utc>,
    #[serde(default)]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub turns_used: u64,
    #[serde(default)]
    pub tool_calls_used: u64,
    #[serde(default)]
    pub elapsed_ms: u64,
    #[serde(default)]
    pub tokens_used: u64,
    #[serde(default)]
    pub checkpoint: Option<String>,
    #[serde(default)]
    pub stop_reason: Option<String>,
}

impl GoalRun {
    pub fn new(goal_id: GoalId) -> Self {
        Self {
            id: new_id(),
            goal_id,
            outcome: None,
            started_at: Utc::now(),
            finished_at: None,
            turns_used: 0,
            tool_calls_used: 0,
            elapsed_ms: 0,
            tokens_used: 0,
            checkpoint: None,
            stop_reason: None,
        }
    }

    pub fn finish(&mut self, outcome: GoalRunOutcome) {
        self.outcome = Some(outcome);
        self.finished_at = Some(Utc::now());
    }
}

/// An append-only domain event. The store decides how to serialize and order
/// it; constructing one does not perform I/O.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GoalEvent {
    pub id: String,
    pub goal_id: GoalId,
    #[serde(default)]
    pub run_id: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub status: Option<GoalStatus>,
    #[serde(default)]
    pub outcome: Option<GoalRunOutcome>,
    #[serde(default)]
    pub payload: Option<String>,
    pub occurred_at: DateTime<Utc>,
}

impl GoalEvent {
    pub fn run_finished(goal: &Goal, run: &GoalRun, outcome: GoalRunOutcome) -> Self {
        Self {
            id: new_id(),
            goal_id: goal.id.clone(),
            run_id: Some(run.id.clone()),
            kind: "run_finished".to_string(),
            status: Some(goal.status),
            outcome: Some(outcome),
            payload: None,
            occurred_at: Utc::now(),
        }
    }
}

/// Async facade over a reviewed [`GoalStore`].
///
/// `GoalManager` exposes durable Goal CRUD, append-only events, and bounded
/// lease/run lifecycle operations without exposing the storage implementation.
pub struct GoalManager {
    store: Box<dyn crate::store::goal_store::GoalStore>,
}

impl GoalManager {
    /// Opens a Goal manager backed by the SQLite database at `path`.
    pub async fn new(path: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self {
            store: Box::new(crate::store::goal_store::SqliteGoalStore::new(path).await?),
        })
    }
    /// Creates a manager from an already configured Goal store.
    pub fn from_store(store: Box<dyn crate::store::goal_store::GoalStore>) -> Self {
        Self { store }
    }
    /// Creates and persists a validated Goal.
    pub async fn create(&self, input: GoalCreate) -> anyhow::Result<Goal> {
        self.store
            .create(Goal::new(input).map_err(anyhow::Error::new)?)
            .await
    }
    /// Loads a Goal by stable ID.
    pub async fn get(&self, id: &str) -> anyhow::Result<Goal> {
        self.store.get(id).await
    }
    /// Lists all persisted Goals in store order.
    pub async fn list(&self) -> anyhow::Result<Vec<Goal>> {
        self.store.list().await
    }
    /// Applies a compare-and-swap Goal update, optionally recording an event.
    pub async fn update(
        &self,
        id: &str,
        expected_version: u64,
        update: GoalUpdate,
        event: Option<GoalEvent>,
        lease: Option<&crate::store::goal_store::LeaseToken>,
    ) -> anyhow::Result<Goal> {
        self.store
            .update(id, expected_version, update, event, lease)
            .await
    }
    /// Appends a Goal event without changing the aggregate.
    pub async fn append_event(&self, event: GoalEvent) -> anyhow::Result<GoalEvent> {
        self.store.append_event(event).await
    }
    /// Lists append-only events for a Goal in occurrence order.
    pub async fn list_events(&self, id: &str) -> anyhow::Result<Vec<GoalEvent>> {
        self.store.list_events(id).await
    }
    /// Claims the singleton execution lease for a bounded TTL.
    pub async fn claim_lease(
        &self,
        owner: &str,
        ttl: std::time::Duration,
    ) -> anyhow::Result<crate::store::goal_store::LeaseToken> {
        self.store.claim_lease(owner, ttl).await
    }
    /// Extends an owned execution lease.
    pub async fn heartbeat_lease(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        ttl: std::time::Duration,
    ) -> anyhow::Result<crate::store::goal_store::LeaseToken> {
        self.store.heartbeat_lease(token, ttl).await
    }
    /// Releases an owned execution lease.
    pub async fn release_lease(
        &self,
        token: &crate::store::goal_store::LeaseToken,
    ) -> anyhow::Result<()> {
        self.store.release_lease(token).await
    }
    /// Starts one active bounded Goal run under the execution lease.
    pub async fn start_run(
        &self,
        owner: &str,
        ttl: std::time::Duration,
        run: GoalRun,
    ) -> anyhow::Result<(crate::store::goal_store::LeaseToken, GoalRun)> {
        self.store.start_run(owner, ttl, run).await
    }
    /// Finishes an active Goal run under the execution lease.
    pub async fn finish_run(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        run: GoalRun,
    ) -> anyhow::Result<GoalRun> {
        self.store.finish_run(token, run).await
    }
    /// Finishes an active Goal run and, in one transaction, applies its status
    /// transitions and records the run-finished event.
    ///
    /// This is the runner's normal exit: the run row, the Goal status and the
    /// run's history entry can never diverge, however the process dies.
    pub async fn finish_run_and_transition(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        run: GoalRun,
        transitions: Vec<(GoalUpdate, GoalEvent)>,
        run_event: GoalEvent,
    ) -> anyhow::Result<(GoalRun, Goal)> {
        self.store
            .finish_run_and_transition(token, run, transitions, run_event)
            .await
    }
    /// Lists runs that are currently marked active.
    pub async fn list_running_runs(&self) -> anyhow::Result<Vec<GoalRun>> {
        self.store.list_running_runs().await
    }
    /// Interrupts active runs when their singleton lease has expired.
    pub async fn recover_expired_active_runs(&self) -> anyhow::Result<Vec<GoalRun>> {
        self.store.recover_expired_active_runs().await
    }
}

/// Cloneable, thread-safe facade over [`GoalManager`].
///
/// The underlying SQLite connection pool coordinates concurrent access, so the
/// shared wrapper does not require an additional mutex.
#[derive(Clone)]
pub struct SharedGoalManager {
    inner: std::sync::Arc<GoalManager>,
}

impl SharedGoalManager {
    /// Wraps a Goal manager for cheap cloning across concurrent callers.
    pub fn new(manager: GoalManager) -> Self {
        Self {
            inner: std::sync::Arc::new(manager),
        }
    }
    /// Creates and persists a validated Goal.
    pub async fn create(&self, input: GoalCreate) -> anyhow::Result<Goal> {
        self.inner.create(input).await
    }
    /// Loads a Goal by stable ID.
    pub async fn get(&self, id: &str) -> anyhow::Result<Goal> {
        self.inner.get(id).await
    }
    /// Lists all persisted Goals in store order.
    pub async fn list(&self) -> anyhow::Result<Vec<Goal>> {
        self.inner.list().await
    }
    /// Applies a compare-and-swap Goal update, optionally recording an event.
    pub async fn update(
        &self,
        id: &str,
        expected_version: u64,
        update: GoalUpdate,
        event: Option<GoalEvent>,
        lease: Option<&crate::store::goal_store::LeaseToken>,
    ) -> anyhow::Result<Goal> {
        self.inner
            .update(id, expected_version, update, event, lease)
            .await
    }
    /// Lists append-only events for a Goal in occurrence order.
    pub async fn list_events(&self, id: &str) -> anyhow::Result<Vec<GoalEvent>> {
        self.inner.list_events(id).await
    }
    /// Appends a Goal event without changing the aggregate.
    pub async fn append_event(&self, event: GoalEvent) -> anyhow::Result<GoalEvent> {
        self.inner.append_event(event).await
    }
    /// Claims the singleton execution lease for a bounded TTL.
    pub async fn claim_lease(
        &self,
        owner: &str,
        ttl: std::time::Duration,
    ) -> anyhow::Result<crate::store::goal_store::LeaseToken> {
        self.inner.claim_lease(owner, ttl).await
    }
    /// Extends an owned execution lease.
    pub async fn heartbeat_lease(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        ttl: std::time::Duration,
    ) -> anyhow::Result<crate::store::goal_store::LeaseToken> {
        self.inner.heartbeat_lease(token, ttl).await
    }
    /// Releases an owned execution lease.
    pub async fn release_lease(
        &self,
        token: &crate::store::goal_store::LeaseToken,
    ) -> anyhow::Result<()> {
        self.inner.release_lease(token).await
    }
    /// Starts one active bounded Goal run under the execution lease.
    pub async fn start_run(
        &self,
        owner: &str,
        ttl: std::time::Duration,
        run: GoalRun,
    ) -> anyhow::Result<(crate::store::goal_store::LeaseToken, GoalRun)> {
        self.inner.start_run(owner, ttl, run).await
    }
    /// Finishes an active Goal run under the execution lease.
    pub async fn finish_run(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        run: GoalRun,
    ) -> anyhow::Result<GoalRun> {
        self.inner.finish_run(token, run).await
    }
    /// Finishes an active Goal run and, in one transaction, applies its status
    /// transitions and records the run-finished event.
    pub async fn finish_run_and_transition(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        run: GoalRun,
        transitions: Vec<(GoalUpdate, GoalEvent)>,
        run_event: GoalEvent,
    ) -> anyhow::Result<(GoalRun, Goal)> {
        self.inner
            .finish_run_and_transition(token, run, transitions, run_event)
            .await
    }
    /// Lists runs that are currently marked active.
    pub async fn list_running_runs(&self) -> anyhow::Result<Vec<GoalRun>> {
        self.inner.list_running_runs().await
    }
    /// Interrupts active runs when their singleton lease has expired.
    pub async fn recover_expired_active_runs(&self) -> anyhow::Result<Vec<GoalRun>> {
        self.inner.recover_expired_active_runs().await
    }
}

/// Errors returned by pure Goal validation and transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalError {
    EmptyTitle,
    EmptyObjective,
    EmptyCriterionId,
    EmptyCriterion { criterion_id: String },
    EmptyEvidence { criterion_id: String },
    CriterionNotFound { criterion_id: String },
    InvalidTransition { from: GoalStatus, to: GoalStatus },
    TerminalGoalMutation { status: GoalStatus },
    NoAcceptanceCriteria,
    AcceptanceCriteriaNotMet { criterion_id: String },
}

impl fmt::Display for GoalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTitle => write!(f, "goal title must not be empty"),
            Self::EmptyObjective => write!(f, "goal objective must not be empty"),
            Self::EmptyCriterionId => write!(f, "goal criterion id must not be empty"),
            Self::EmptyCriterion { criterion_id } => {
                write!(f, "goal criterion {criterion_id} must not be empty")
            }
            Self::EmptyEvidence { criterion_id } => {
                write!(
                    f,
                    "goal criterion {criterion_id} evidence must not be empty"
                )
            }
            Self::CriterionNotFound { criterion_id } => {
                write!(f, "goal criterion {criterion_id} was not found")
            }
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid goal transition from {from:?} to {to:?}")
            }
            Self::TerminalGoalMutation { status } => {
                write!(f, "cannot mutate terminal goal in {status:?} state")
            }
            Self::NoAcceptanceCriteria => {
                write!(f, "goal must have acceptance criteria to succeed")
            }
            Self::AcceptanceCriteriaNotMet { criterion_id } => {
                write!(
                    f,
                    "required criterion {criterion_id} lacks valid current-version evidence"
                )
            }
        }
    }
}

impl std::error::Error for GoalError {}

#[cfg(test)]
mod task3_manager_tests {
    use super::*;
    use crate::store::goal_store::GoalStoreError;
    use std::time::Duration;

    fn temp_db(name: &str) -> std::path::PathBuf {
        {
            let dir = std::env::temp_dir().join(format!("tact-goal-manager-test-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            dir.join("tact.db")
        }
    }

    fn input(title: &str) -> GoalCreate {
        GoalCreate {
            title: title.into(),
            objective: format!("objective for {title}"),
            constraints: vec![],
            criteria: vec![GoalCriterion::required("done")],
        }
    }

    #[tokio::test]
    async fn manager_delegates_crud_events_and_run_lease_operations() {
        let manager = GoalManager::new(&temp_db("facade")).await.unwrap();
        let created = manager.create(input("first")).await.unwrap();
        assert_eq!(manager.get(&created.id).await.unwrap(), created);
        assert_eq!(manager.list().await.unwrap(), vec![created.clone()]);

        let event = GoalEvent {
            id: "manager-event".into(),
            goal_id: created.id.clone(),
            run_id: None,
            kind: "created".into(),
            status: None,
            outcome: None,
            payload: None,
            occurred_at: chrono::Utc::now(),
        };
        let updated = manager
            .update(
                &created.id,
                created.version,
                GoalUpdate {
                    title: Some("changed".into()),
                    ..Default::default()
                },
                Some(event.clone()),
                None,
            )
            .await
            .unwrap();
        assert_eq!(updated.title, "changed");
        assert_eq!(manager.list_events(&created.id).await.unwrap(), vec![event]);

        let conflict = manager
            .update(
                &created.id,
                created.version,
                GoalUpdate {
                    title: Some("rejected".into()),
                    ..Default::default()
                },
                None,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            conflict.downcast_ref::<GoalStoreError>(),
            Some(GoalStoreError::VersionConflict { .. })
        ));

        let token = manager
            .claim_lease("manager-test", Duration::from_secs(60))
            .await
            .unwrap();
        let goal = manager.get(&created.id).await.unwrap();
        let run = GoalRun::new(goal.id.clone());
        let (same_token, running) = manager
            .start_run("manager-test", Duration::from_secs(60), run)
            .await
            .unwrap();
        assert_eq!(same_token, token);
        assert_eq!(
            manager.list_running_runs().await.unwrap(),
            vec![running.clone()]
        );
        let mut finished = running;
        finished.finish(GoalRunOutcome::ProgressMade);
        assert_eq!(
            manager.finish_run(&token, finished.clone()).await.unwrap(),
            finished
        );
        manager.release_lease(&token).await.unwrap();
    }

    #[tokio::test]
    async fn shared_manager_delegates_complete_lease_and_run_lifecycle() {
        let shared = SharedGoalManager::new(
            GoalManager::new(&temp_db("shared-lifecycle"))
                .await
                .unwrap(),
        );
        let goal = shared.create(input("shared")).await.unwrap();
        let token = shared
            .claim_lease("shared-test", Duration::from_secs(60))
            .await
            .unwrap();
        let heartbeated = shared
            .heartbeat_lease(&token, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(heartbeated, token);

        let (run_token, mut run) = shared
            .start_run(
                "shared-test",
                Duration::from_secs(60),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        assert_eq!(run_token, token);
        assert_eq!(shared.list_running_runs().await.unwrap(), vec![run.clone()]);
        assert!(shared.list_events(&goal.id).await.unwrap().is_empty());
        run.finish(GoalRunOutcome::ProgressMade);
        assert_eq!(shared.finish_run(&token, run.clone()).await.unwrap(), run);
        assert!(shared.list_running_runs().await.unwrap().is_empty());

        let (_, active_run) = shared
            .start_run(
                "shared-test",
                Duration::from_millis(25),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let recovered = shared.recover_expired_active_runs().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, active_run.id);
        assert_eq!(recovered[0].stop_reason.as_deref(), Some("lease expired"));
        assert!(shared.list_running_runs().await.unwrap().is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_goal() -> Goal {
        Goal::new(GoalCreate {
            title: "Ship the release".to_string(),
            objective: "Publish the approved release to production".to_string(),
            constraints: vec!["Do not change the public API".to_string()],
            criteria: vec![GoalCriterion::required("release is published")],
        })
        .expect("valid goal")
    }

    /// Every id the Goal domain mints is short enough to read and type: the
    /// user copies the Goal id between commands and the model quotes criterion
    /// ids back through `goal_propose_completion`.
    #[test]
    fn ids_are_short_hex_identifiers() {
        let goal = create_goal();
        let run = GoalRun::new(goal.id.clone());
        let event = GoalEvent::run_finished(&goal, &run, GoalRunOutcome::ProgressMade);
        for id in [&goal.id, &goal.criteria[0].id, &run.id, &event.id] {
            assert_eq!(id.len(), ID_LEN, "{id} must be {ID_LEN} characters");
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit()),
                "{id} must be hex"
            );
        }
        assert_ne!(new_id(), new_id(), "ids must not repeat");
    }

    #[test]
    fn goal_follows_the_normal_execution_lifecycle() {
        let mut goal = create_goal();
        assert_eq!(goal.status, GoalStatus::Draft);
        assert_eq!(goal.phase, GoalPhase::Planning);

        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::Verifying).unwrap();
        goal.criteria[0].record_evidence("release-123", goal.criteria_version);
        goal.succeed().unwrap();
        assert_eq!(goal.status, GoalStatus::Succeeded);
        assert_eq!(goal.phase, GoalPhase::Verification);
    }

    #[test]
    fn goal_can_wait_pause_and_resume_without_becoming_terminal() {
        let mut goal = create_goal();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();

        goal.transition_to(GoalStatus::WaitingUser).unwrap();
        goal.resume().unwrap();
        assert_eq!(goal.status, GoalStatus::Running);

        goal.pause().unwrap();
        assert_eq!(goal.status, GoalStatus::Paused);
        goal.resume().unwrap();
        assert_eq!(goal.status, GoalStatus::Running);
    }

    #[test]
    fn terminal_goals_reject_transitions_without_mutation() {
        let mut goal = create_goal();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::Cancelled).unwrap();

        let error = goal.transition_to(GoalStatus::Running).unwrap_err();
        assert!(matches!(error, GoalError::InvalidTransition { .. }));
        assert_eq!(goal.status, GoalStatus::Cancelled);
    }

    #[test]
    fn succeeded_and_failed_goals_reject_transitions_without_mutation() {
        let mut succeeded = create_goal();
        succeeded.transition_to(GoalStatus::Queued).unwrap();
        succeeded.transition_to(GoalStatus::Running).unwrap();
        succeeded.transition_to(GoalStatus::Verifying).unwrap();
        succeeded.criteria[0].record_evidence("ok", succeeded.criteria_version);
        succeeded.succeed().unwrap();
        let succeeded_before = succeeded.clone();
        assert!(succeeded.transition_to(GoalStatus::Running).is_err());
        assert_eq!(succeeded, succeeded_before);

        let mut failed = create_goal();
        failed.transition_to(GoalStatus::Queued).unwrap();
        failed.transition_to(GoalStatus::Running).unwrap();
        failed.transition_to(GoalStatus::Failed).unwrap();
        let failed_before = failed.clone();
        assert!(failed.transition_to(GoalStatus::Queued).is_err());
        assert_eq!(failed, failed_before);
    }

    #[test]
    fn title_and_objective_must_not_be_empty() {
        let title_error = Goal::new(GoalCreate {
            title: "  ".to_string(),
            objective: "An objective".to_string(),
            constraints: Vec::new(),
            criteria: vec![GoalCriterion::required("done")],
        })
        .unwrap_err();
        assert!(matches!(title_error, GoalError::EmptyTitle));

        let objective_error = Goal::new(GoalCreate {
            title: "A title".to_string(),
            objective: "\n\t".to_string(),
            constraints: Vec::new(),
            criteria: vec![GoalCriterion::required("done")],
        })
        .unwrap_err();
        assert!(matches!(objective_error, GoalError::EmptyObjective));
    }

    #[test]
    fn updating_criteria_bumps_version_and_invalidates_old_evidence() {
        let mut goal = create_goal();
        let criterion_id = goal.criteria[0].id.clone();
        goal.criteria[0].record_evidence("release-123", goal.criteria_version);
        assert!(goal.criteria[0].has_valid_evidence(goal.criteria_version));

        let old_version = goal.criteria_version;
        goal.update(GoalUpdate {
            title: None,
            objective: None,
            constraints: None,
            criteria: Some(vec![GoalCriterion::required_with_id(
                criterion_id,
                "release is published and smoke-tested",
            )]),
            evidence: Vec::new(),
            status: None,
        })
        .unwrap();

        assert_eq!(goal.criteria_version, old_version + 1);
        assert!(!goal.criteria[0].has_valid_evidence(goal.criteria_version));
        assert!(goal.criteria[0].evidence.is_empty());
    }

    #[test]
    fn invalid_multi_field_update_is_atomic() {
        let mut goal = create_goal();
        let before = goal.clone();

        let error = goal
            .update(GoalUpdate {
                title: Some("A new title".to_string()),
                objective: Some("A new objective".to_string()),
                constraints: Some(vec!["new constraint".to_string()]),
                criteria: Some(vec![GoalCriterion::required("  ")]),
                evidence: Vec::new(),
                status: None,
            })
            .unwrap_err();

        assert!(matches!(error, GoalError::EmptyCriterion { .. }));
        assert_eq!(goal, before);
    }

    #[test]
    fn terminal_goals_reject_updates() {
        for terminal in [
            GoalStatus::Succeeded,
            GoalStatus::Failed,
            GoalStatus::Cancelled,
        ] {
            let mut goal = create_goal();
            if terminal == GoalStatus::Succeeded {
                goal.transition_to(GoalStatus::Queued).unwrap();
                goal.transition_to(GoalStatus::Running).unwrap();
                goal.transition_to(GoalStatus::Verifying).unwrap();
                goal.criteria[0].record_evidence("ok", goal.criteria_version);
                goal.succeed().unwrap();
            } else if terminal == GoalStatus::Failed {
                goal.transition_to(GoalStatus::Queued).unwrap();
                goal.transition_to(GoalStatus::Running).unwrap();
                goal.transition_to(GoalStatus::Failed).unwrap();
            } else {
                goal.transition_to(GoalStatus::Cancelled).unwrap();
            }
            let before = goal.clone();

            let error = goal
                .update(GoalUpdate {
                    title: Some("attempted mutation".to_string()),
                    ..GoalUpdate::default()
                })
                .unwrap_err();

            assert!(
                matches!(error, GoalError::TerminalGoalMutation { status } if status == terminal)
            );
            assert_eq!(goal, before);
        }
    }

    #[test]
    fn success_requires_current_version_evidence_for_every_required_criterion() {
        let mut goal = Goal::new(GoalCreate {
            title: "Ship it".to_string(),
            objective: "Publish it".to_string(),
            constraints: Vec::new(),
            criteria: vec![
                GoalCriterion::required("first check"),
                GoalCriterion::required("second check"),
            ],
        })
        .unwrap();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::Verifying).unwrap();

        let missing = goal.succeed().unwrap_err();
        assert!(matches!(
            missing,
            GoalError::AcceptanceCriteriaNotMet { .. }
        ));
        goal.criteria[0].record_evidence("ok", goal.criteria_version);
        assert!(goal.succeed().is_err());

        goal.criteria[1].record_evidence("stale", goal.criteria_version - 1);
        assert!(goal.succeed().is_err());

        goal.criteria[1].status = GoalCriterionStatus::Rejected;
        goal.criteria[1].evidence[0].criteria_version = goal.criteria_version;
        assert!(goal.succeed().is_err());

        goal.criteria[1].status = GoalCriterionStatus::Passed;
        goal.succeed().unwrap();
        assert_eq!(goal.status, GoalStatus::Succeeded);
    }

    #[test]
    fn success_rejects_empty_acceptance_criteria() {
        let mut goal = Goal::new(GoalCreate {
            title: "Ship it".to_string(),
            objective: "Publish it".to_string(),
            constraints: Vec::new(),
            criteria: Vec::new(),
        })
        .unwrap();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::Verifying).unwrap();

        assert_eq!(goal.succeed().unwrap_err(), GoalError::NoAcceptanceCriteria);
    }

    #[test]
    fn serde_round_trip_and_legacy_defaults_are_supported() {
        let goal = create_goal();
        let encoded = serde_json::to_string(&goal).unwrap();
        let decoded: Goal = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, goal);

        let legacy = serde_json::json!({
            "id": "legacy-goal",
            "title": "Legacy goal",
            "objective": "Keep working",
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z"
        });
        let decoded: Goal = serde_json::from_value(legacy).unwrap();
        assert_eq!(decoded.status, GoalStatus::Draft);
        assert_eq!(decoded.phase, GoalPhase::Planning);
        assert_eq!(decoded.criteria_version, 1);
        assert_eq!(decoded.version, 1);
        assert!(decoded.criteria.is_empty());
        assert!(decoded.constraints.is_empty());
        assert_eq!(decoded.completed_at, None);

        let criterion: GoalCriterion = serde_json::from_value(serde_json::json!({
            "id": "legacy-criterion",
            "description": "legacy check"
        }))
        .unwrap();
        assert!(criterion.required);
        assert_eq!(criterion.verification_kind, GoalVerificationKind::Automatic);
        assert_eq!(criterion.status, GoalCriterionStatus::Pending);
        assert!(criterion.evidence.is_empty());

        let run: GoalRun = serde_json::from_value(serde_json::json!({
            "id": "run-1",
            "goal_id": "legacy-goal",
            "started_at": "2026-01-01T00:00:00Z"
        }))
        .unwrap();
        assert_eq!(run.outcome, None);
        assert_eq!(run.turns_used, 0);
        assert_eq!(run.tool_calls_used, 0);
        assert_eq!(run.elapsed_ms, 0);
        assert_eq!(run.tokens_used, 0);
        assert_eq!(run.checkpoint, None);
        assert_eq!(run.stop_reason, None);
    }

    #[test]
    fn run_outcomes_are_explicit_and_events_are_storage_independent() {
        let goal = create_goal();
        let run = GoalRun::new(goal.id.clone());
        assert_eq!(run.outcome, None);

        let event = GoalEvent::run_finished(&goal, &run, GoalRunOutcome::ProgressMade);
        assert_eq!(event.goal_id, goal.id);
        assert_eq!(event.run_id, Some(run.id));
    }

    #[test]
    fn evidence_update_records_current_version_and_can_resume_waiting_goal() {
        let mut goal = Goal::new(GoalCreate {
            title: "Review release".into(),
            objective: "Wait for explicit acceptance".into(),
            constraints: Vec::new(),
            criteria: vec![GoalCriterion::new(
                "User accepts the result",
                true,
                GoalVerificationKind::Human,
            )],
        })
        .unwrap();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::WaitingUser).unwrap();

        goal.update(GoalUpdate {
            evidence: vec![GoalEvidenceUpdate {
                criterion_id: goal.criteria[0].id.clone(),
                value: "Accepted after manual review".into(),
            }],
            status: Some(GoalStatus::Running),
            ..GoalUpdate::default()
        })
        .unwrap();

        assert_eq!(goal.status, GoalStatus::Running);
        assert!(goal.criteria[0].has_valid_evidence(goal.criteria_version));
        assert_eq!(
            goal.criteria[0].evidence[0].value,
            "Accepted after manual review"
        );
    }
}
