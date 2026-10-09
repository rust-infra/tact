//! View-layer contract.
//!
//! The events a View adapter renders ([`AgentUpdate`]) and the commands it
//! submits ([`UserCommand`]) are Rust view-model types, not the cross-language
//! Plugin Protocol — so they live here rather than in `tact_protocol`. The
//! Runtime stream carries the structured `tact_protocol::RuntimeEvent`
//! projection produced by [`runtime_events_for`].

use std::fmt;

use serde::{Deserialize, Serialize};
use tact_protocol::{
    ModelCallParams, PlanStep, RunId, RuntimeCommand, RuntimeEvent, StepResult,
    SubagentRunSnapshot, TaskSnapshot, TasksChangeReason, ThinkingChunk, TokenUsageInfo,
    ToolOutputChunk, ToolPresentationInfo,
};

/// Error classification — lets the TUI distinguish fatal errors (displayed as ❌ Error)
/// from non-fatal situations (shown as Info).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "message", rename_all = "snake_case")]
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

/// Status update messages sent from the Agent to the TUI.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
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
    /// reverse [`UserCommand::Runtime`] channel rather than an in-message
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
    /// Update tool-card metadata (model name, token usage, task id) without
    /// cluttering the output stream. Emitted by subagents (model / tokens, to
    /// keep the parent tool card header up to date) and by `background_run`
    /// (the id of the task it just started, which the card shows while the task
    /// keeps running after the invocation returned).
    ToolMeta {
        tool_id: String,
        model: Option<String>,
        token_usage: Option<TokenUsageInfo>,
        /// Background task started by this tool call, shown next to the card's
        /// phase so the id can be read (and polled) while the task runs.
        task_id: Option<String>,
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
    /// Client-neutral command emitted by a View adapter during migration.
    Runtime(RuntimeCommand),
}

