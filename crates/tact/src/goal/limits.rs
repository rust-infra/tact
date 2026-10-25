//! The budget for one bounded Goal run.
//!
//! Lives here rather than in the runner because the projection (`snapshots`) and
//! the runner both have to agree on it: a panel showing a different limit than
//! the runner enforces is a lie the user only discovers when the run stops.
//!
//! The whole rule is one function so it has one definition and one test.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GoalLimits {
    /// Model calls (loop turns) one Goal run may consume.
    pub max_turns: u32,
    /// Wall-clock ceiling for one Goal run.
    pub max_elapsed: Duration,
}

/// Default model-call budget for one Goal run.
///
/// A run is a bounded slice of work, not a session. The built-in default lives
/// in [`tact::config::GoalSettings`] (500) so `[goal].max_turns` and the
/// runtime fallback cannot drift; a larger cap still ends through the
/// wall-clock budget or the agent loop's own stop reasons.
impl Default for GoalLimits {
    fn default() -> Self {
        Self::from_settings(&crate::config::GoalSettings::default())
    }
}

impl GoalLimits {
    /// Builds the runner budget from resolved `[goal]` settings.
    pub fn from_settings(settings: &crate::config::GoalSettings) -> Self {
        Self {
            max_turns: settings.max_turns,
            max_elapsed: Duration::from_secs(settings.max_elapsed_secs),
        }
    }

    /// Whether a run that consumed `used_turns` model calls and `elapsed`
    /// wall-clock time is still inside its budget.
    ///
    /// Tool calls are deliberately not capped: every tool call is already
    /// inside a model call, so `max_turns` bounds the fan-out, and a separate
    /// count only ended useful runs early. Usage is still recorded (`GoalRun::
    /// tool_calls_used`) for the run summary and the panel.
    ///
    /// The whole budget rule lives here so it has exactly one definition and one
    /// test; a zero limit rejects every activation (including one that did no
    /// work).
    pub fn permit_activation(&self, used_turns: u32, elapsed: Duration) -> bool {
        used_turns < self.max_turns && elapsed < self.max_elapsed
    }
}
