//! SQLite-backed persistence for durable Goal aggregates, runs, events, and leases.

pub use sqlite::SqliteGoalStore;

use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;

use crate::goal::{Goal, GoalEvent, GoalRun, GoalStatus, GoalUpdate};

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
    async fn latest_event_of_kind(&self, goal_id: &str, kind: &str) -> Result<Option<GoalEvent>>;
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
        kind: &str,
        since: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool>;
    async fn claim_lease(&self, owner: &str, ttl: Duration) -> Result<LeaseToken>;
    async fn heartbeat_lease(&self, token: &LeaseToken, ttl: Duration) -> Result<LeaseToken>;
    async fn release_lease(&self, token: &LeaseToken) -> Result<()>;
    async fn start_run(
        &self,
        owner: &str,
        ttl: Duration,
        run: GoalRun,
    ) -> Result<(LeaseToken, GoalRun)>;
    async fn finish_run(&self, token: &LeaseToken, run: GoalRun) -> Result<GoalRun>;
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
    async fn recover_expired_active_runs(&self) -> Result<Vec<GoalRun>>;
}

mod sqlite;
