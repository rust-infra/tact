//! SQLite-backed persistence for durable Goal aggregates, runs, events, and leases.

pub use sqlite::SqliteGoalStore;

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use crate::goal::{Goal, GoalEvent, GoalEventKind, GoalRun, GoalStatus, GoalUpdate};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaseToken {
    pub owner: String,
    pub epoch: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalStoreError {
    NotFound {
        kind: &'static str,
        id: String,
    },
    VersionConflict {
        goal_id: String,
        expected: u64,
        actual: u64,
    },
    LeaseHeld {
        owner: String,
        epoch: i64,
    },
    StaleLease {
        owner: String,
        epoch: i64,
    },
    ActiveRunExists,
    /// A run can only be started for a Goal that is actually `Running`.
    ///
    /// Without this check the runner's load-then-start sequence had a race:
    /// a pause or cancel committed between the two could be overwritten by a
    /// run that started anyway.
    InvalidGoalStatus {
        goal_id: String,
        status: GoalStatus,
    },
    RunNotFinished {
        run_id: String,
    },
    RunIdentityMismatch {
        run_id: String,
        stored_id: String,
        supplied_id: String,
        stored_goal_id: String,
        supplied_goal_id: String,
        stored_started_at: chrono::DateTime<chrono::Utc>,
        supplied_started_at: chrono::DateTime<chrono::Utc>,
    },
}

impl std::fmt::Display for GoalStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound { kind, id } => write!(f, "{kind} {id} not found"),
            Self::VersionConflict {
                goal_id,
                expected,
                actual,
            } => {
                write!(
                    f,
                    "goal {goal_id} version conflict: expected {expected}, actual {actual}"
                )
            }
            Self::LeaseHeld { owner, epoch } => write!(f, "lease held by {owner} at epoch {epoch}"),
            Self::StaleLease { owner, epoch } => write!(f, "stale lease {owner} at epoch {epoch}"),
            Self::ActiveRunExists => write!(f, "an active goal run already exists"),
            Self::InvalidGoalStatus { goal_id, status } => {
                write!(f, "goal {goal_id} is {status:?}; a run requires Running")
            }
            Self::RunNotFinished { run_id } => write!(f, "goal run {run_id} is not finished"),
            Self::RunIdentityMismatch {
                run_id,
                stored_id,
                supplied_id,
                stored_goal_id,
                supplied_goal_id,
                stored_started_at,
                supplied_started_at,
            } => write!(
                f,
                "goal run {run_id} identity mismatch: id {stored_id} vs {supplied_id}, goal {stored_goal_id} vs {supplied_goal_id}, started_at {stored_started_at} vs {supplied_started_at}"
            ),
        }
    }
}
impl std::error::Error for GoalStoreError {}

