//! Agent–TUI protocol types.
//!
//! These messages flow between the agent runtime and the terminal UI:
//! execution status updates, user commands, step results, token usage, errors,
//! and streaming output.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

use crate::biz::{BalanceInfo, UsageQuotaInfo};

/// Execution status of a step.
#[derive(Debug, Clone)]
pub enum StepStatus {
    Success,
    Failed,
}

/// Structured result of a step execution.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub tool: String,
    pub arg_summary: String,
    /// Full tool argument summary (untruncated), used by detailed UI views.
    pub arg_full: Option<String>,
    pub status: StepStatus,
    pub message: String,
    /// Additional details, e.g. full content of a written file or raw command output.
    pub detail: Option<String>,
    /// Tool execution duration in microseconds. None for non-tool steps.
    pub duration_us: Option<u64>,
    /// Permission choice label when the user was prompted (e.g. "Allow once").
    pub permission_label: Option<String>,
}

/// Parameters for a model API call.
#[derive(Debug, Clone)]
pub struct ModelCallParams {
    pub model: String,
    pub max_tokens: u32,
    pub thinking_budget: Option<u32>,
    pub reasoning_effort: Option<String>,
    pub extra_body: Option<String>,
}

/// Error classification — lets the TUI distinguish fatal errors (displayed as ❌ Error)
/// from non-fatal situations (shown as Info).
#[derive(Debug, Clone)]
pub enum AgentErrorKind {
    /// Balance query failed (network or API error)
    BalanceQueryFailed(String),
    /// Balance query is only supported for DeepSeek provider
    BalanceNotSupported,
    /// Generic error (catch-all)
    Other(String),
}

impl AgentErrorKind {
    /// Returns a human-readable error description.
    pub fn display(&self) -> &str {
        match self {
            AgentErrorKind::BalanceQueryFailed(e) => e,
            AgentErrorKind::BalanceNotSupported => {
                "Balance query is only available for DeepSeek provider"
            }
            AgentErrorKind::Other(msg) => msg,
        }
    }
}

/// Token usage info returned from an LLM API call.
#[derive(Debug, Clone, Default)]
pub struct TokenUsageInfo {
    pub prompt: u32,
    pub completion: u32,
    pub total: u32,
    /// DeepSeek KV cache hit prompt tokens (0 for non-DeepSeek providers)
    pub prompt_cache_hit_tokens: u32,
    /// DeepSeek KV cache miss prompt tokens
    pub prompt_cache_miss_tokens: u32,
    /// Reasoning tokens consumed by the model (R1 / V3 thinking).
    pub reasoning_tokens: u32,
}

/// Status update messages sent from the Agent to the TUI.
#[derive(Debug)]
pub enum AgentUpdate {
    /// Pre-generated plan batch (legacy).
    ///
    /// The current agent runtime does not emit this variant. The plan panel is
    /// driven by [`StepAdded`](Self::StepAdded) and [`StepStarted`](Self::StepStarted).
    /// The TUI handler is retained for backward compatibility only.
    #[deprecated(
        since = "0.19.0",
        note = "use StepAdded/StepStarted; agent no longer emits PlanGenerated"
    )]
    PlanGenerated(Vec<PlanStep>),
    /// Dynamically append a step to the existing plan (does not reset selection state)
    StepAdded(PlanStep),
    /// Step `idx` has started execution
    StepStarted(
        usize,
        String, /* tool_id */
        String, /* tool_name */
        String, /* arg_summary */
    ),
    /// Step `idx` succeeded, with structured result
    StepFinished(usize, String /* tool_id */, StepResult),
    /// Step `idx` failed, with error message
    StepFailed(usize, String /* tool_id */, String),
    /// The entire task is complete
    TaskComplete(String),
    /// Agent error, with classification for the TUI to decide display style
    Error(AgentErrorKind),
    /// Token usage stats
    TokenUsage {
        prompt: u32,
        completion: u32,
        total: u32,
        /// DeepSeek KV cache hit prompt tokens (0 for non-DeepSeek providers)
        prompt_cache_hit_tokens: u32,
        /// DeepSeek KV cache miss prompt tokens
        prompt_cache_miss_tokens: u32,
        /// Reasoning tokens consumed by the model (R1 / V3 thinking).
        /// This is a subset of `completion` exposed by the usage object's
        /// `completion_tokens_details.reasoning_tokens` field.
        reasoning_tokens: u32,
    },
    /// Account balance info (DeepSeek / Moonshot Open Platform)
    Balance(BalanceInfo),
    /// Kimi Code subscription quota (weekly + rolling window).
    UsageQuota(UsageQuotaInfo),
    /// Model call parameters (name, max_tokens, thinking budget, etc.)
    ModelInfo(ModelCallParams),
    /// Informational notice (does not change state)
    Info(String),

    /// Request user to choose from a list of options; returns option index (None = cancelled)
    RequestSelect {
        prompt: String,
        options: Vec<String>,
        respond: oneshot::Sender<Option<usize>>,
    },
    /// Streaming output text fragment (appended to Log in real time)
    StreamChunk(String),
    /// Streaming thinking / reasoning content fragment
    ThinkingChunk(String),
}

/// User commands sent from the TUI to the Agent.
#[derive(Debug)]
pub enum UserCommand {
    /// Submit a new natural-language task
    SubmitTask(String),
    /// Cancel the current task (full cancellation logic not yet implemented)
    Cancel,
    /// Query account balance (DeepSeek/Kimi)
    QueryBalance,
}

/// A single step in the execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Human-readable step description
    pub description: String,
    /// Tool name: read_file / write_file / run_command
    pub tool: String,
    /// LLM-assigned tool-use id from the assistant message.
    #[serde(default)]
    pub tool_id: String,
    /// Tool arguments (key-value pairs)
    pub args: HashMap<String, String>,
    /// Whether user manual approval is required before execution (legacy).
    ///
    /// Permission flow is driven by `PermissionManager` at tool dispatch time;
    /// the agent does not set this flag today.
    #[deprecated(
        since = "0.19.0",
        note = "permission is enforced by PermissionManager, not PlanStep flags"
    )]
    pub need_approval: bool,
    /// Output after execution (populated by TUI; defaults to None on JSON deserialization)
    #[serde(default)]
    pub output: Option<String>,
}

impl PlanStep {
    /// Construct a plan step for the streaming agent loop.
    pub fn new(
        description: impl Into<String>,
        tool: impl Into<String>,
        tool_id: impl Into<String>,
        args: HashMap<String, String>,
    ) -> Self {
        Self {
            description: description.into(),
            tool: tool.into(),
            tool_id: tool_id.into(),
            args,
            #[allow(deprecated)]
            need_approval: false,
            output: None,
        }
    }
}
