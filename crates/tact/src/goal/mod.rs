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

/// Opening tag of the context the runner injects at the start of an
/// activation (see `book/28_chapter_goal_zh.md` §5).
///
/// The tag is a *contract* between two crates: the runner frames the context
/// with it, and every reader that must tell injected context from a user turn
/// (compaction rebuild, TUI history load) recognizes it. Like the hook-context
/// markers it is the only signal that survives a reload, because
/// `MessageKind` is never serialized.
pub const GOAL_CONTEXT_OPEN_TAG: &str = "<goal-context>";
/// Closing tag of the injected context (see [`GOAL_CONTEXT_OPEN_TAG`]).
pub const GOAL_CONTEXT_CLOSE_TAG: &str = "</goal-context>";

/// True when `text` is the runner's injected Goal context rather than a user
/// turn.
///
/// Detection is by marker, not by kind, for the same reason as
/// `hook::is_hook_context_text`: a session reloaded from disk carries no kind.
/// Leading whitespace is tolerated because a reload may have trimmed or
/// re-indented the cell.
pub fn is_goal_context_text(text: &str) -> bool {
    text.trim_start().starts_with(GOAL_CONTEXT_OPEN_TAG)
}

/// New user-facing identifier: Goals and criteria share one width, so no surface
/// has to explain two shapes. Runs and events use [`new_event_id`].
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

