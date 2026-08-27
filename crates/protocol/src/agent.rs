//! Agent–TUI protocol types.
//!
//! These messages flow between the agent runtime and the terminal UI:
//! execution status updates, user commands, step results, token usage, errors,
//! and streaming output.
//!
//! State machine transitions: see [book/25_chapter_protocol.md](../../book/25_chapter_protocol.md).

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::tool_output::ToolOutputChunk;

/// Execution status of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepStatus {
    Success,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum ToolDetailKind {
    #[default]
    None,
    Result,
    InputField(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolPopupKind {
    #[default]
    None,
    SubagentTranscript,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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
    /// Presentation metadata for the TUI rendering layer.
    pub presentation: ToolPresentationInfo,
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

/// UI-facing task status (excludes soft-deleted records).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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

/// Why a [`AgentUpdate::TasksChanged`] was emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TasksChangeReason {
    Created,
    Updated,
}

/// One non-deleted persistent task for TUI progress surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
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
        /// Presentation metadata for the TUI rendering layer.
        presentation: ToolPresentationInfo,
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
        /// Tool argument summary (e.g. the web-search query) so a failed
        /// card keeps a distinguishable title even when the failure arrives
        /// without one. The TUI falls back to the `StepStarted` summary when
        /// this is empty.
        arg_summary: String,
        error: String,
    },
    /// Incremental text produced while a tool invocation is still running.
    ToolProgress {
        tool_id: String,
        chunks: Vec<ToolOutputChunk>,
    },
    /// The entire task is complete
    TaskComplete(String),
    /// The in-flight task was cancelled by the user. TUI must leave
    /// `Planning` / `Executing` so a new prompt can be submitted.
    /// Emitted by the command driver after `agent_loop` returns with
    /// `cancel_flag` set — not by `agent_loop` itself.
    TaskCancelled,
    /// Agent error, with classification for the TUI to decide display style
    Error(AgentErrorKind),
    /// Token usage stats
    TokenUsage(TokenUsageInfo),
    /// Model call parameters (name, max_tokens, thinking budget, etc.)
    ModelInfo(ModelCallParams),
    /// Informational notice (does not change state)
    Info(String),
    /// Markdown-formatted informational notice, delivered whole (one shot).
    ///
    /// Rendered by the TUI as a single Markdown cell (headings / lists /
    /// tables / fenced code keep their formatting), unlike [`Info`] which is
    /// treated as short single-line system text.
    MdInfo(String),
    /// Session statistics (triggered by the /stats command)
    SessionStats(String),

    /// Request user to choose **one** option; returns option index (None = cancelled).
    /// Used by permission prompts and single-choice `ask_user`.
    ///
    /// The request carries a unique `request_id`; the TUI answers over the
    /// reverse [`UserCommand::UiResponse`] channel rather than an in-message
    /// oneshot sender, so [`AgentUpdate`] no longer carries a transport handle
    /// (pure data, transport-agnostic).
    RequestSelect {
        request_id: u64,
        prompt: String,
        options: Vec<String>,
        /// When true, TUI appends a "Selected: …" system line after confirm.
        /// Permission prompts keep this `false` (choice already shown on the tool meta row).
        log_confirm: bool,
    },
    /// Request user to choose **zero or more** options (Space toggles, Enter confirms).
    /// Used by `ask_user` when `multi_select` is true. Does not affect [`RequestSelect`].
    RequestMultiSelect {
        request_id: u64,
        prompt: String,
        options: Vec<String>,
    },
    /// Streaming output text fragment (appended to Log in real time)
    StreamChunk(String),
    /// Streaming thinking / reasoning lifecycle event
    ThinkingChunk(ThinkingChunk),
    /// Persistent task list changed (`task_create` / `task_update`).
    /// `tasks` excludes soft-deleted records.
    TasksChanged {
        tasks: Vec<TaskSnapshot>,
        reason: TasksChangeReason,
    },
    /// Update tool-card metadata (model name, token usage) without
    /// cluttering the output stream. Emitted by subagents to keep
    /// the parent tool card header up to date.
    ToolMeta {
        tool_id: String,
        model: Option<String>,
        token_usage: Option<TokenUsageInfo>,
    },
    /// Finalize a tool card that stayed live after its invocation returned
    /// (see [`ToolPresentationInfo::keep_live`]). Emitted by background tasks
    /// when the underlying process finishes.
    BackgroundTaskFinished {
        tool_id: String,
        /// `true` when the command exited successfully.
        success: bool,
        /// One-line summary (e.g. `Background task 018f3a2c completed`).
        message: String,
        /// Final combined stdout+stderr output (already capped).
        output: String,
    },
    /// Finalize a subagent tool card after a `run_in_background` child finishes.
    /// The subagent analog of [`Self::BackgroundTaskFinished`]; the TUI reuses
    /// the same "finalize a keep-live card" path but must also carry over the
    /// full transcript into the popup.
    ///
    /// Emitted on the **parent** `ui_tx` (never the child's tagged forwarder,
    /// which drops unknown variants).
    SubagentFinished {
        tool_id: String,
        /// Child session id (the `async_launched { id }` handle).
        child_id: String,
        /// `true` when the child completed successfully.
        success: bool,
        /// One-line summary; the full transcript stays in the popup.
        summary: String,
    },
}