/// Projects one View-model update into structured Runtime events.
///
/// This is the single mapping from the extension layer's view update to the
/// protocol's structured events: a View consumes the events directly, so the
/// Runtime stream never carries an opaque view-model blob.
#[must_use]
pub fn runtime_events_for(update: &AgentUpdate, run_id: Option<RunId>) -> Vec<RuntimeEvent> {
    match update {
        AgentUpdate::StreamChunk(content) => vec![RuntimeEvent::Text {
            run_id,
            role: "assistant".into(),
            content: content.clone(),
        }],
        AgentUpdate::ThinkingChunk(chunk) => vec![RuntimeEvent::Thinking {
            run_id,
            chunk: chunk.clone(),
        }],
        AgentUpdate::ToolProgress { tool_id, chunks } => vec![RuntimeEvent::ToolProgress {
            run_id,
            tool_id: tool_id.clone(),
            chunks: chunks.clone(),
        }],
        AgentUpdate::ModelInfo(params) => vec![RuntimeEvent::ModelInfo {
            run_id,
            params: params.clone(),
        }],
        AgentUpdate::TokenUsage(usage) => vec![RuntimeEvent::TokenUsage {
            run_id,
            usage: usage.clone(),
        }],
        AgentUpdate::TurnStats {
            turns_taken,
            max_turns,
        } => vec![RuntimeEvent::TurnStats {
            run_id,
            turns_taken: *turns_taken,
            max_turns: *max_turns,
        }],
        AgentUpdate::StepAdded(step) => vec![RuntimeEvent::StepAdded {
            run_id,
            step: step.clone(),
        }],
        AgentUpdate::StepStarted {
            idx,
            tool_id,
            tool_name,
            arg_summary,
            arg_full,
            presentation,
        } => vec![RuntimeEvent::StepStarted {
            run_id,
            idx: *idx,
            tool_id: tool_id.clone(),
            tool_name: tool_name.clone(),
            arg_summary: arg_summary.clone(),
            arg_full: arg_full.clone(),
            presentation: presentation.clone(),
        }],
        AgentUpdate::StepFinished {
            idx,
            tool_id,
            result,
        } => vec![RuntimeEvent::StepFinished {
            run_id,
            idx: *idx,
            tool_id: tool_id.clone(),
            result: result.clone(),
        }],
        AgentUpdate::StepFailed {
            idx,
            tool_id,
            arg_summary,
            error,
        } => vec![RuntimeEvent::StepFailed {
            run_id,
            idx: *idx,
            tool_id: tool_id.clone(),
            arg_summary: arg_summary.clone(),
            error: error.clone(),
        }],
        AgentUpdate::Error(error) => vec![RuntimeEvent::Error {
            run_id,
            message: error.to_string(),
        }],
        AgentUpdate::RequestSelect {
            request_id,
            prompt,
            options,
            log_confirm,
        } => vec![RuntimeEvent::InteractionRequested {
            request: tact_protocol::InteractionRequest::Select {
                request_id: tact_protocol::RequestId::from(request_id.to_string()),
                prompt: prompt.clone(),
                options: options.clone(),
                log_confirm: *log_confirm,
            },
        }],
        AgentUpdate::RequestMultiSelect {
            request_id,
            prompt,
            options,
        } => vec![RuntimeEvent::InteractionRequested {
            request: tact_protocol::InteractionRequest::MultiSelect {
                request_id: tact_protocol::RequestId::from(request_id.to_string()),
                prompt: prompt.clone(),
                options: options.clone(),
            },
        }],
        AgentUpdate::TaskComplete(content) => vec![
            RuntimeEvent::TaskComplete {
                run_id: run_id.clone(),
                content: content.clone(),
            },
            RuntimeEvent::RunFinished {
                run_id: run_id.unwrap_or_else(|| tact_protocol::RunId::from("runtime")),
                success: true,
            },
        ],
        AgentUpdate::TaskCancelled => vec![RuntimeEvent::Cancelled {
            run_id: run_id.unwrap_or_else(|| tact_protocol::RunId::from("runtime")),
        }],
        AgentUpdate::Info(content) => vec![RuntimeEvent::Info {
            run_id,
            content: content.clone(),
        }],
        AgentUpdate::MdInfo(content) => vec![RuntimeEvent::MdInfo {
            run_id,
            content: content.clone(),
        }],
        AgentUpdate::HookContext { source, text } => vec![RuntimeEvent::HookContext {
            run_id,
            source: source.clone(),
            text: text.clone(),
        }],
        AgentUpdate::HookStatus {
            id,
            source,
            message,
            elapsed_ms,
        } => vec![RuntimeEvent::HookStatus {
            run_id,
            id: *id,
            source: source.clone(),
            message: message.clone(),
            elapsed_ms: *elapsed_ms,
        }],
        AgentUpdate::PopupMarkdown { title, source } => vec![RuntimeEvent::PopupMarkdown {
            run_id,
            title: title.clone(),
            source: source.clone(),
        }],
        AgentUpdate::TasksChanged { tasks, reason } => vec![RuntimeEvent::TasksChanged {
            run_id,
            tasks: tasks.clone(),
            reason: *reason,
        }],
        AgentUpdate::ToolMeta {
            tool_id,
            model,
            token_usage,
            task_id,
        } => vec![RuntimeEvent::ToolMeta {
            run_id,
            tool_id: tool_id.clone(),
            model: model.clone(),
            token_usage: token_usage.clone(),
            task_id: task_id.clone(),
        }],
        AgentUpdate::BackgroundTaskFinished {
            tool_id,
            success,
            message,
            output,
        } => vec![RuntimeEvent::BackgroundTaskFinished {
            run_id,
            tool_id: tool_id.clone(),
            success: *success,
            message: message.clone(),
            output: output.clone(),
        }],
        AgentUpdate::SubagentFinished {
            tool_id,
            child_id,
            success,
            summary,
        } => vec![RuntimeEvent::SubagentFinished {
            run_id,
            tool_id: tool_id.clone(),
            child_id: child_id.clone(),
            success: *success,
            summary: summary.clone(),
        }],
        AgentUpdate::SubagentsChanged { runs } => vec![RuntimeEvent::SubagentsChanged {
            run_id,
            runs: runs.clone(),
        }],
    }
}