#[allow(clippy::double_must_use)] // async_trait generates #[must_use] on Pin<Box<dyn Future>> which is already #[must_use]
#[async_trait]
pub trait GoalStore: Send + Sync {
    async fn create(&self, goal: Goal) -> Result<Goal>;
    async fn get(&self, goal_id: &str) -> Result<Goal>;
    async fn list(&self) -> Result<Vec<Goal>>;
    async fn update(
        &self,
        goal_id: &str,
        expected_version: u64,
        update: GoalUpdate,
        event: Option<GoalEvent>,
        lease: Option<&LeaseToken>,
    ) -> Result<Goal>;
    async fn append_event(&self, event: GoalEvent) -> Result<GoalEvent>;
    async fn list_events(&self, goal_id: &str) -> Result<Vec<GoalEvent>>;
    /// How many events a Goal has recorded.
    ///
    /// The cheap "did anything happen" probe: the runner's per-cycle progress
    /// gate compares two counts instead of re-reading and JSON-parsing the
    /// whole log (2026-10-16).
    async fn count_events(&self, goal_id: &str) -> Result<u64>;
    /// The most recent `limit` events, oldest first.
    ///
    /// A bounded tail read — no hot path needs a Goal's whole history, and the
    /// per-cycle context injection used to materialize all of it.
    async fn list_recent_events(&self, goal_id: &str, limit: usize) -> Result<Vec<GoalEvent>>;
    /// The most recent event of one kind, if any.
    async fn latest_event_of_kind(
        &self,
        goal_id: &str,
        kind: GoalEventKind,
    ) -> Result<Option<GoalEvent>>;
    /// The most recent event carrying one status, if any.
    async fn latest_event_with_status(
        &self,
        goal_id: &str,
        status: GoalStatus,
    ) -> Result<Option<GoalEvent>>;
    /// Whether an event of one kind exists at or after `since`.
    ///
    /// The completion-proposal predicate, asked exactly: a bounded read could
    /// miss a proposal from early in a long run, and a missed proposal is a
    /// Goal that can never succeed.
    async fn has_event_of_kind_since(
        &self,
        goal_id: &str,
        kind: GoalEventKind,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool>;
    /// Whether an event of one kind was appended after `seq`.
    ///
    /// The exact form of [`Self::has_event_of_kind_since`]: `seq` is the
    /// table's monotonic insert order, so nothing appended in the same
    /// millisecond as the anchor can be misattributed to the wrong run.
    async fn has_event_of_kind_after(
        &self,
        goal_id: &str,
        kind: GoalEventKind,
        seq: u64,
    ) -> Result<bool>;
    /// The largest `seq` currently stored (0 when the table is empty).
    ///
    /// Taken when a run starts and kept on the run record, so later "since this
    /// run" questions are a seq comparison rather than a clock comparison.
    async fn max_event_seq(&self) -> Result<u64>;
    async fn claim_lease(&self, owner: &str, ttl: Duration) -> Result<LeaseToken>;
    async fn heartbeat_lease(&self, token: &LeaseToken, ttl: Duration) -> Result<LeaseToken>;
    async fn release_lease(&self, token: &LeaseToken) -> Result<()>;
    /// Claims a fresh lease and starts a run (test/bootstrap convenience).
    ///
    /// A runner that already holds a token must use
    /// [`GoalStore::start_run_with_lease`]; a live lease is never re-issued.
    async fn start_run(
        &self,
        owner: &str,
        ttl: Duration,
        run: GoalRun,
    ) -> Result<(LeaseToken, GoalRun)>;
    /// Starts a run holding an already-claimed lease.
    ///
    /// Unlike [`GoalStore::start_run`], this never re-claims the lease: the
    /// caller's token is the authority, so a lease rotation cannot strand the
    /// run on a stale epoch. The Goal must still be `Running` (checked in the
    /// same transaction as the insert).
    async fn start_run_with_lease(
        &self,
        token: &LeaseToken,
        ttl: Duration,
        run: GoalRun,
    ) -> Result<GoalRun>;
    /// Publishes the run's in-flight usage while the row is still `running`.
    ///
    /// The panel reads [`GoalStore::list_running_runs`], which used to see the
    /// all-zero row written at `start_run` until the run finished.
    async fn update_run_progress(&self, token: &LeaseToken, run: &GoalRun) -> Result<()>;
    async fn finish_run(&self, token: &LeaseToken, run: GoalRun) -> Result<GoalRun>;
    /// Applies several validated Goal updates in one transaction, each CASing
    /// against the version produced by the previous one.
    ///
    /// Acceptance needs evidence + `Running → Verifying → Succeeded` to be
    /// one durable commit: three separate updates could leave a Goal stranded
    /// in `Verifying` if the process died between the last two.
    async fn update_chain(
        &self,
        goal_id: &str,
        expected_version: u64,
        updates: Vec<(GoalUpdate, GoalEvent)>,
        lease: Option<&LeaseToken>,
    ) -> Result<Goal>;
    /// Finishes `run` and, in the same transaction, applies the Goal's status
    /// transitions and records the run-finished event.
    ///
    /// Splitting these writes across transactions lets a crash or a stale CAS
    /// land between them, leaving a completed run on a still-`Running` Goal or
    /// dropping the run's own history entry. One commit removes both windows.
    async fn finish_run_and_transition(
        &self,
        token: &LeaseToken,
        run: GoalRun,
        transitions: Vec<(GoalUpdate, GoalEvent)>,
        run_event: GoalEvent,
    ) -> Result<(GoalRun, Goal)>;
    async fn list_running_runs(&self) -> Result<Vec<GoalRun>>;
    /// Interrupts active runs whose execution lease can no longer belong to a
    /// live process — expired, or written by a process that is gone.
    ///
    /// Both halves matter after a restart: a crashed runner leaves `expires_at`
    /// in the future for up to the whole TTL (`max_elapsed + 60s`), and
    /// interrupting only on expiry would show a Goal as `running` while nothing
    /// runs it.
    async fn recover_dead_active_runs(&self) -> Result<Vec<GoalRun>>;
}

mod sqlite;
