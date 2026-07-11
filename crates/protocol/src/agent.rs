//! Agent–TUI protocol types.
//!
//! These messages flow between the agent runtime and the terminal UI:
//! execution status updates, user commands, step results, token usage, errors,
//! and streaming output.

use std::fmt;

use serde::{Deserialize, Serialize};
use tokio::sync::oneshot;

/// Execution status of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// Generic error (catch-all)
    Other(String),
}

impl fmt::Display for AgentErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentErrorKind::Other(msg) => f.write_str(msg),
        }
    }
}

impl std::error::Error for AgentErrorKind {}

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
    /// This is a subset of `completion` exposed by the usage object's
    /// `completion_tokens_details.reasoning_tokens` field.
    pub reasoning_tokens: u32,
}

/// Status update messages sent from the Agent to the TUI.
#[derive(Debug)]
pub enum AgentUpdate {
    /// Dynamically append a step to the existing plan (does not reset selection state)
    StepAdded(PlanStep),
    /// A step has started execution.
    StepStarted {
        idx: usize,
        tool_id: String,
        tool_name: String,
        arg_summary: String,
        /// Full tool argument summary (untruncated), used by detailed UI views.
        arg_full: String,
    },
    /// A step succeeded, with structured result.
    StepFinished {
        idx: usize,
        tool_id: String,
        result: StepResult,
    },
    /// A step failed, with error message.
    StepFailed {
        idx: usize,
        tool_id: String,
        error: String,
    },
    /// The entire task is complete
    TaskComplete(String),
    /// Agent error, with classification for the TUI to decide display style
    Error(AgentErrorKind),
    /// Token usage stats
    TokenUsage(TokenUsageInfo),
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
    /// Tool name, e.g. `read_file` / `write_file` / `run_command`
    pub tool: String,
    /// LLM-assigned tool-use id from the assistant message.
    #[serde(default)]
    pub tool_id: String,
    /// Tool arguments as sent by the model (order-preserving, lossless JSON).
    #[serde(default)]
    pub args: serde_json::Map<String, serde_json::Value>,
    /// Output after execution (populated by TUI; defaults to None on JSON deserialization)
    #[serde(default)]
    pub output: Option<String>,
}

impl PlanStep {
    /// Construct a plan step for the streaming agent loop.
    pub fn new<I, K, V>(
        description: impl Into<String>,
        tool: impl Into<String>,
        tool_id: impl Into<String>,
        args: I,
    ) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<serde_json::Value>,
    {
        Self {
            description: description.into(),
            tool: tool.into(),
            tool_id: tool_id.into(),
            args: args
                .into_iter()
                .map(|(k, v)| (k.into(), v.into()))
                .collect(),
            output: None,
        }
    }
}
