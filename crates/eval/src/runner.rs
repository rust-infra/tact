//! What a runner hands back, and the one thing both runners can honestly say.
//!
//! A run's evidence has two halves. The observable half (final answer, tool
//! calls, token usage, wall clock) exists for both runners. The typed half
//! (`AgentLoopStop`, recovery counters) exists only in process: `AgentUpdate`
//! is not serializable, and `AgentLoopOutcome` deliberately never crosses the
//! process boundary.
//!
//! So [`RunArtifact::stop`] is an [`Option`], not a `StopView` with a default.
//! A runner that could not see the stop reason says `None`, and the report
//! prints it as unknown instead of a zero that would average into a suite's
//! numbers as if it were measured.

use std::time::Duration;

use serde::{Deserialize, Serialize};

pub use tact::AgentLoopStop;

/// One tool call as the run saw it.
///
/// `name`, `args`, and `duration_ms` are [`Option`] because the two runners do
/// not see the same things. The host runner reads them from `StepResult`; the
/// CLI runner can read a name and duration only from a finished-step log line,
/// and a provider-activity failure line carries neither. A field one runner
/// cannot observe stays `None` instead of being guessed, so a report never
/// compares a real argument summary against an error message under one name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRecord {
    pub name: Option<String>,
    pub args: Option<String>,
    pub status: ToolStatus,
    pub duration_ms: Option<u64>,
    /// The step's message, or the failure text for a line that named no tool.
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Succeeded,
    Failed,
    Cancelled,
}

/// Token counts, with the provider-reported extras optional.
///
/// `prompt` and `completion` come from both runners. Cache-hit and reasoning
/// tokens are only on `AgentUpdate::TokenUsage`, which the CLI path cannot see
/// (its log line has no field for them), so they are `None` there rather than
/// `0`. A zero would read as "the provider reported no cache hits", which is a
/// different claim from "this runner cannot observe it".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenTotals {
    pub prompt: u64,
    pub completion: u64,
    pub cache_hit: Option<u64>,
    pub reasoning: Option<u64>,
}

/// Everything one attempt produced, described the same way by both runners.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunArtifact {
    pub case_id: String,
    pub attempt: u32,
    /// The session the run wrote, so a reviewer can open its transcript.
    pub session_id: Option<String>,
    /// Final assistant text, or the empty string when the run produced none.
    pub answer: String,
    pub tools: Vec<ToolRecord>,
    pub tokens: TokenTotals,
    pub duration_ms: u64,
    /// Why the activation returned. `None` when this runner cannot see it.
    pub stop: Option<StopView>,
    pub failure: Option<RunFailure>,
    /// The model calls made. `None` when the runner cannot see them.
    pub turns: Option<u32>,
}

/// The stop reason, flattened to a serializable label.
///
/// `AgentLoopStop` is not serializable by design, so this mirrors the closed
/// set instead of leaking a product enum onto disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopView {
    Completed,
    EmptyTurn,
    GoalYield,
    MaxTurns,
    MaxTokens,
    Cancelled,
    BlockedByHook,
}

impl From<AgentLoopStop> for StopView {
    fn from(stop: AgentLoopStop) -> Self {
        match stop {
            AgentLoopStop::Completed => Self::Completed,
            AgentLoopStop::EmptyTurn => Self::EmptyTurn,
            AgentLoopStop::GoalYield(_) => Self::GoalYield,
            AgentLoopStop::MaxTurns => Self::MaxTurns,
            AgentLoopStop::MaxTokens => Self::MaxTokens,
            AgentLoopStop::Cancelled => Self::Cancelled,
            AgentLoopStop::BlockedByHook => Self::BlockedByHook,
        }
    }
}

/// Why an attempt did not finish. Distinct from a failed grade: a crashed run
/// is evidence about the harness or the process, not about the model.
///
/// Every variant is a struct variant with a named `message` field. A newtype
/// variant cannot be serialized under an internal tag, and a failure that
/// cannot be written to the report is exactly the failure a reader most needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunFailure {
    Timeout { limit_secs: u64 },
    Startup { message: String },
    Harness { message: String },
}

impl RunFailure {
    pub fn startup(message: impl Into<String>) -> Self {
        Self::Startup {
            message: message.into(),
        }
    }

    pub fn harness(message: impl Into<String>) -> Self {
        Self::Harness {
            message: message.into(),
        }
    }
}

