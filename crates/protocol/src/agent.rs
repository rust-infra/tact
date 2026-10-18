//! Agent–TUI protocol types.
//!
//! These messages flow between the agent runtime and the terminal UI:
//! execution status updates, user commands, step results, token usage, errors,
//! and streaming output.
//!
//! State machine transitions: see [book/25_chapter_protocol_zh.md](../../book/25_chapter_protocol_zh.md).

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

/// UI-facing Goal lifecycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalStatusSnapshot {
    #[default]
    Draft,
    Queued,
    Running,
    WaitingUser,
    WaitingPermission,
    Paused,
    Verifying,
    Succeeded,
    Failed,
    Cancelled,
}

impl GoalStatusSnapshot {
    pub fn marker(self) -> &'static str {
        match self {
            Self::Draft => "◇",
            Self::Queued => "◷",
            Self::Running => "▶",
            Self::WaitingUser | Self::WaitingPermission => "?",
            Self::Paused => "Ⅱ",
            Self::Verifying => "◎",
            Self::Succeeded => "✓",
            Self::Failed => "✗",
            Self::Cancelled => "⏹",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed | Self::Cancelled)
    }
}

/// UI-facing Goal execution phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalPhaseSnapshot {
    #[default]
    Planning,
    Execution,
    Verification,
}

/// UI-facing acceptance-criterion status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoalCriterionStatusSnapshot {
    #[default]
    Pending,
    Passed,
    Rejected,
}

/// One acceptance criterion projected for protocol consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GoalCriterionSnapshot {
    pub id: String,
    pub description: String,
    pub required: bool,
    pub human: bool,
    pub status: GoalCriterionStatusSnapshot,
    pub evidence_count: usize,
}

/// Aggregate acceptance counts shown in compact Goal surfaces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GoalAcceptanceSnapshot {
    pub required: usize,
    pub passed: usize,
    pub pending: usize,
    pub rejected: usize,
}

/// Budget usage for the Goal's current activation.
///
/// `GoalSnapshot::run_budget` is `Some` exactly while an activation is in
/// flight (the store's active-run rows are the source), and `None` when the
/// Goal has no run: a `running` Goal without a budget is idle, waiting for the
/// next explicit resume, and surfaces draw it as such.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GoalRunBudgetSnapshot {
    pub turns_used: u32,
    pub turn_limit: u32,
    pub tool_calls_used: u32,
    pub elapsed_ms: u64,
    pub wall_clock_limit_ms: u64,
}

/// Full durable Goal projection. `GoalsChanged` always replaces the previous
/// collection so reconnect/recovery never depends on missed deltas.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GoalSnapshot {
    pub id: String,
    pub title: String,
    pub objective: String,
    pub status: GoalStatusSnapshot,
    pub phase: GoalPhaseSnapshot,
    pub criteria: Vec<GoalCriterionSnapshot>,
    pub task_total: usize,
    pub task_completed: usize,
    pub wait_reason: Option<String>,
    pub acceptance: GoalAcceptanceSnapshot,
    pub run_budget: Option<GoalRunBudgetSnapshot>,
    /// Filed out of the default list / sticky panel (see
    /// [`UserCommand::SetGoalArchived`]). The status is unchanged, so a surface
    /// that does show an archived Goal still reports how it finished.
    #[serde(default)]
    pub archived: bool,
}

