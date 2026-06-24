// Agent core module
// Receives user tasks, calls the OpenAI API to generate execution plans,
// and executes them step by step inside a sandbox.
// Communicates with the TUI module over channels, reporting execution status in real time.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tokio::sync::oneshot;

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
    pub status: StepStatus,
    pub message: String,
    /// Additional details, e.g. full content of a written file or raw command output.
    pub detail: Option<String>,
    /// Tool execution duration in milliseconds. None for non-tool steps.
    pub duration_ms: Option<u64>,
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
    /// Plan generated, with list of steps
    PlanGenerated(Vec<PlanStep>),
    /// Dynamically append a step to the existing plan (does not reset selection state)
    StepAdded(PlanStep),
    /// Step `idx` has started execution
    StepStarted(usize),
    /// Step `idx` succeeded, with structured result
    StepFinished(usize, StepResult),
    /// Step `idx` failed, with error message
    StepFailed(usize, String),
    /// Requires user approval: prompt text, step index, approval channel (true=accept, false=reject)
    NeedApproval(String, usize, oneshot::Sender<bool>),
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
        /// This is a subset of `completion` exposed via the usage object's
        /// `completion_tokens_details.reasoning_tokens` field.
        reasoning_tokens: u32,
    },
    /// Account balance info (DeepSeek only)
    Balance(BalanceInfo),
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
    /// Query account balance (DeepSeek only)
    QueryBalance,
}

/// A single step in the execution plan.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanStep {
    /// Human-readable step description
    pub description: String,
    /// Tool name: read_file / write_file / run_command
    pub tool: String,
    /// Tool arguments (key-value pairs)
    pub args: HashMap<String, String>,
    /// Whether user manual approval is required before execution
    pub need_approval: bool,
    /// Output after execution (populated by TUI; defaults to None on JSON deserialization)
    #[serde(default)]
    pub output: Option<String>,
}

/// A single currency entry in DeepSeek account balance info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceEntry {
    /// Currency type: CNY or USD
    pub currency: String,
    /// Total available balance (granted + topped up)
    pub total_balance: String,
    /// Unexpired granted balance
    pub granted_balance: String,
    /// Topped-up balance
    pub topped_up_balance: String,
}

/// DeepSeek account balance query result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BalanceInfo {
    /// Whether the account has available balance
    pub is_available: bool,
    /// Per-currency balance details
    pub balance_infos: Vec<BalanceEntry>,
}

// Format a byte count using human-readable units: Byte, K, M, G.
//
// Uses 1024 as the unit base. Values below 1024 are shown as
// `"<n> Byte"`; larger values are scaled to the largest fitting unit
// with one decimal place and trailing ".0" removed.
pub fn format_bytes(bytes: usize) -> String {
    const UNITS: &[&str] = &["Byte", "K", "M", "G"];

    if bytes < 1024 {
        return format!("{} Byte", bytes);
    }

    let mut size = bytes as f64;
    let mut unit_index = 0;
    while size >= 1024.0 && unit_index < UNITS.len() - 1 {
        size /= 1024.0;
        unit_index += 1;
    }

    let formatted = format!("{:.1}", size);
    if formatted.ends_with(".0") {
        format!(
            "{} {}",
            &formatted[..formatted.len() - 2],
            UNITS[unit_index]
        )
    } else {
        format!("{} {}", formatted, UNITS[unit_index])
    }
}

#[cfg(test)]
mod tests {
    use super::format_bytes;

    #[test]
    fn format_bytes_units() {
        assert_eq!(format_bytes(0), "0 Byte");
        assert_eq!(format_bytes(1023), "1023 Byte");
        assert_eq!(format_bytes(1024), "1 K");
        assert_eq!(format_bytes(1536), "1.5 K");
        assert_eq!(format_bytes(1024 * 1024), "1 M");
        assert_eq!(format_bytes(1024 * 1024 * 1024), "1 G");
        assert_eq!(format_bytes(1024 * 1024 * 1024 * 3), "3 G");
    }
}