/// Projects protocol events back onto the legacy `AgentUpdate` view model.
///
/// The inverse of [`runtime_events_for`]. A producer that only holds the
/// protocol type — an LLM adapter, a harness — can still feed a consumer that
/// has not migrated to `RuntimeEvent` yet. It goes away with `AgentUpdate`.
pub fn runtime_event_to_agent_updates(event: tact_protocol::RuntimeEvent) -> Vec<AgentUpdate> {
    use tact_protocol::RuntimeEvent;

    match event {
        RuntimeEvent::Text { role, content, .. } if role == "assistant" => {
            vec![AgentUpdate::StreamChunk(content)]
        }
        RuntimeEvent::Thinking { chunk, .. } => vec![AgentUpdate::ThinkingChunk(chunk)],
        RuntimeEvent::ToolProgress {
            tool_id, chunks, ..
        } => vec![AgentUpdate::ToolProgress { tool_id, chunks }],
        RuntimeEvent::ModelInfo { params, .. } => vec![AgentUpdate::ModelInfo(params)],
        RuntimeEvent::TokenUsage { usage, .. } => vec![AgentUpdate::TokenUsage(usage)],
        RuntimeEvent::TurnStats {
            turns_taken,
            max_turns,
            ..
        } => vec![AgentUpdate::TurnStats {
            turns_taken,
            max_turns,
        }],
        RuntimeEvent::StepAdded { step, .. } => vec![AgentUpdate::StepAdded(step)],
        RuntimeEvent::StepStarted {
            idx,
            tool_id,
            tool_name,
            arg_summary,
            arg_full,
            presentation,
            ..
        } => vec![AgentUpdate::StepStarted {
            idx,
            tool_id,
            tool_name,
            arg_summary,
            arg_full,
            presentation,
        }],
        RuntimeEvent::StepFinished {
            idx,
            tool_id,
            result,
            ..
        } => vec![AgentUpdate::StepFinished {
            idx,
            tool_id,
            result,
        }],
        RuntimeEvent::StepFailed {
            idx,
            tool_id,
            arg_summary,
            error,
            ..
        } => vec![AgentUpdate::StepFailed {
            idx,
            tool_id,
            arg_summary,
            error,
        }],
        RuntimeEvent::TaskComplete { content, .. } => vec![AgentUpdate::TaskComplete(content)],
        RuntimeEvent::Info { content, .. } => vec![AgentUpdate::Info(content)],
        RuntimeEvent::MdInfo { content, .. } => vec![AgentUpdate::MdInfo(content)],
        RuntimeEvent::HookContext { source, text, .. } => {
            vec![AgentUpdate::HookContext { source, text }]
        }
        RuntimeEvent::HookStatus {
            id,
            source,
            message,
            elapsed_ms,
            ..
        } => vec![AgentUpdate::HookStatus {
            id,
            source,
            message,
            elapsed_ms,
        }],
        RuntimeEvent::PopupMarkdown { title, source, .. } => {
            vec![AgentUpdate::PopupMarkdown { title, source }]
        }
        RuntimeEvent::TasksChanged { tasks, reason, .. } => {
            vec![AgentUpdate::TasksChanged { tasks, reason }]
        }
        RuntimeEvent::ToolMeta {
            tool_id,
            model,
            token_usage,
            task_id,
            ..
        } => vec![AgentUpdate::ToolMeta {
            tool_id,
            model,
            token_usage,
            task_id,
        }],
        RuntimeEvent::BackgroundTaskFinished {
            tool_id,
            success,
            message,
            output,
            ..
        } => vec![AgentUpdate::BackgroundTaskFinished {
            tool_id,
            success,
            message,
            output,
        }],
        RuntimeEvent::SubagentFinished {
            tool_id,
            child_id,
            success,
            summary,
            ..
        } => vec![AgentUpdate::SubagentFinished {
            tool_id,
            child_id,
            success,
            summary,
        }],
        RuntimeEvent::SubagentsChanged { runs, .. } => vec![AgentUpdate::SubagentsChanged { runs }],
        RuntimeEvent::Cancelled { .. } => vec![AgentUpdate::TaskCancelled],
        RuntimeEvent::Error { message, .. } => {
            vec![AgentUpdate::Error(AgentErrorKind::Other(message))]
        }
        RuntimeEvent::Notification { content, .. } => vec![AgentUpdate::Info(content)],
        RuntimeEvent::InteractionRequested { request } => match request {
            tact_protocol::InteractionRequest::Select {
                request_id,
                prompt,
                options,
                log_confirm,
            } => request_id
                .as_str()
                .parse::<u64>()
                .ok()
                .map(|request_id| {
                    vec![AgentUpdate::RequestSelect {
                        request_id,
                        prompt,
                        options,
                        log_confirm,
                    }]
                })
                .unwrap_or_default(),
            tact_protocol::InteractionRequest::MultiSelect {
                request_id,
                prompt,
                options,
            } => request_id
                .as_str()
                .parse::<u64>()
                .ok()
                .map(|request_id| {
                    vec![AgentUpdate::RequestMultiSelect {
                        request_id,
                        prompt,
                        options,
                    }]
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_changed_snapshot_round_trips_fields() {
        let update = AgentUpdate::TasksChanged {
            tasks: vec![TaskSnapshot {
                id: 1,
                subject: "Fix auth".into(),
                status: tact_protocol::TaskStatusSnapshot::InProgress,
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
                assert!(matches!(reason, TasksChangeReason::Created));
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn subagents_changed_snapshot_round_trips_fields() {
        let update = AgentUpdate::SubagentsChanged {
            runs: vec![tact_protocol::SubagentRunSnapshot {
                child_id: "child-abc".into(),
                status: tact_protocol::SubagentStatusSnapshot::Completed,
                summary_first: "all done".into(),
                started_at: Some(1),
                finished_at: Some(2),
            }],
        };
        match update {
            AgentUpdate::SubagentsChanged { runs } => {
                assert_eq!(runs.len(), 1);
                assert_eq!(runs[0].child_id, "child-abc");
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn tool_progress_event_keeps_ordered_chunks() {
        use tact_protocol::ToolOutputChunk;
        let chunks = vec![
            ToolOutputChunk::stdout("out"),
            ToolOutputChunk::stderr("err"),
        ];
        let event = AgentUpdate::ToolProgress {
            tool_id: "bash-1".to_string(),
            chunks: chunks.clone(),
        };

        assert!(matches!(
            event,
            AgentUpdate::ToolProgress { tool_id, chunks: actual }
                if tool_id == "bash-1" && actual == chunks
        ));
    }

    /// `log_confirm` decides whether the View appends a "Selected: …" line
    /// after the user confirms, so dropping it in the projection silently
    /// changes what the reader sees.
    #[test]
    fn select_projection_keeps_log_confirm() {
        for log_confirm in [true, false] {
            let events = runtime_events_for(
                &AgentUpdate::RequestSelect {
                    request_id: 7,
                    prompt: "Choose".into(),
                    options: vec!["one".into(), "two".into()],
                    log_confirm,
                },
                Some(tact_protocol::RunId::from("run-1")),
            );
            match events.as_slice() {
                [
                    RuntimeEvent::InteractionRequested {
                        request:
                            tact_protocol::InteractionRequest::Select {
                                request_id,
                                prompt,
                                options,
                                log_confirm: projected,
                            },
                    },
                ] => {
                    assert_eq!(request_id.as_str(), "7");
                    assert_eq!(prompt, "Choose");
                    assert_eq!(options, &vec!["one".to_string(), "two".to_string()]);
                    assert_eq!(*projected, log_confirm, "log_confirm must survive");
                }
                other => panic!("unexpected {other:?}"),
            }
        }
    }
}