/// UI-facing subagent run status for the sticky 总览 (mirrors
/// [`TaskStatusSnapshot`] but keeps terminal states visible: a finished child
/// still has a summary worth showing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
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
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SubagentRunSnapshot {
    /// Child session id (the `async_launched { id }` handle).
    pub child_id: String,
    pub status: SubagentStatusSnapshot,
    /// First line of the run summary (single-line safe for the sticky body).
    pub summary_first: String,
    pub started_at: Option<i64>,
    pub finished_at: Option<i64>,
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
    /// One structured entry of a subagent's transcript, addressed to the
    /// parent `spawn_subagent` card's popup.
    ///
    /// Emitted by the child's tagged forwarder **alongside** the flattened
    /// [`Self::ToolProgress`] chunk for the same event: the flat text still
    /// feeds the card's live preview and the popup's copy fallback, while this
    /// carries the structure the popup renders. The flat stream cannot be
    /// reversed into structure, which is why both exist.
    ToolTranscript {
        /// Parent-side tool id of the `spawn_subagent` card this belongs to.
        tool_id: String,
        entry: crate::subagent_transcript::SubagentTranscriptEntry,
    },
    /// The assistant text accumulated so far in the child's current turn.
    ///
    /// Sent while the text still streams, so the popup grows like the main log
    /// instead of staying empty until the turn ends. The payload is the **whole
    /// turn's** text, not a delta: it replaces the previous in-flight assistant
    /// body, and it is redacted whole rather than chunk by chunk — a secret
    /// split across two `StreamChunk`s would slip past a per-chunk scan.
    /// When the turn ends it is replaced by the final [`Self::ToolTranscript`]
    /// `Assistant` entry, identical once nothing was redacted.
    ToolTranscriptStream { tool_id: String, text: String },
    /// The entire task is complete
    TaskComplete(String),
    /// The in-flight task was cancelled by the user. TUI must leave
    /// `Planning` / `Executing` so a new prompt can be submitted.
    /// Emitted by the command driver after `agent_loop` returns with
    /// `cancel_flag` set — not by `agent_loop` itself.
    TaskCancelled,
    /// An out-of-band turn (a Goal activation) finished without a completion
    /// payload. TUI must leave `Planning` / `Executing` so queued messages are
    /// flushed instead of waiting on a busy state that no `TaskComplete` will
    /// ever clear. Emitted by the command driver after `GoalRunner::run_once`.
    TaskIdle,
    /// Agent error, with classification for the TUI to decide display style
    Error(AgentErrorKind),
    /// Token usage stats
    TokenUsage(TokenUsageInfo),
    /// Agent-loop turn counter for the current task, emitted once per loop
    /// iteration (one LLM call). `max_turns` is the loop cap when the agent
    /// has one (`Agent::max_turns`; `None` = unbounded, which is the case for
    /// the main agent — only subagents set it today).
    TurnStats {
        turns_taken: u32,
        max_turns: Option<u32>,
    },
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
    /// Hook-injected context, delivered whole (one shot) and **labelled**.
    ///
    /// Same body rendering as [`Self::MdInfo`], different provenance: a hook's
    /// stdout is neither a user turn nor Tact's own notice. The message is
    /// recorded with `<hook-context>` framing, but the TUI strips that before
    /// rendering, so without a label the reader cannot tell a plugin's briefing
    /// from a system message Tact wrote itself. `source` names the hook that
    /// produced the text (`plugin codex`, `~/.tact/hooks.json`) when the caller
    /// knows it; the label itself is the TUI's to draw.
    HookContext {
        source: Option<String>,
        text: String,
    },
    /// A plugin hook's own progress line, with a lifetime.
    ///
    /// A plugin declares `statusMessage` because its hook is a subprocess that
    /// can take seconds (a cold `uv run --script` has taken ~100s on this
    /// machine): the reader has to see that something is running, and has to see
    /// it stop claiming that once the hook returned. The row is keyed by `id`
    /// and rewritten in place — never removed, because removing a log row
    /// shifts every physical index the TUI keys selection and cards on.
    ///
    /// `elapsed_ms` is the state: `None` while the hook runs, the measured time
    /// once it returned (success or failure — the line is a trace either way).
    HookStatus {
        id: u64,
        source: Option<String>,
        message: String,
        elapsed_ms: Option<u64>,
    },
    /// Pre-rendered Markdown to show in the modal popup instead of the
    /// transcript, headed by `title`.
    ///
    /// Same rendering as [`Self::MdInfo`], different destination: a read-out you
    /// open, read and dismiss (`/stats`, `/background`) should not push the
    /// conversation off the log. The title travels with the body because the
    /// producer also owns the text — the TUI does not have to know which command
    /// it is answering.
    PopupMarkdown { title: String, source: String },

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
        /// True when the request came from a `spawn_subagent` child rather than
        /// the main agent. The popup says whose permission is being asked
        /// ("Select from subagent"); the prompt body stays clean, so the marker
        /// is a title and not a line the model's own text could imitate.
        from_subagent: bool,
    },
    /// Request user to choose **zero or more** options (Space toggles, Enter confirms).
    /// Used by `ask_user` when `multi_select` is true. Does not affect [`RequestSelect`].
    RequestMultiSelect {
        request_id: u64,
        prompt: String,
        options: Vec<String>,
        /// See [`Self::RequestSelect`]'s `from_subagent`.
        from_subagent: bool,
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
    /// Full Goal snapshot after creation, lifecycle mutation, recovery, or an
    /// explicit list/show request.
    GoalsChanged { goals: Vec<GoalSnapshot> },
    /// Update tool-card metadata (model name, token usage, task id) without
    /// cluttering the output stream. Emitted by subagents (model / tokens, to
    /// keep the parent tool card header up to date) and by `background_run`
    /// (the id of the task it just started, which the card shows while the task
    /// keeps running after the invocation returned).
    ToolMeta {
        tool_id: String,
        model: Option<String>,
        token_usage: Option<TokenUsageInfo>,
        /// Context window of the agent that sent this, so a subagent card can
        /// render the same `ctx {pct}% {used}/{window}` meter as the main bar
        /// instead of a bare token total. `None` from senders with no window to
        /// report (e.g. `background_run`), which leaves the meter off.
        context_window: Option<usize>,
        /// Background task started by this tool call, shown next to the card's
        /// phase so the id can be read (and polled) while the task runs.
        task_id: Option<String>,
        /// Child session id of the subagent this tool call spawned. Sent by
        /// `spawn_subagent` itself (the id is minted by the tool, before the
        /// child exists), so every subagent card — synchronous or background —
        /// carries the same handle the child's metadata arrives under. The card
        /// shows its short form, matching the sticky Subagent overview.
        subagent_id: Option<String>,
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
    /// The set of subagent runs started by the current process changed
    /// (a spawn started / a sync or async child finished / a cancel was
    /// requested). `runs` is the full visible snapshot for the TUI sticky
    /// strip (Running first, then newest-finished, capped). The subagent
    /// analog of [`Self::TasksChanged`]; read-only tools do not emit.
    SubagentsChanged { runs: Vec<SubagentRunSnapshot> },
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
    /// Create a durable Goal. Empty `criteria` lets the driver install its
    /// default human-acceptance criterion for interactive use.
    CreateGoal {
        title: String,
        objective: String,
        criteria: Vec<String>,
    },
    /// List every durable Goal as a full replacement snapshot.
    ///
    /// `include_archived` is the difference between "my working set" (the
    /// sticky panel) and "everything this workspace ever recorded": an archived
    /// Goal is finished and filed, so it leaves the default list rather than
    /// deleting its record.
    ListGoals { include_archived: bool },
    /// Show one Goal while still refreshing the full replacement snapshot.
    ShowGoal { goal_id: String },
    /// Persist user steering text in the Goal event stream.
    ///
    /// `goal_id` is `None` when the user omitted it; the driver resolves the
    /// target against the store (the single running Goal, or a leading
    /// id/prefix in `text`).
    SteerGoal {
        goal_id: Option<String>,
        text: String,
    },
    /// Pause a non-terminal Goal after any active activation releases it.
    PauseGoal { goal_id: String },
    /// Resume a paused/waiting Goal and start one bounded activation.
    ResumeGoal { goal_id: String },
    /// Cancel a non-terminal Goal after any active activation releases it.
    CancelGoal { goal_id: String },
    /// Record explicit human acceptance and resume one bounded activation.
    AcceptGoal { goal_id: String, text: String },
    /// File a finished Goal out of the panel (`archived: true`), or bring it
    /// back (`false`). Not a lifecycle change: the Goal keeps its status.
    SetGoalArchived { goal_id: String, archived: bool },
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
    /// Cancel a running background subagent (triggered by `/subagent_cancel`
    /// or the TUI tool-card cancel button). The driver flips the child's
    /// cooperative cancel flag and marks its run record Cancelled.
    CancelSubagent { child_id: String },
    /// Run the interactive OAuth authorization flow for a remote MCP server
    /// (triggered by `/mcp auth <server>`). On success the driver reloads the
    /// MCP router so the server becomes usable without restarting.
    McpAuth { server: String },
    /// List the configured MCP servers with their live status (triggered by
    /// `/mcp list`). The driver renders it from the agent's **already-connected**
    /// router, so it never reconnects and cannot disturb in-flight work.
    McpList,
    /// List the prompts connected servers publish (triggered by
    /// `/mcp prompts [server]`). Read-only; the driver answers it from the
    /// agent's **already-connected** router, like [`UserCommand::McpList`].
    McpPrompts { server: Option<String> },
    /// Fetch one MCP prompt and run it (triggered by
    /// `/mcp prompt <server> <name> [key=value …]`). The driver owns the fetch
    /// because only it can see the router, and submits the rendered messages
    /// through the ordinary task path — a prompt is a starting message, not a
    /// new turn shape. An empty `name` is refused by the driver, which is where
    /// "no such prompt" can also be said.
    RunMcpPrompt {
        server: String,
        name: String,
        arguments: std::collections::BTreeMap<String, String>,
    },
    /// List every configured command hook with its review status (triggered by
    /// `/hooks list`). Read-only; the driver answers it because only the Tact
    /// crate can read the hook sources and the review store.
    HooksList,
    /// Approve hooks that are waiting for review (triggered by
    /// `/hooks trust --all` or `/hooks trust --source <label>`). One of the two
    /// is required — approving a hook by accident is the failure the review
    /// step exists to prevent.
    HooksTrust { all: bool, source: Option<String> },
    /// Revoke every hook approval (triggered by `/hooks forget --all`).
    HooksForget,
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
        AgentUpdate, GoalAcceptanceSnapshot, GoalCriterionSnapshot, GoalCriterionStatusSnapshot,
        GoalPhaseSnapshot, GoalRunBudgetSnapshot, GoalSnapshot, GoalStatusSnapshot, TaskSnapshot,
        TaskStatusSnapshot, TasksChangeReason, ToolDetailKind, ToolPopupKind, ToolPresentationInfo,
        ToolVisualKind, UserCommand,
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

    #[test]
    fn goals_changed_carries_a_full_serializable_snapshot() {
        let goal = GoalSnapshot {
            id: "goal-1".into(),
            title: "Ship Goal UI".into(),
            objective: "Expose supervised goal execution".into(),
            status: GoalStatusSnapshot::WaitingUser,
            phase: GoalPhaseSnapshot::Verification,
            criteria: vec![GoalCriterionSnapshot {
                id: "criterion-1".into(),
                description: "User accepts the result".into(),
                required: true,
                human: true,
                status: GoalCriterionStatusSnapshot::Pending,
                evidence_count: 0,
            }],
            task_total: 3,
            task_completed: 2,
            wait_reason: Some("needs acceptance".into()),
            acceptance: GoalAcceptanceSnapshot {
                required: 1,
                passed: 0,
                pending: 1,
                rejected: 0,
            },
            run_budget: Some(GoalRunBudgetSnapshot {
                turns_used: 2,
                turn_limit: 8,
                tool_calls_used: 5,
                elapsed_ms: 100,
                wall_clock_limit_ms: 60_000,
            }),
            archived: false,
        };
        let encoded = serde_json::to_string(&goal).unwrap();
        let decoded: GoalSnapshot = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, goal);

        let update = AgentUpdate::GoalsChanged {
            goals: vec![goal.clone()],
        };
        match update {
            AgentUpdate::GoalsChanged { goals } => assert_eq!(goals, vec![goal]),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn goal_commands_keep_ids_and_user_text_typed() {
        let commands = [
            UserCommand::CreateGoal {
                title: "Ship it".into(),
                objective: "Finish the feature".into(),
                criteria: vec!["Tests pass".into()],
            },
            UserCommand::ListGoals {
                include_archived: false,
            },
            UserCommand::ShowGoal {
                goal_id: "goal-1".into(),
            },
            UserCommand::SteerGoal {
                goal_id: Some("goal-1".into()),
                text: "Prefer the smaller design".into(),
            },
            UserCommand::PauseGoal {
                goal_id: "goal-1".into(),
            },
            UserCommand::ResumeGoal {
                goal_id: "goal-1".into(),
            },
            UserCommand::CancelGoal {
                goal_id: "goal-1".into(),
            },
            UserCommand::AcceptGoal {
                goal_id: "goal-1".into(),
                text: "Accepted after manual review".into(),
            },
        ];

        assert!(matches!(
            commands[1],
            UserCommand::ListGoals {
                include_archived: false
            }
        ));
        assert!(matches!(
            &commands[3],
            UserCommand::SteerGoal { goal_id, text }
                if goal_id.as_deref() == Some("goal-1") && text == "Prefer the smaller design"
        ));
        assert!(matches!(
            &commands[7],
            UserCommand::AcceptGoal { goal_id, text }
                if goal_id == "goal-1" && text == "Accepted after manual review"
        ));
    }

    #[test]
    fn subagents_changed_snapshot_round_trips_fields() {
        use super::{SubagentRunSnapshot, SubagentStatusSnapshot};
        let update = AgentUpdate::SubagentsChanged {
            runs: vec![SubagentRunSnapshot {
                child_id: "child-abc".into(),
                status: SubagentStatusSnapshot::Completed,
                summary_first: "all done".into(),
                started_at: Some(1),
                finished_at: Some(2),
            }],
        };
        match update {
            AgentUpdate::SubagentsChanged { runs } => {
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].child_id, "child-abc");
                assert_eq!(runs[0].status, SubagentStatusSnapshot::Completed);
                assert_eq!(runs[0].summary_first, "all done");
            }
            other => panic!("unexpected {other:?}"),
        }
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