impl std::fmt::Display for RunFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout { limit_secs } => write!(formatter, "timed out after {limit_secs}s"),
            Self::Startup { message } => write!(formatter, "startup failed: {message}"),
            Self::Harness { message } => write!(formatter, "harness error: {message}"),
        }
    }
}

impl RunArtifact {
    /// A run that produced an answer and recorded no failure.
    pub fn succeeded(&self) -> bool {
        self.failure.is_none() && !self.answer.trim().is_empty()
    }
}

/// One attempt's artifact plus the directory it ran in.
///
/// The work dir travels next to the artifact rather than inside it: it is a
/// scratch path that exists only until grading finishes, so persisting it in a
/// report would promise a reader something already deleted.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub artifact: RunArtifact,
    pub work_dir: std::path::PathBuf,
}

/// Which runner executed a case. A closed set, so a report cannot carry a
/// misspelled runner name that nothing recognises.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunnerKind {
    /// In process, through the same bootstrap as the product frontends.
    Host,
    /// A separate `tact-ui headless` process.
    Cli,
}

impl RunnerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Host => "host",
            Self::Cli => "cli",
        }
    }
}

impl std::fmt::Display for RunnerKind {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl std::str::FromStr for RunnerKind {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "host" => Ok(Self::Host),
            "cli" => Ok(Self::Cli),
            other => Err(format!("unknown runner {other:?}; expected host or cli")),
        }
    }
}

/// What a run executed under, as the runner itself knows it.
///
/// `model` and `base_url` are [`Option`] because the CLI runner can be told to
/// use whatever a config file says, and then it genuinely does not know. A
/// report that filled those with a placeholder would compare equal to a run
/// under a different model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunIdentity {
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub permission_mode: tact_protocol::PermissionMode,
}

/// One runner: the only thing that differs between the CLI and host paths.
#[async_trait::async_trait]
pub trait Runner: Send + Sync {
    fn kind(&self) -> RunnerKind;

    /// The configuration this runner executes under, for the report header.
    fn identity(&self) -> RunIdentity;

    /// Run one attempt of one case. Implementations report failures inside the
    /// artifact rather than returning an error, because a crashed attempt is
    /// still part of the suite's evidence.
    async fn run(&self, case: &crate::EvalCase, attempt: u32) -> RunOutcome;
}

/// Elapsed milliseconds, shared so both runners measure the same thing.
pub(crate) fn elapsed_ms(start: std::time::Instant) -> u64 {
    let elapsed: Duration = start.elapsed();
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stop_reason_has_a_stable_label() {
        for (stop, label) in [
            (AgentLoopStop::Completed, "completed"),
            (AgentLoopStop::EmptyTurn, "empty_turn"),
            (
                AgentLoopStop::GoalYield(tact::tool::GoalYieldReason::NeedsUser),
                "goal_yield",
            ),
            (AgentLoopStop::MaxTurns, "max_turns"),
            (AgentLoopStop::MaxTokens, "max_tokens"),
            (AgentLoopStop::Cancelled, "cancelled"),
            (AgentLoopStop::BlockedByHook, "blocked_by_hook"),
        ] {
            assert_eq!(serde_json::to_value(StopView::from(stop)).unwrap(), label);
        }
    }

    #[test]
    fn an_empty_answer_is_not_a_success() {
        let artifact = RunArtifact {
            case_id: "c".into(),
            attempt: 1,
            session_id: None,
            answer: "   ".into(),
            tools: Vec::new(),
            tokens: Default::default(),
            duration_ms: 1,
            stop: Some(StopView::EmptyTurn),
            failure: None,
            turns: Some(1),
        };
        assert!(!artifact.succeeded());
    }

    #[test]
    fn a_work_dir_is_not_part_of_the_artifact() {
        // The work dir is deleted after grading, so it must not be persisted as
        // if it were durable evidence.
        let json = serde_json::to_string(&RunArtifact {
            case_id: "c".into(),
            attempt: 1,
            session_id: Some("s".into()),
            answer: "a".into(),
            tools: Vec::new(),
            tokens: Default::default(),
            duration_ms: 3,
            stop: None,
            failure: None,
            turns: None,
        })
        .unwrap();
        assert!(!json.contains("work_dir"), "{json}");
    }
}
