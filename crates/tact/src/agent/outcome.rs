//! Typed outcome of one bounded [`Agent::agent_loop_detailed`] activation.
//!
//! The driver-owned Goal Runner needs to know *why* one bounded activation of
//! the agent loop returned and how much budget it consumed, instead of
//! inferring the reason from `runtime.goal_yield_reason` plus scattered state.
//! These plain (serialization-free) types carry that information back to the
//! caller without disturbing existing callers of [`Agent::agent_loop`].
//!
//! [`Agent::agent_loop_detailed`]: super::Agent::agent_loop_detailed
//! [`Agent::agent_loop`]: super::Agent::agent_loop

/// Why one bounded `agent_loop` activation returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentLoopStop {
    /// The model ended the turn (`end_turn` / `stop_sequence` / `pause_turn` / none)
    /// and no tool asked to yield.
    Completed,
    /// A Goal tool asked the Runner to hand control back to the user.
    GoalYield(crate::tool::GoalYieldReason),
    /// The turn cap (`max_turns`) was reached.
    MaxTurns,
    /// Output was truncated and the continuation attempts were exhausted, so
    /// pending tool calls were abandoned.
    MaxTokens,
    /// A user cancel request stopped the activation.
    Cancelled,
    /// A hook blocked the activation before the first model call
    /// (`SessionStart` or `UserPromptSubmit`).
    BlockedByHook,
}

/// What one bounded `agent_loop` activation did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentLoopOutcome {
    /// Why the activation returned.
    pub stop: AgentLoopStop,
    /// Model calls (loop iterations) consumed by this activation.
    pub turns: u32,
    /// Tool calls actually executed by this activation. Calls that were denied,
    /// blocked, or abandoned-unexecuted are NOT counted.
    pub tool_calls: u64,
}