/// Identifier for high-volume rows (runs and events).
///
/// Eight hex characters are fine for the handful of Goals a workspace holds,
/// but append-only events can reach tens of thousands: a 32-bit birthday space
/// collides often enough that `goal_events.id UNIQUE` would start failing real
/// checkpoints. Keep the user-facing Goal/criterion width, widen the audit rows.
pub const EVENT_ID_LEN: usize = 16;
pub fn new_event_id() -> String {
    Uuid::new_v4().simple().to_string()[..EVENT_ID_LEN].to_string()
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
    /// A pause request stopped the run. The Goal returns to `paused` and can be
    /// resumed explicitly; unlike `Cancelled`, this is not terminal.
    Paused,
    /// The run ended without leaving anything observable behind: the activation
    /// only produced text, so another cycle would re-read the same context. The
    /// Goal stays `Running` for the next activation.
    NoProgress,
    /// An explicit user cancel request (`/goal cancel`) stopped the run.
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

    /// The default required criterion is human-verified.
    ///
    /// Automatic verification is reserved until the runner has a real check
    /// for it; `GoalManager::create` rejects an Automatic criterion rather
    /// than creating a Goal that can never pass.
    pub fn required(description: impl Into<String>) -> Self {
        Self::new(description, true, GoalVerificationKind::Human)
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
    pub criteria: Option<Vec<GoalCriterion>>,
    pub evidence: Vec<GoalEvidenceUpdate>,
    /// Optional lifecycle transition, applied through the same legal-transition
    /// table as [`Goal::transition_to`] so a status change can be persisted
    /// atomically with the rest of the update (and with a fencing lease, when
    /// the caller supplies one).
    pub status: Option<GoalStatus>,
    /// File a finished Goal away (`Some(true)`) or bring it back
    /// (`Some(false)`).
    ///
    /// A flag rather than a `GoalStatus::Archived` on purpose: the status a
    /// finished Goal reached **is** its record — a failed Goal must not start
    /// reporting itself as "archived" — and nothing about the lifecycle
    /// changes when it is filed. It is also the one write a terminal Goal
    /// accepts; see [`Goal::update`].
    pub archived: Option<bool>,
}

impl GoalUpdate {
    /// True when the only thing this update does is file the Goal away.
    ///
    /// That is the single mutation a *terminal* Goal accepts: the outcome of a
    /// finished Goal is frozen (title, objective, criteria, evidence, status),
    /// while where it is listed is not.
    fn is_archive_only(&self) -> bool {
        self.archived.is_some()
            && self.title.is_none()
            && self.objective.is_none()
            && self.criteria.is_none()
            && self.evidence.is_empty()
            && self.status.is_none()
    }
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
    /// When the Goal was filed out of the way, if it was.
    ///
    /// Archiving is presentation, not lifecycle: the Goal keeps the status it
    /// finished in, its runs and its events, and stops taking space in the
    /// sticky panel / `/goal list`. Optional, so every row written before this
    /// field existed decodes as "not archived".
    #[serde(default)]
    pub archived_at: Option<DateTime<Utc>>,
}

impl Goal {
    pub fn new(input: GoalCreate) -> Result<Self, GoalError> {
        validate_text(&input.title, GoalError::EmptyTitle)?;
        validate_text(&input.objective, GoalError::EmptyObjective)?;
        for criterion in &input.criteria {
            criterion.validate()?;
        }
        validate_unique_criterion_ids(&input.criteria)?;

        let now = Utc::now();
        Ok(Self {
            id: new_id(),
            title: input.title,
            objective: input.objective,
            status: GoalStatus::Draft,
            phase: GoalPhase::Planning,
            criteria: input.criteria,
            criteria_version: 1,
            version: 1,
            created_at: now,
            updated_at: now,
            completed_at: None,
            archived_at: None,
        })
    }

    /// Whether the Goal has been filed away.
    pub fn is_archived(&self) -> bool {
        self.archived_at.is_some()
    }

    /// Apply a validated update. Changing criteria always advances the
    /// criteria version and discards evidence from the previous definition;
    /// [`GoalUpdate::status`] performs one legal lifecycle transition against
    /// the fields this call just applied (so `succeed()` still has to pass
    /// [`Goal::validate_acceptance`]).
    pub fn update(&mut self, update: GoalUpdate) -> Result<(), GoalError> {
        // A terminal Goal's record is frozen: after it finished, nothing may
        // rewrite its title, objective, criteria, evidence or status. Filing it
        // away is the exception — it changes where the Goal is listed, not what
        // it did.
        if self.status.is_terminal() && !update.is_archive_only() {
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
        if let Some(criteria) = update.criteria {
            for criterion in &criteria {
                criterion.validate()?;
            }
            validate_unique_criterion_ids(&criteria)?;
            validate_supported_criteria(&criteria)?;
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
            // A rejection is a user decision, not a gap to be filled by the
            // next writer: only redefining the criteria (which bumps the
            // version and clears evidence) may bring the criterion back.
            if criterion.status == GoalCriterionStatus::Rejected {
                return Err(GoalError::CriterionRejected {
                    criterion_id: evidence.criterion_id,
                });
            }
            criterion.record_evidence(evidence.value, candidate.criteria_version);
        }
        if let Some(archived) = update.archived {
            // Only a finished Goal can be filed away: an archive that hid a
            // Goal still being worked on would take the work off the panel
            // while it keeps running.
            if archived && !candidate.status.is_terminal() {
                return Err(GoalError::UnfinishedGoal {
                    status: candidate.status,
                });
            }
            candidate.archived_at = archived.then(Utc::now);
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

    /// Whether any criterion must be satisfied before this Goal may succeed.
    ///
    /// Criteria are optional. A Goal created without them (the default for
    /// `/goal create`) has nothing to gate success on: it finishes on the
    /// model's completion proposal, and the runner records that as
    /// self-attested rather than pretending it was verified.
    pub fn has_required_criteria(&self) -> bool {
        self.criteria.iter().any(|criterion| criterion.required)
    }

    /// Validate the evidence recorded for the current criteria version.
    ///
    /// Only *required* criteria gate success, and only recorded, versioned
    /// evidence satisfies them. A Goal without required criteria passes
    /// vacuously — there is nothing for evidence to prove.
    pub fn validate_acceptance(&self) -> Result<(), GoalError> {
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

/// Criterion ids are what completion claims and evidence address, so a
/// duplicate would make the same id resolve to two different criteria.
fn validate_unique_criterion_ids(criteria: &[GoalCriterion]) -> Result<(), GoalError> {
    for (index, criterion) in criteria.iter().enumerate() {
        if criteria[..index]
            .iter()
            .any(|existing| existing.id == criterion.id)
        {
            return Err(GoalError::DuplicateCriterionId {
                criterion_id: criterion.id.clone(),
            });
        }
    }
    Ok(())
}

/// The manager-level gate for criteria the current runner can actually verify.
///
/// `Goal::new` stays permissive so the verifier's pure tests can construct a
/// Goal with an Automatic criterion; every production create/update path goes
/// through this check and fails closed.
///
/// Criteria themselves are optional: a Goal without required criteria is a
/// legitimate request whose completion is self-attested. Automatic criteria
/// are still refused because nothing in production produces their evidence,
/// which would make a required criterion unsatisfiable.
fn validate_supported_criteria(criteria: &[GoalCriterion]) -> Result<(), GoalError> {
    if let Some(criterion) = criteria
        .iter()
        .find(|criterion| criterion.verification_kind == GoalVerificationKind::Automatic)
    {
        return Err(GoalError::AutomaticVerificationUnsupported {
            criterion_id: criterion.id.clone(),
        });
    }
    Ok(())
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
        // `Verifying` allows `Paused` / `WaitingPermission` like every other
        // non-terminal state: the AcceptGoal path writes `Verifying` and
        // `Succeeded` as two separate updates, and a pause in that window used
        // to error out while the Goal advanced to `Succeeded` anyway
        // (2026-10-16).
        Verifying => matches!(
            to,
            Succeeded | Running | WaitingUser | WaitingPermission | Paused | Failed | Cancelled
        ),
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
    /// The last `goal_events.seq` that existed when this run started.
    ///
    /// "Did anything happen *during this run*" is asked several times per
    /// activation (a checkpoint, a completion proposal, a pause/cancel
    /// request), and the answer used to be a timestamp comparison on
    /// `occurred_at` in milliseconds. Events written in the same millisecond as
    /// the run's start could not be ordered against it, and `>=` then counted a
    /// previous run's proposal as this one's. A seq is monotonic and unique, so
    /// `seq > event_seq_anchor` is the exact predicate (2026-10-20).
    ///
    /// `0` means "unknown" (a run record written before this field existed, or
    /// by a caller that did not fill it): readers fall back to the timestamp
    /// window, which is what they used before.
    #[serde(default)]
    pub event_seq_anchor: u64,
}

impl GoalRun {
    pub fn new(goal_id: GoalId) -> Self {
        Self {
            id: new_event_id(),
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
            event_seq_anchor: 0,
        }
    }

    pub fn finish(&mut self, outcome: GoalRunOutcome) {
        self.outcome = Some(outcome);
        self.finished_at = Some(Utc::now());
    }
}

/// The closed taxonomy of Goal event kinds.
///
/// Serialized as the bare snake_case token so the persisted JSON and the
/// store's `json_extract(data, '$.kind')` queries keep matching unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalEventKind {
    Checkpoint,
    NeedsUser,
    CompletionProposed,
    Steering,
    StatusChanged,
    PauseRequested,
    CancelRequested,
    RunFinished,
    Archived,
    Unarchived,
    UserAnswer,
    UserAccepted,
}

impl GoalEventKind {
    /// The persisted token, for `json_extract` query bindings.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Checkpoint => "checkpoint",
            Self::NeedsUser => "needs_user",
            Self::CompletionProposed => "completion_proposed",
            Self::Steering => "steering",
            Self::StatusChanged => "status_changed",
            Self::PauseRequested => "pause_requested",
            Self::CancelRequested => "cancel_requested",
            Self::RunFinished => "run_finished",
            Self::Archived => "archived",
            Self::Unarchived => "unarchived",
            Self::UserAnswer => "user_answer",
            Self::UserAccepted => "user_accepted",
        }
    }
}

impl std::fmt::Display for GoalEventKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Who appended a Goal event.
///
/// The event log mixes two authorities that used to be indistinguishable by
/// inspection: the model (through the Goal tools) and the user (through the
/// command surface / acceptance). `book/28` requires that a model tool can
/// never forge a user decision, and today that holds only because no tool
/// writes `user_accepted` — a property that has to survive every future tool
/// addition. Recording the author makes it checkable instead of remembered
/// (2026-10-20).
///
/// `Runner` is the default for rows written before this field existed: they
/// were overwhelmingly Runner/status writes, and the default has to be a value
/// that cannot be mistaken for a user decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalEventSource {
    /// The model, through a Goal tool (`goal_checkpoint`, …).
    Model,
    /// The user, through a command (`/goal steer`, `/goal accept`, a reply).
    User,
    /// The Runner / store / driver itself: transitions, run finishes,
    /// archive flags, recovery.
    #[default]
    Runner,
}

impl GoalEventSource {
    /// The persisted token.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::User => "user",
            Self::Runner => "runner",
        }
    }

    /// Whether this event records something only the user is entitled to say.
    ///
    /// The kinds a model must never author. Kept next to the enum so the rule
    /// has one home; the store rejects a violating event rather than trusting
    /// every future caller to remember it.
    pub fn may_author(&self, kind: GoalEventKind) -> bool {
        match kind {
            GoalEventKind::UserAnswer | GoalEventKind::UserAccepted => !matches!(self, Self::Model),
            _ => true,
        }
    }
}

