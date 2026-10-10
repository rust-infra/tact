//! Shared runtime data types carried by Runtime events and view projections.
//!
//! The view-model types themselves (the agent-update and user-command enums)
//! live in `tact_view`; this module keeps the serializable payloads both the
//! Runtime protocol and the View contract refer to.
//!
//! State machine transitions: see [book/25_chapter_protocol_zh.md](../../book/25_chapter_protocol_zh.md).

use serde::{Deserialize, Serialize};

/// Execution status of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolVisualKind {
    #[default]
    Generic,
    FileWrite,
    FileRead,
    FileEdit,
    Command,
    Task,
    Subagent,
    Sleep,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolDetailKind {
    #[default]
    None,
    Result,
    InputField(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolPopupKind {
    #[default]
    None,
    SubagentTranscript,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolPresentationInfo {
    pub visual_kind: ToolVisualKind,
    pub display_name: String,
    pub keep_full_live_output: bool,
    pub detail: ToolDetailKind,
    pub popup: ToolPopupKind,
    pub compact_result_to_meta: bool,
    /// Keep the tool card live after `StepFinished` so later `ToolProgress`
    /// updates keep streaming and a follow-up event finalizes it. Used by
    /// fire-and-forget tools such as `background_run`, whose invocation
    /// returns immediately but whose underlying work continues.
    pub keep_live: bool,
}

impl ToolPresentationInfo {
    pub fn generic(name: impl Into<String>) -> Self {
        Self {
            visual_kind: ToolVisualKind::Generic,
            display_name: name.into(),
            keep_full_live_output: false,
            detail: ToolDetailKind::Result,
            popup: ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        }
    }
}

/// Structured result of a step execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
    /// Presentation metadata for the TUI rendering layer.
    pub presentation: ToolPresentationInfo,
}

/// Parameters for a model API call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelCallParams {
    pub model: String,
    pub max_tokens: u32,
    pub thinking_budget: Option<u32>,
    pub reasoning_effort: Option<String>,
    pub extra_body: Option<String>,
}

/// Token usage info returned from an LLM API call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

/// UI-facing task status (excludes soft-deleted records).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatusSnapshot {
    #[default]
    Pending,
    InProgress,
    Completed,
}

impl TaskStatusSnapshot {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Pending => "[ ]",
            Self::InProgress => "[>]",
            Self::Completed => "[x]",
        }
    }
}

/// Why a task-list change was emitted (`RuntimeEvent::TasksChanged`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TasksChangeReason {
    Created,
    Updated,
}

/// One non-deleted persistent task for TUI progress surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: u64,
    pub subject: String,
    pub status: TaskStatusSnapshot,
    pub session_id: String,
    pub owner: String,
    /// Task ids that this task blocks (outgoing edges for DAG).
    pub blocks: Vec<u64>,
    /// Task ids that block this task (incoming edges).
    pub blocked_by: Vec<u64>,
    pub created_at: Option<i64>,
    pub started_at: Option<i64>,
    pub completed_at: Option<i64>,
}

/// UI-facing subagent run status for the sticky 总览 (mirrors
/// [`TaskStatusSnapshot`] but keeps terminal states visible: a finished child
/// still has a summary worth showing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatusSnapshot {
    #[default]
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl SubagentStatusSnapshot {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Running => "▶",
            Self::Completed => "✓",
            Self::Failed => "✗",
            Self::Cancelled => "⏹",
        }
    }
}

/// One subagent run for the sticky total-overview strip. The snapshot is
/// scoped to runs started by the **current process** (a `SubagentManager`
/// in-memory known set) — unlike `subagent_runs` rows, which accumulate across
/// sessions and orphan-repair noise. Live detail still lives on the parent
/// `spawn_subagent` tool card / popup; this is status-level only.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubagentRunSnapshot {
    /// Child session id (the `async_launched { id }` handle).
    pub child_id: String,
    pub status: SubagentStatusSnapshot,
    /// First line of the run summary (single-line safe for the sticky body).
    pub summary_first: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
}

/// Lifecycle of a streaming thinking / reasoning block.
///
/// Producers emit `Started` once, zero or more `Delta` fragments, then `Finished`.
/// Adapters that only expose deltas (e.g. OpenAI `reasoning_content`) must synthesize
/// `Started` / `Finished` around the delta stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "text", rename_all = "snake_case")]
pub enum ThinkingChunk {
    /// A new thinking block is opening (title / region start).
    Started,
    /// Incremental reasoning text.
    Delta(String),
    /// The thinking block is complete; TUI should flush and collapse it.
    Finished,
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

#[cfg(test)]
mod tests {
    use super::{
        TaskStatusSnapshot, ToolDetailKind, ToolPopupKind, ToolPresentationInfo, ToolVisualKind,
    };

    #[test]
    fn generic_tool_presentation_has_no_native_privileges() {
        let presentation = ToolPresentationInfo::generic("mcp__demo__search");

        assert_eq!(presentation.visual_kind, ToolVisualKind::Generic);
        assert_eq!(presentation.display_name, "mcp__demo__search");
        assert_eq!(presentation.detail, ToolDetailKind::Result);
        assert_eq!(presentation.popup, ToolPopupKind::None);
        assert!(!presentation.keep_full_live_output);
        assert!(!presentation.keep_live);
        assert!(!presentation.compact_result_to_meta);
    }

    #[test]
    fn task_status_snapshot_markers() {
        assert_eq!(TaskStatusSnapshot::Pending.marker(), "[ ]");
        assert_eq!(TaskStatusSnapshot::InProgress.marker(), "[>]");
        assert_eq!(TaskStatusSnapshot::Completed.marker(), "[x]");
    }

    #[test]
    fn subagent_status_snapshot_markers() {
        use super::SubagentStatusSnapshot as S;
        assert_eq!(S::Running.marker(), "▶");
        assert_eq!(S::Completed.marker(), "✓");
        assert_eq!(S::Failed.marker(), "✗");
        assert_eq!(S::Cancelled.marker(), "⏹");
    }
}