/// Lifecycle of a streaming thinking / reasoning block.
///
/// Producers emit `Started` once, zero or more `Delta` fragments, then `Finished`.
/// Adapters that only expose deltas (e.g. OpenAI `reasoning_content`) must synthesize
/// `Started` / `Finished` around the delta stream.
#[derive(Debug, Clone)]
pub enum ThinkingChunk {
    /// A new thinking block is opening (title / region start).
    Started,
    /// Incremental reasoning text.
    Delta(String),
    /// The thinking block is complete; TUI should flush and collapse it.
    Finished,
}

/// Response to a UI request, sent from the TUI back to the agent runtime over
/// the reverse channel ([`UserCommand::UiResponse`]).
///
/// Carries the [`AgentUpdate::RequestSelect`] / [`AgentUpdate::RequestMultiSelect`]
/// `request_id` so the runtime can route it to the waiting caller. Keeping this
/// separate from the request enum (rather than embedding a oneshot sender) is
/// what lets [`AgentUpdate`] stay pure data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiResponse {
    /// Answer a single-choice request; `choice` is `None` when cancelled.
    Select {
        request_id: u64,
        choice: Option<usize>,
    },
    /// Answer a multi-choice request; `choices` is `None` when cancelled.
    MultiSelect {
        request_id: u64,
        choices: Option<Vec<usize>>,
    },
}

impl UiResponse {
    /// The request this response answers.
    pub fn request_id(&self) -> u64 {
        match self {
            UiResponse::Select { request_id, .. } => *request_id,
            UiResponse::MultiSelect { request_id, .. } => *request_id,
        }
    }
}

/// User commands sent from the TUI to the Agent.
#[derive(Debug)]
pub enum UserCommand {
    /// Submit a new natural-language task
    SubmitTask(String),
    /// Cancel the current in-flight task by setting `cancel_flag`.
    /// The agent loop exits cooperatively at the next check point and does not
    /// emit `TaskComplete`. The command driver emits [`AgentUpdate::TaskCancelled`]
    /// so the TUI can leave the busy state. The next `SubmitTask` clears the flag.
    Cancel,
    /// Compact the session history (triggered by `/compact` slash command).
    /// Runs compaction on the existing context and stops — does not start a
    /// new task.
    Compact,
    /// Query account balance (DeepSeek/Kimi)
    QueryBalance,
    /// Query session statistics (triggered by the /stats command)
    QueryStats,
    /// Query background task status (triggered by the `/background` slash
    /// command). `None` lists all tasks one line per task; `Some(id)` shows a
    /// single task as pretty JSON.
    QueryBackground(Option<String>),
    /// Set the active permission mode.
    /// The TUI sends this after the user picks through the `/permission` popup.
    /// Only affects the in-memory session; config is never written.
    SetPermissionMode(String),
    /// Set the active session's thinking budget for subsequent LLM requests.
    /// The TUI sends this after `/model` budget confirmation; config persistence
    /// is a separate optional local flow.
    SetThinkingBudget(usize),
    /// Set the active agent session's reasoning effort (openai / deepseek / kimi k3).
    /// `Some("low"|"medium"|...)` sets it; `None` clears (wire omits effort).
    /// The TUI sends this after `/model` effort confirmation; config persistence
    /// is a separate optional local flow.
    SetReasoningEffort(Option<String>),
    /// Set the active agent session's model (per-agent, not global).
    /// The TUI sends this after `/model` model confirmation.
    SetModel(String),
    /// A background subagent finished while the parent may be idle. The driver
    /// decides whether to drop it (parent is mid-turn — the result is already
    /// in `pending_subagent_results`) or submit a synthetic wake-up turn.
    SubagentFinishedNotification {
        child_id: String,
        summary: String,
        success: bool,
    },
    /// Answer a pending [`AgentUpdate::RequestSelect`] / [`RequestMultiSelect`]
    /// (see [`UiResponse`]). Routed by the driver to the shared responder.
    UiResponse(UiResponse),
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
        AgentUpdate, TaskSnapshot, TaskStatusSnapshot, TasksChangeReason, ToolDetailKind,
        ToolPopupKind, ToolPresentationInfo, ToolVisualKind,
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
    fn tasks_changed_snapshot_round_trips_fields() {
        let update = AgentUpdate::TasksChanged {
            tasks: vec![TaskSnapshot {
                id: 1,
                subject: "Fix auth".into(),
                status: TaskStatusSnapshot::InProgress,
                owner: String::new(),
                blocks: vec![2],
                blocked_by: vec![],
                ..Default::default()
            }],
            reason: TasksChangeReason::Created,
        };
        match update {
            AgentUpdate::TasksChanged { tasks, reason } => {
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].id, 1);
                assert_eq!(tasks[0].status, TaskStatusSnapshot::InProgress);
                assert!(matches!(reason, TasksChangeReason::Created));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn task_status_snapshot_markers() {
        assert_eq!(TaskStatusSnapshot::Pending.marker(), "[ ]");
        assert_eq!(TaskStatusSnapshot::InProgress.marker(), "[>]");
        assert_eq!(TaskStatusSnapshot::Completed.marker(), "[x]");
    }
}