impl std::fmt::Display for GoalEventSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    pub kind: GoalEventKind,
    /// Who appended this event. Defaults to [`GoalEventSource::Runner`] so a
    /// row written before the field existed cannot read as a user decision.
    #[serde(default)]
    pub source: GoalEventSource,
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
            id: new_event_id(),
            goal_id: goal.id.clone(),
            run_id: Some(run.id.clone()),
            kind: GoalEventKind::RunFinished,
            source: GoalEventSource::Runner,
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
        let goal = Goal::new(input).map_err(anyhow::Error::new)?;
        validate_supported_criteria(&goal.criteria).map_err(anyhow::Error::new)?;
        self.store.create(goal).await
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
    /// Counts a Goal's events — the progress gate's cheap probe.
    pub async fn count_events(&self, id: &str) -> anyhow::Result<u64> {
        self.store.count_events(id).await
    }
    /// Lists the most recent `limit` events, oldest first.
    pub async fn list_recent_events(
        &self,
        id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<GoalEvent>> {
        self.store.list_recent_events(id, limit).await
    }
    /// The most recent event of one kind, if any.
    pub async fn latest_event_of_kind(
        &self,
        id: &str,
        kind: GoalEventKind,
    ) -> anyhow::Result<Option<GoalEvent>> {
        self.store.latest_event_of_kind(id, kind).await
    }
    /// The most recent event carrying one status, if any.
    pub async fn latest_event_with_status(
        &self,
        id: &str,
        status: GoalStatus,
    ) -> anyhow::Result<Option<GoalEvent>> {
        self.store.latest_event_with_status(id, status).await
    }
    /// Whether an event of one kind exists at or after `since`.
    pub async fn has_event_of_kind_since(
        &self,
        id: &str,
        kind: GoalEventKind,
        since: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<bool> {
        self.store.has_event_of_kind_since(id, kind, since).await
    }
    /// Whether an event of one kind was appended after `seq` (exact window).
    pub async fn has_event_of_kind_after(
        &self,
        id: &str,
        kind: GoalEventKind,
        seq: u64,
    ) -> anyhow::Result<bool> {
        self.store.has_event_of_kind_after(id, kind, seq).await
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
    /// Starts a run holding an already-claimed lease.
    pub async fn start_run_with_lease(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        ttl: std::time::Duration,
        run: GoalRun,
    ) -> anyhow::Result<GoalRun> {
        self.store.start_run_with_lease(token, ttl, run).await
    }
    /// Publishes in-flight run usage while the row is still `running`.
    pub async fn update_run_progress(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        run: &GoalRun,
    ) -> anyhow::Result<()> {
        self.store.update_run_progress(token, run).await
    }
    /// Applies a sequence of Goal updates in one transaction.
    pub async fn update_chain(
        &self,
        goal_id: &str,
        expected_version: u64,
        updates: Vec<(GoalUpdate, GoalEvent)>,
        lease: Option<&crate::store::goal_store::LeaseToken>,
    ) -> anyhow::Result<Goal> {
        self.store
            .update_chain(goal_id, expected_version, updates, lease)
            .await
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
    /// Interrupts active runs whose singleton lease is expired or whose holder
    /// process is gone.
    pub async fn recover_dead_active_runs(&self) -> anyhow::Result<Vec<GoalRun>> {
        self.store.recover_dead_active_runs().await
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
    /// Counts a Goal's events — the progress gate's cheap probe.
    pub async fn count_events(&self, id: &str) -> anyhow::Result<u64> {
        self.inner.count_events(id).await
    }
    /// Lists the most recent `limit` events, oldest first.
    pub async fn list_recent_events(
        &self,
        id: &str,
        limit: usize,
    ) -> anyhow::Result<Vec<GoalEvent>> {
        self.inner.list_recent_events(id, limit).await
    }
    /// The most recent event of one kind, if any.
    pub async fn latest_event_of_kind(
        &self,
        id: &str,
        kind: GoalEventKind,
    ) -> anyhow::Result<Option<GoalEvent>> {
        self.inner.latest_event_of_kind(id, kind).await
    }
    /// The most recent event carrying one status, if any.
    pub async fn latest_event_with_status(
        &self,
        id: &str,
        status: GoalStatus,
    ) -> anyhow::Result<Option<GoalEvent>> {
        self.inner.latest_event_with_status(id, status).await
    }
    /// Whether an event of one kind exists at or after `since`.
    pub async fn has_event_of_kind_since(
        &self,
        id: &str,
        kind: GoalEventKind,
        since: chrono::DateTime<chrono::Utc>,
    ) -> anyhow::Result<bool> {
        self.inner.has_event_of_kind_since(id, kind, since).await
    }
    /// Whether an event of one kind was appended after `seq` (exact window).
    pub async fn has_event_of_kind_after(
        &self,
        id: &str,
        kind: GoalEventKind,
        seq: u64,
    ) -> anyhow::Result<bool> {
        self.inner.has_event_of_kind_after(id, kind, seq).await
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
    /// Starts a run holding an already-claimed lease.
    pub async fn start_run_with_lease(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        ttl: std::time::Duration,
        run: GoalRun,
    ) -> anyhow::Result<GoalRun> {
        self.inner.start_run_with_lease(token, ttl, run).await
    }
    /// Publishes in-flight run usage while the row is still `running`.
    pub async fn update_run_progress(
        &self,
        token: &crate::store::goal_store::LeaseToken,
        run: &GoalRun,
    ) -> anyhow::Result<()> {
        self.inner.update_run_progress(token, run).await
    }
    /// Applies a sequence of Goal updates in one transaction.
    pub async fn update_chain(
        &self,
        goal_id: &str,
        expected_version: u64,
        updates: Vec<(GoalUpdate, GoalEvent)>,
        lease: Option<&crate::store::goal_store::LeaseToken>,
    ) -> anyhow::Result<Goal> {
        self.inner
            .update_chain(goal_id, expected_version, updates, lease)
            .await
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
    /// Interrupts active runs whose singleton lease is expired or whose holder
    /// process is gone.
    pub async fn recover_dead_active_runs(&self) -> anyhow::Result<Vec<GoalRun>> {
        self.inner.recover_dead_active_runs().await
    }
}

/// Errors returned by pure Goal validation and transitions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalError {
    EmptyTitle,
    EmptyObjective,
    EmptyCriterionId,
    EmptyCriterion {
        criterion_id: String,
    },
    EmptyEvidence {
        criterion_id: String,
    },
    CriterionNotFound {
        criterion_id: String,
    },
    /// Criterion ids must be unique inside one Goal: claims and evidence
    /// address a criterion by id.
    DuplicateCriterionId {
        criterion_id: String,
    },
    /// A rejected criterion is a user decision; new evidence may not silently
    /// flip it back to passed.
    CriterionRejected {
        criterion_id: String,
    },
    InvalidTransition {
        from: GoalStatus,
        to: GoalStatus,
    },
    TerminalGoalMutation {
        status: GoalStatus,
    },
    /// An archive was refused because the Goal is still in flight.
    UnfinishedGoal {
        status: GoalStatus,
    },
    /// Automatic verification is reserved: no runner-side checker exists yet,
    /// so accepting such a criterion would make the Goal uncompletable.
    AutomaticVerificationUnsupported {
        criterion_id: String,
    },
    AcceptanceCriteriaNotMet {
        criterion_id: String,
    },
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
            Self::DuplicateCriterionId { criterion_id } => {
                write!(f, "goal criterion id {criterion_id} is used more than once")
            }
            Self::CriterionRejected { criterion_id } => {
                write!(
                    f,
                    "goal criterion {criterion_id} was rejected; redefine the criteria to retry"
                )
            }
            Self::InvalidTransition { from, to } => {
                write!(f, "invalid goal transition from {from:?} to {to:?}")
            }
            Self::TerminalGoalMutation { status } => {
                write!(f, "cannot mutate terminal goal in {status:?} state")
            }
            Self::UnfinishedGoal { status } => {
                write!(
                    f,
                    "goal is still {status:?}: only a finished goal can be archived"
                )
            }
            Self::AutomaticVerificationUnsupported { criterion_id } => {
                write!(
                    f,
                    "goal criterion {criterion_id} requests automatic verification, which is not implemented yet"
                )
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
            kind: GoalEventKind::StatusChanged,
            source: GoalEventSource::Runner,
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
        let mut goal = manager.get(&created.id).await.unwrap();
        for status in [GoalStatus::Queued, GoalStatus::Running] {
            goal = manager
                .update(
                    &goal.id,
                    goal.version,
                    GoalUpdate {
                        status: Some(status),
                        ..GoalUpdate::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        let run = GoalRun::new(goal.id.clone());
        let running = manager
            .start_run_with_lease(&token, Duration::from_secs(60), run)
            .await
            .unwrap();
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
        let mut goal = shared.create(input("shared")).await.unwrap();
        for status in [GoalStatus::Queued, GoalStatus::Running] {
            goal = shared
                .update(
                    &goal.id,
                    goal.version,
                    GoalUpdate {
                        status: Some(status),
                        ..GoalUpdate::default()
                    },
                    None,
                    None,
                )
                .await
                .unwrap();
        }
        let token = shared
            .claim_lease("shared-test", Duration::from_secs(60))
            .await
            .unwrap();
        let heartbeated = shared
            .heartbeat_lease(&token, Duration::from_secs(60))
            .await
            .unwrap();
        assert_eq!(heartbeated, token);

        let mut run = shared
            .start_run_with_lease(
                &token,
                Duration::from_secs(60),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        assert_eq!(shared.list_running_runs().await.unwrap(), vec![run.clone()]);
        assert!(shared.list_events(&goal.id).await.unwrap().is_empty());
        run.finish(GoalRunOutcome::ProgressMade);
        assert_eq!(shared.finish_run(&token, run.clone()).await.unwrap(), run);
        assert!(shared.list_running_runs().await.unwrap().is_empty());
        shared.release_lease(&token).await.unwrap();

        let (_active_token, active_run) = shared
            .start_run(
                "shared-test",
                Duration::from_millis(25),
                GoalRun::new(goal.id.clone()),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(40)).await;
        let recovered = shared.recover_dead_active_runs().await.unwrap();
        assert_eq!(recovered.len(), 1);
        assert_eq!(recovered[0].id, active_run.id);
        assert_eq!(recovered[0].stop_reason.as_deref(), Some("lease expired"));
        assert!(shared.list_running_runs().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn manager_allows_criterialess_goals_and_refuses_automatic_ones() {
        let manager = GoalManager::new(&temp_db("criteria-gate")).await.unwrap();
        let base = |criteria: Vec<GoalCriterion>| GoalCreate {
            title: "criteria gate".into(),
            objective: "exercise the create gate".into(),
            criteria,
        };

        // Criteria are optional: a Goal without them completes on the model's
        // proposal, recorded as self-attested.
        let criterialess = manager
            .create(base(Vec::new()))
            .await
            .expect("a criterialess goal is a legitimate request");
        assert!(!criterialess.has_required_criteria());
        let optional_only = manager
            .create(base(vec![GoalCriterion::new(
                "optional",
                false,
                GoalVerificationKind::Human,
            )]))
            .await
            .expect("an optional-only goal is a legitimate request");
        assert!(!optional_only.has_required_criteria());

        let error = manager
            .create(base(vec![GoalCriterion::new(
                "automatic",
                true,
                GoalVerificationKind::Automatic,
            )]))
            .await
            .expect_err("automatic verification is not implemented");
        assert!(error.to_string().contains("automatic verification"));

        let created = manager
            .create(base(vec![GoalCriterion::required("human check")]))
            .await
            .unwrap();
        assert_eq!(created.criteria.len(), 1);
        assert_eq!(
            created.criteria[0].verification_kind,
            GoalVerificationKind::Human
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The injected-context marker is what a reloaded session has left, so it
    /// must recognize the frame the runner writes and nothing else — a real
    /// user turn that merely *mentions* the tag must not be filtered out.
    #[test]
    fn goal_context_marker_recognizes_only_the_injected_frame() {
        assert!(is_goal_context_text(&format!(
            "{GOAL_CONTEXT_OPEN_TAG}\ngoal: abc\n{GOAL_CONTEXT_CLOSE_TAG}"
        )));
        assert!(
            is_goal_context_text("  \n<goal-context>\nbody"),
            "leading whitespace survives a reload"
        );
        assert!(!is_goal_context_text("how does <goal-context> work?"));
        assert!(!is_goal_context_text(
            "`<goal-context>` is injected per activation"
        ));
        assert!(!is_goal_context_text("just a user turn"));
    }

    fn create_goal() -> Goal {
        Goal::new(GoalCreate {
            title: "Ship the release".to_string(),
            objective: "Publish the approved release to production".to_string(),
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
        for id in [&goal.id, &goal.criteria[0].id] {
            assert_eq!(id.len(), ID_LEN, "{id} must be {ID_LEN} characters");
            assert!(
                id.chars().all(|c| c.is_ascii_hexdigit()),
                "{id} must be hex"
            );
        }
        for id in [&run.id, &event.id] {
            assert_eq!(
                id.len(),
                EVENT_ID_LEN,
                "{id} must be {EVENT_ID_LEN} characters"
            );
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
            criteria: vec![GoalCriterion::required("done")],
        })
        .unwrap_err();
        assert!(matches!(title_error, GoalError::EmptyTitle));

        let objective_error = Goal::new(GoalCreate {
            title: "A title".to_string(),
            objective: "\n\t".to_string(),
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
            criteria: Some(vec![GoalCriterion::required_with_id(
                criterion_id,
                "release is published and smoke-tested",
            )]),
            evidence: Vec::new(),
            status: None,
            archived: None,
        })
        .unwrap();

        assert_eq!(goal.criteria_version, old_version + 1);
        assert!(!goal.criteria[0].has_valid_evidence(goal.criteria_version));
        assert!(goal.criteria[0].evidence.is_empty());
    }

    #[test]
    fn criteria_updates_only_reject_unsupported_kinds() {
        let mut goal = create_goal();
        // An optional-only replacement is fine: nothing about it can dead-end.
        goal.update(GoalUpdate {
            criteria: Some(vec![GoalCriterion::new(
                "optional",
                false,
                GoalVerificationKind::Human,
            )]),
            ..GoalUpdate::default()
        })
        .unwrap();
        assert!(!goal.has_required_criteria());
        let version = goal.criteria_version;

        let error = goal
            .update(GoalUpdate {
                criteria: Some(vec![GoalCriterion::new(
                    "automatic",
                    true,
                    GoalVerificationKind::Automatic,
                )]),
                ..GoalUpdate::default()
            })
            .unwrap_err();
        assert!(matches!(
            error,
            GoalError::AutomaticVerificationUnsupported { .. }
        ));
        assert_eq!(goal.criteria_version, version, "failed updates do not bump");
    }

    /// Archiving is a stamp, not a lifecycle move: the Goal keeps the outcome
    /// it reached, and filing it away is the one write a terminal Goal accepts.
    #[test]
    fn archiving_is_the_only_write_a_finished_goal_accepts() {
        let mut goal = create_goal();
        goal.update(GoalUpdate {
            status: Some(GoalStatus::Cancelled),
            ..GoalUpdate::default()
        })
        .unwrap();
        let version = goal.version;

        goal.update(GoalUpdate {
            archived: Some(true),
            ..GoalUpdate::default()
        })
        .unwrap();
        assert!(goal.is_archived());
        assert_eq!(
            goal.status,
            GoalStatus::Cancelled,
            "the outcome is what the Goal reports, archived or not"
        );
        assert!(
            goal.version > version,
            "archiving is persisted like any write"
        );

        // Everything else about a finished Goal is frozen.
        assert!(matches!(
            goal.update(GoalUpdate {
                title: Some("renamed".into()),
                ..GoalUpdate::default()
            }),
            Err(GoalError::TerminalGoalMutation { .. })
        ));
        assert!(
            matches!(
                goal.update(GoalUpdate {
                    archived: Some(true),
                    title: Some("renamed".into()),
                    ..GoalUpdate::default()
                }),
                Err(GoalError::TerminalGoalMutation { .. })
            ),
            "an archive may not smuggle another field through"
        );

        goal.update(GoalUpdate {
            archived: Some(false),
            ..GoalUpdate::default()
        })
        .unwrap();
        assert!(!goal.is_archived());
        assert_eq!(goal.status, GoalStatus::Cancelled);
    }

    /// An archive that hid running work would take the Goal off the panel while
    /// it kept going, so the domain refuses it.
    #[test]
    fn archiving_an_unfinished_goal_is_refused() {
        let mut goal = create_goal();
        goal.update(GoalUpdate {
            status: Some(GoalStatus::Queued),
            ..GoalUpdate::default()
        })
        .unwrap();

        let error = goal
            .update(GoalUpdate {
                archived: Some(true),
                ..GoalUpdate::default()
            })
            .unwrap_err();
        assert!(matches!(error, GoalError::UnfinishedGoal { .. }), "{error}");
        assert!(!goal.is_archived());
    }

    /// Rows written before `archived_at` existed decode as "not archived" —
    /// the field is additive, so no store migration is needed.
    #[test]
    fn a_goal_without_the_archived_field_decodes_as_unarchived() {
        let goal = create_goal();
        let mut json: serde_json::Value = serde_json::to_value(&goal).unwrap();
        json.as_object_mut().unwrap().remove("archived_at");

        let decoded: Goal = serde_json::from_value(json).unwrap();
        assert!(!decoded.is_archived());
    }

    #[test]
    fn invalid_multi_field_update_is_atomic() {
        let mut goal = create_goal();
        let before = goal.clone();

        let error = goal
            .update(GoalUpdate {
                title: Some("A new title".to_string()),
                objective: Some("A new objective".to_string()),
                criteria: Some(vec![GoalCriterion::required("  ")]),
                evidence: Vec::new(),
                status: None,
                archived: None,
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

    /// Criteria are optional: with nothing required, `succeed()` is allowed and
    /// the runner labels the completion self-attested.
    #[test]
    fn success_allows_a_goal_without_acceptance_criteria() {
        let mut goal = Goal::new(GoalCreate {
            title: "Ship it".to_string(),
            objective: "Publish it".to_string(),
            criteria: Vec::new(),
        })
        .unwrap();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::Verifying).unwrap();

        assert!(!goal.has_required_criteria());
        goal.succeed().unwrap();
        assert_eq!(goal.status, GoalStatus::Succeeded);
    }

    #[test]
    fn success_allows_an_all_optional_acceptance_list() {
        let mut goal = Goal::new(GoalCreate {
            title: "Ship it".to_string(),
            objective: "Publish it".to_string(),
            criteria: vec![GoalCriterion::new(
                "optional nicety",
                false,
                GoalVerificationKind::Automatic,
            )],
        })
        .unwrap();
        goal.transition_to(GoalStatus::Queued).unwrap();
        goal.transition_to(GoalStatus::Running).unwrap();
        goal.transition_to(GoalStatus::Verifying).unwrap();

        assert!(!goal.has_required_criteria());
        goal.succeed().unwrap();
        assert_eq!(goal.status, GoalStatus::Succeeded);
    }

    #[test]
    fn rejected_criteria_do_not_accept_new_evidence() {
        let mut goal = create_goal();
        goal.criteria[0].status = GoalCriterionStatus::Rejected;
        let before = goal.clone();

        let error = goal
            .update(GoalUpdate {
                evidence: vec![GoalEvidenceUpdate {
                    criterion_id: goal.criteria[0].id.clone(),
                    value: "looks fine now".to_string(),
                }],
                ..GoalUpdate::default()
            })
            .unwrap_err();

        assert!(matches!(error, GoalError::CriterionRejected { .. }));
        assert_eq!(goal, before, "a rejected criterion must not be resurrected");
    }

    #[test]
    fn duplicate_criterion_ids_are_rejected_on_create_and_update() {
        let mut first = GoalCriterion::required("first");
        first.id = "dup-id".to_string();
        let mut second = GoalCriterion::required("second");
        second.id = "dup-id".to_string();

        let create_error = Goal::new(GoalCreate {
            title: "Ship it".to_string(),
            objective: "Publish it".to_string(),
            criteria: vec![first, second],
        })
        .unwrap_err();
        assert!(matches!(
            create_error,
            GoalError::DuplicateCriterionId { .. }
        ));

        let mut goal = create_goal();
        let mut first = GoalCriterion::required("first");
        first.id = "same".to_string();
        let mut second = GoalCriterion::required("second");
        second.id = "same".to_string();
        let update_error = goal
            .update(GoalUpdate {
                criteria: Some(vec![first, second]),
                ..GoalUpdate::default()
            })
            .unwrap_err();
        assert!(matches!(
            update_error,
            GoalError::DuplicateCriterionId { .. }
        ));
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
