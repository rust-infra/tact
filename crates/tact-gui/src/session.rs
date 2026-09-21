//! Bridge between the headless agent session and the GPUI shell.
//!
//! `tact-session` owns the agent and its command driver. This module owns the
//! presentation half of the contract: it starts a session, hands the shell a
//! command sender, and folds [`AgentUpdate`]s into the transcript model. No
//! GPUI type crosses into the headless crates and no protocol type reaches the
//! row renderer — [`Conversation`] is the seam in both directions.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use gpui_kit::Task;
use tact_protocol::{
    AgentUpdate, ModelCallParams, PlanStep, StepStatus, SubagentRunSnapshot, TaskSnapshot,
    ThinkingChunk, TokenUsageInfo, ToolOutputChunk, ToolVisualKind, UserCommand,
};
use tact_session::{
    HistoryBlock, HistoryMessage, HistoryRole, RecentSession, SessionOptions, SessionRuntime,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::pane::DiffEntry;
use crate::transcript::{ToolStatus, TranscriptRow, diff_line_counts};

/// Commands and identity for one running session.
pub(crate) struct SessionHandle {
    session_id: String,
    commands: UnboundedSender<UserCommand>,
}

impl SessionHandle {
    /// Wrap the driver's command channel with the session's identity.
    pub(crate) fn new(session_id: String, commands: UnboundedSender<UserCommand>) -> Self {
        Self {
            session_id,
            commands,
        }
    }

    /// Session id the agent writes history to.
    pub(crate) fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Queue a natural-language task.
    ///
    /// Returns `false` once the driver has ended, so the caller can tell the
    /// user instead of waiting for output that will never arrive.
    pub(crate) fn submit(&self, task: String) -> bool {
        self.commands.send(UserCommand::SubmitTask(task)).is_ok()
    }

    /// Ask the in-flight turn to stop at its next checkpoint.
    pub(crate) fn cancel(&self) {
        let _ = self.commands.send(UserCommand::Cancel);
    }

    /// Send any other protocol command (model, effort, permission, …).
    pub(crate) fn send(&self, command: UserCommand) {
        let _ = self.commands.send(command);
    }
}

/// The event streams the shell drains on its own executor.
///
/// Tokio channels are executor-agnostic: GPUI polls these directly instead of
/// the shell owning a runtime of its own.
pub(crate) struct SessionStreams {
    pub(crate) events: UnboundedReceiver<AgentUpdate>,
    pub(crate) account: UnboundedReceiver<tact_protocol::AccountUpdate>,
}

/// Start a fresh session for `workdir`.
///
/// Returns as soon as the local session row exists. Agent construction (LLM
/// client, MCP handshake, hooks) is deferred onto the session thread and
/// reports failure as `AgentUpdate::Error` on [`SessionStreams::events`], so a
/// broken provider can never prevent the window from opening.
pub(crate) fn start(workdir: PathBuf) -> anyhow::Result<(SessionHandle, SessionStreams)> {
    from_runtime(SessionRuntime::start(SessionOptions::new(workdir))?)
}

/// Reopen `session_id` for `workdir`.
///
/// Resume is id reuse: the agent reloads that session's stored conversation
/// while it is built, so the reopened session continues the same thread.
pub(crate) fn resume(
    workdir: PathBuf,
    session_id: String,
) -> anyhow::Result<(SessionHandle, SessionStreams)> {
    from_runtime(SessionRuntime::start(SessionOptions::resume(
        workdir, session_id,
    ))?)
}

/// Split a started runtime into the front end's handle and event streams.
fn from_runtime(runtime: SessionRuntime) -> anyhow::Result<(SessionHandle, SessionStreams)> {
    let handle = SessionHandle::new(runtime.session_id().to_string(), runtime.commands.clone());
    let streams = SessionStreams {
        events: runtime.events,
        account: runtime.account,
    };
    Ok((handle, streams))
}

/// Recent sessions for `workdir`, newest first.
///
/// A session list is never worth blocking a window on: an unreadable store
/// yields an empty list rather than an error the shell would have to render.
pub(crate) fn recent(workdir: &std::path::Path) -> Vec<RecentSession> {
    match tact_session::sessions::recent(workdir) {
        Ok(sessions) => sessions,
        Err(err) => {
            tracing::warn!("cannot list sessions for {}: {err:#}", workdir.display());
            Vec::new()
        }
    }
}

/// A session's persisted conversation, oldest first.
///
/// Reopening a session must redraw what it already said instead of starting on
/// a blank page. A store that cannot be read yields an empty history rather
/// than an error the shell would have to render, the same bargain [`recent`]
/// makes for the session list.
pub(crate) fn history(workdir: &std::path::Path, session_id: &str) -> Vec<HistoryMessage> {
    match tact_session::history::history(workdir, session_id) {
        Ok(messages) => messages,
        Err(err) => {
            tracing::warn!("cannot read session {session_id} history: {err:#}");
            Vec::new()
        }
    }
}

/// Name a session in the workspace's store, or clear its name with `""`.
pub(crate) fn rename(
    workdir: &std::path::Path,
    session_id: &str,
    name: &str,
) -> anyhow::Result<()> {
    tact_session::session_actions::rename(workdir, session_id, name)
}

/// Set or clear a session's archive flag. This never deletes.
pub(crate) fn set_archived(
    workdir: &std::path::Path,
    session_id: &str,
    archived: bool,
) -> anyhow::Result<()> {
    tact_session::session_actions::set_archived(workdir, session_id, archived)
}

/// Copy a session's conversation into a new session and return its id.
pub(crate) fn duplicate(workdir: &std::path::Path, session_id: &str) -> anyhow::Result<String> {
    tact_session::session_actions::duplicate(workdir, session_id)
}

/// Open the workspace in the desktop's file manager.
pub(crate) fn reveal(workdir: &std::path::Path) -> anyhow::Result<()> {
    tact_session::session_actions::reveal(workdir)
}

/// Short label for a session id: the first UUID segment.
pub(crate) fn short_id(session_id: &str) -> &str {
    tact_session::sessions::short_id(session_id)
}

/// Current Unix time in seconds.
pub(crate) fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or_default()
}

/// Coarse age text. Precision past a day is noise in a session list.
pub(crate) fn age_label(seconds: i64) -> String {
    match seconds {
        ..=59 => "just now".to_string(),
        s if s < 3600 => format!("{}m ago", s / 60),
        s if s < 86_400 => format!("{}h ago", s / 3600),
        s => format!("{}d ago", s / 86_400),
    }
}

/// What the scroller needs to know after an update lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Change {
    /// Nothing the scroller tracks changed.
    None,
    /// `count` rows were appended to the end of the transcript.
    Appended(usize),
    /// Row `index` changed height and must be remeasured.
    Resized(usize),
}

/// A pending single- or multi-choice request from the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Request {
    /// Protocol request id, echoed back on the response.
    pub(crate) id: u64,
    /// Prompt shown above the options.
    pub(crate) prompt: String,
    /// Option labels in wire order.
    pub(crate) options: Vec<String>,
    /// `true` for `RequestMultiSelect`; Space toggles instead of Enter.
    pub(crate) multi: bool,
    /// Indices the user has toggled (multi-select only).
    pub(crate) selected: Vec<usize>,
}

/// Non-transcript session state: what the work panes, composer, and status bar
/// read. Kept separate from [`Conversation`] so pane rendering never walks the
/// transcript.
#[derive(Default)]
pub(crate) struct SessionState {
    /// Plan steps in arrival order.
    pub(crate) plan: Vec<PlanStep>,
    /// When each plan step was first seen to finish, keyed by step index.
    ///
    /// Keyed by index rather than kept beside the step so a plan replaced
    /// wholesale (the design preview) simply has no stamps, instead of leaving
    /// a parallel vector to fall out of step with the list.
    pub(crate) plan_done_at: HashMap<usize, i64>,
    /// Plan steps the user expanded in the work pane.
    pub(crate) plan_expanded: HashSet<usize>,
    /// Plan steps whose tool reported a failure.
    ///
    /// `PlanStep::output` records that a step ran but not whether it succeeded;
    /// retry needs that distinction, so the fold records failed step indices
    /// separately from the human-readable output.
    pub(crate) plan_failed: HashSet<usize>,
    /// Latest persistent task snapshot.
    pub(crate) tasks: Vec<TaskSnapshot>,
    /// Latest subagent run snapshot.
    pub(crate) subagents: Vec<SubagentRunSnapshot>,
    /// Most recent token usage for the usage ring.
    pub(crate) usage: Option<TokenUsageInfo>,
    /// Turns taken in the current task, and the loop cap when one exists.
    pub(crate) turns: Option<(u32, Option<u32>)>,
    /// Model parameters from the last request.
    pub(crate) model: Option<ModelCallParams>,
    /// The agent has a turn in flight.
    pub(crate) running: bool,
    /// A blocking choice the agent is waiting on. Answering it appends a
    /// [`TranscriptRow::Approval`] row, so the record keeps the slot it was
    /// asked in instead of hanging off the tail of the list.
    pub(crate) request: Option<Request>,
    /// Latest balance / quota update, when the provider supports one.
    pub(crate) account: Option<tact_protocol::AccountUpdate>,
    /// Workspace the session is scoped to; the Files pane roots itself here.
    pub(crate) workdir: Option<PathBuf>,
    /// Current git branch for the workspace, when one can be resolved.
    pub(crate) branch: Option<String>,
    /// In-memory permission mode shown by the composer and status bar.
    pub(crate) permission_mode: String,
    /// File changes recorded from write and edit tool results.
    pub(crate) diff: Vec<DiffEntry>,
    /// Commands still running in the background.
    ///
    /// A keep-live tool card stays open past its invocation, so it is exactly
    /// the "background command" shape the sidebar lists; the entry is removed
    /// when `BackgroundTaskFinished` or a final `StepFinished` seals the card.
    pub(crate) background: Vec<String>,
}

/// The transcript: rows plus the indices needed to append to live ones.
///
/// Protocol events carry tool and request ids, while the scroller indexes rows.
/// Tracking the open tool/thinking/assistant rows here keeps that mapping out
/// of both the renderer and the shell.
#[derive(Default)]
pub(crate) struct Conversation {
    rows: Vec<TranscriptRow>,
    open_tools: HashMap<String, usize>,
    open_thinking: Option<usize>,
    open_assistant: Option<usize>,
    /// When the live reasoning block started, so `Finished` can stamp it with
    /// the elapsed seconds the summary shows.
    thinking_started_at: Option<std::time::Instant>,
}

impl Conversation {
    /// The rows, in display order.
    pub(crate) fn rows(&self) -> &[TranscriptRow] {
        &self.rows
    }

    /// Number of rows.
    pub(crate) fn len(&self) -> usize {
        self.rows.len()
    }

    /// Replace the transcript with a session's persisted history.
    ///
    /// Blocks replay in stored order, so a redrawn transcript reads
    /// `thinking -> tool -> answer` the way the turn produced it, and a tool
    /// card takes the slot of its own `ToolUse` block.
    ///
    /// Only what was stored can come back: a redrawn card has no duration, no
    /// live output tail and the producer's visual kind was never persisted, so
    /// it falls back to the generic one. The store keeps no per-message
    /// timestamp, so redrawn rows carry [`NO_TIMESTAMP`] and print no clock.
    /// A result fills the card its own `ToolUse` opened, which is also the only
    /// evidence the store keeps that the tool finished.
    pub(crate) fn load_history(&mut self, messages: &[HistoryMessage]) -> usize {
        self.rows.clear();
        self.open_tools.clear();
        self.open_thinking = None;
        self.open_assistant = None;
        self.thinking_started_at = None;

        let answered: HashSet<&str> = messages
            .iter()
            .flat_map(|message| message.blocks.iter())
            .filter_map(|block| match block {
                HistoryBlock::ToolResult { tool_use_id, .. } => Some(tool_use_id.as_str()),
                _ => None,
            })
            .collect();

        let mut tools: HashMap<&str, usize> = HashMap::new();
        for message in messages {
            for block in &message.blocks {
                match block {
                    HistoryBlock::Text(text) => {
                        let text = text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        match message.role {
                            HistoryRole::User => {
                                self.push_row(TranscriptRow::User {
                                    text: text.to_string(),
                                    sent_at: NO_TIMESTAMP,
                                });
                            }
                            HistoryRole::Assistant => {
                                self.push_row(TranscriptRow::Assistant {
                                    markdown: text.to_string(),
                                    streaming: false,
                                    sent_at: NO_TIMESTAMP,
                                    model: None,
                                });
                            }
                        }
                    }
                    HistoryBlock::Thinking(text) => {
                        let text = text.trim();
                        if text.is_empty() {
                            continue;
                        }
                        self.push_row(TranscriptRow::Thinking {
                            text: text.to_string(),
                            duration_seconds: None,
                            expanded: true,
                        });
                    }
                    HistoryBlock::ToolUse { id, name, detail } => {
                        // A card whose turn ended without a result is left
                        // running, exactly as the live transcript left it.
                        let status = if answered.contains(id.as_str()) {
                            ToolStatus::Succeeded
                        } else {
                            ToolStatus::Running
                        };
                        let index = self.push_row(TranscriptRow::Tool {
                            display_name: name.clone(),
                            detail: first_line(detail),
                            output: String::new(),
                            duration: String::new(),
                            status,
                            expanded: false,
                            visual_kind: ToolVisualKind::default(),
                            diff_stats: None,
                        });
                        tools.insert(id.as_str(), index);
                    }
                    HistoryBlock::ToolResult {
                        tool_use_id,
                        output,
                    } => {
                        let Some(&index) = tools.get(tool_use_id.as_str()) else {
                            continue;
                        };
                        if let Some(TranscriptRow::Tool {
                            output: stored,
                            status,
                            ..
                        }) = self.rows.get_mut(index)
                        {
                            stored.push_str(output.trim());
                            retain_output_tail(stored);
                            *status = ToolStatus::Succeeded;
                        }
                    }
                }
            }
        }
        self.rows.len()
    }

    /// Start a new turn: seal live rows so the next chunk opens fresh ones.
    fn end_turn(&mut self) {
        self.seal_open_assistant();
        self.open_thinking = None;
        self.open_tools.clear();
    }

    /// Stop appending to the open assistant row; the next chunk opens its own.
    ///
    /// A thinking or tool row is a real boundary in the transcript, so prose
    /// that arrives after one belongs *below* it. Without this the chunk would
    /// be appended to the row that opened before the card, which reads as the
    /// answer jumping above the tool it came from. The terminal client has the
    /// same rule: it flushes pending prose before allocating a card, and later
    /// prose appends at the tail.
    ///
    /// A prompt queued mid-turn deliberately does *not* seal ([`Self::push_user`]).
    fn seal_open_assistant(&mut self) {
        if let Some(index) = self.open_assistant.take()
            && let Some(TranscriptRow::Assistant { streaming, .. }) = self.rows.get_mut(index)
        {
            *streaming = false;
        }
    }

    /// Append the user's own message.
    ///
    /// Sealing live rows is the *turn end*'s job ([`Self::end_turn`] on
    /// `TaskComplete`), not the submit's: a prompt queued mid-turn must not
    /// close the assistant row that is still streaming.
    /// Append a fully-formed row.
    ///
    /// The design preview and layout tests own the row shape directly rather
    /// than feeding it through an agent event stream.
    pub(crate) fn push_row(&mut self, row: TranscriptRow) -> usize {
        self.rows.push(row);
        self.rows.len() - 1
    }

    pub(crate) fn push_user(&mut self, text: String) -> usize {
        self.rows.push(TranscriptRow::User {
            text,
            sent_at: now_unix(),
        });
        self.rows.len()
    }

    /// Append a completed assistant turn.
    ///
    /// The live path builds assistant rows from [`super::AgentUpdate`] traffic;
    /// this seeds the same row shape for previews and tests without pretending
    /// the text is still streaming.
    pub(crate) fn push_assistant(&mut self, markdown: String) -> usize {
        self.rows.push(TranscriptRow::Assistant {
            markdown,
            streaming: false,
            sent_at: now_unix(),
            model: None,
        });
        self.rows.len()
    }

    /// Append a local system note (startup failures, cancelled turns).
    pub(crate) fn push_system(&mut self, text: String) -> usize {
        self.rows.push(TranscriptRow::System { text });
        self.rows.len()
    }

    /// Append an answered permission or question card.
    ///
    /// The card is a row rather than a slot beside the list, so it keeps the
    /// slot it was asked in: the prototype's `.approval.done` still reads as a
    /// record, but the transcript keeps moving past it instead of leaving the
    /// answered card as the last thing on screen for the rest of the session.
    pub(crate) fn push_approval(&mut self, request: Request, result: String) -> usize {
        self.rows.push(TranscriptRow::Approval { request, result });
        self.rows.len()
    }

    /// Fold one protocol update into the transcript and session state.
    pub(crate) fn apply(&mut self, update: AgentUpdate, state: &mut SessionState) -> Change {
        match update {
            AgentUpdate::StreamChunk(text) => {
                let model = state.model.as_ref().map(|params| params.model.clone());
                self.append_stream(text, model)
            }
            AgentUpdate::ThinkingChunk(chunk) => self.apply_thinking(chunk),
            AgentUpdate::StepAdded(step) => {
                state.plan.push(step.clone());
                // The transcript shows a placeholder as soon as a step is
                // planned, so the plan pane and the transcript cannot disagree
                // about how many steps exist.
                if step.tool_id.is_empty() {
                    Change::None
                } else {
                    // A planned step carries no presentation yet; the kind
                    // arrives with `StepStarted`.
                    self.ensure_tool(
                        &step.tool_id,
                        &step.tool,
                        &step.description,
                        ToolStatus::Running,
                        None,
                        None,
                    )
                }
            }
            AgentUpdate::StepStarted {
                tool_id,
                tool_name,
                arg_summary,
                presentation,
                ..
            } => {
                let name = display_name(&presentation.display_name, &tool_name);
                let detail = if arg_summary.is_empty() {
                    name.clone()
                } else {
                    arg_summary.clone()
                };
                // A keep-live card outlives its invocation, which is what makes
                // it background work rather than an ordinary completed step.
                if presentation.keep_live && !arg_summary.is_empty() {
                    state.background.push(arg_summary.clone());
                    state.background.dedup();
                }
                self.set_tool(
                    &tool_id,
                    name,
                    detail,
                    ToolStatus::Running,
                    String::new(),
                    Some(presentation.visual_kind),
                    None,
                )
            }
            AgentUpdate::StepFinished {
                tool_id, result, ..
            } => {
                let status = match result.status {
                    StepStatus::Success => ToolStatus::Succeeded,
                    StepStatus::Failed => ToolStatus::Failed,
                };
                let detail = first_line(&result.arg_summary);
                let duration = format_duration(result.duration_us);
                // File writes and edits are the only steps whose *result* is a
                // change worth re-reading later, so they feed the Diff pane.
                let mut diff_stats = None;
                if matches!(
                    result.presentation.visual_kind,
                    ToolVisualKind::FileWrite | ToolVisualKind::FileEdit
                ) && let Some(changed) = result
                    .detail
                    .as_ref()
                    .filter(|text| !text.trim().is_empty())
                {
                    diff_stats = diff_line_counts(changed);
                    let entry = DiffEntry::new(
                        result
                            .arg_full
                            .clone()
                            .unwrap_or_else(|| result.arg_summary.clone()),
                        changed.clone(),
                    );
                    state.diff.push(match diff_stats {
                        Some((added, removed)) => entry.with_stats(added, removed),
                        None => entry,
                    });
                }
                // The plan pane only ever pushed steps; without this it would
                // claim every step is still pending after the tool ran.
                let plan_output = if result.message.trim().is_empty() {
                    detail.clone()
                } else {
                    result.message.clone()
                };
                mark_plan_step(
                    state,
                    &tool_id,
                    plan_output,
                    result.status == StepStatus::Failed,
                );
                self.set_tool(
                    &tool_id,
                    display_name(&result.presentation.display_name, &result.tool),
                    detail,
                    status,
                    duration,
                    Some(result.presentation.visual_kind),
                    diff_stats,
                )
            }
            AgentUpdate::StepFailed {
                tool_id,
                arg_summary,
                error,
                ..
            } => {
                let detail = if arg_summary.is_empty() {
                    first_line(&error)
                } else {
                    format!("{} — {}", first_line(&arg_summary), first_line(&error))
                };
                mark_plan_step(state, &tool_id, error.clone(), true);
                self.set_tool(
                    &tool_id,
                    self.tool_name(&tool_id).unwrap_or_else(|| "Tool".into()),
                    detail,
                    ToolStatus::Failed,
                    String::new(),
                    None,
                    None,
                )
            }
            AgentUpdate::ToolProgress { tool_id, chunks } => {
                self.apply_tool_progress(&tool_id, &chunks)
            }
            AgentUpdate::ToolMeta {
                tool_id,
                model,
                token_usage,
            } => {
                if let Some(usage) = token_usage {
                    state.usage = Some(usage);
                }
                if let Some(model) = model {
                    let detail = self
                        .tool_detail(&tool_id)
                        .map(|detail| format!("{detail} · {model}"))
                        .unwrap_or(model);
                    self.set_tool(
                        &tool_id,
                        self.tool_name(&tool_id).unwrap_or_else(|| "Tool".into()),
                        detail,
                        self.tool_status(&tool_id).unwrap_or(ToolStatus::Running),
                        self.tool_duration(&tool_id).unwrap_or_default(),
                        None,
                        None,
                    )
                } else {
                    Change::None
                }
            }
            AgentUpdate::BackgroundTaskFinished {
                tool_id,
                success,
                message,
                output,
            } => {
                let sealed = self.tool_name(&tool_id);
                if let Some(name) = sealed.as_deref()
                    && let Some(position) = state.background.iter().position(|cmd| cmd == name)
                {
                    state.background.remove(position);
                }
                let status = if success {
                    ToolStatus::Succeeded
                } else {
                    ToolStatus::Failed
                };
                let detail = if output.trim().is_empty() {
                    first_line(&message)
                } else {
                    format!("{} — {}", first_line(&message), last_line(&output))
                };
                self.set_tool(
                    &tool_id,
                    self.tool_name(&tool_id)
                        .unwrap_or_else(|| "Background task".into()),
                    detail,
                    status,
                    self.tool_duration(&tool_id).unwrap_or_default(),
                    None,
                    None,
                )
            }
            AgentUpdate::SubagentFinished {
                tool_id,
                success,
                summary,
                ..
            } => {
                let status = if success {
                    ToolStatus::Succeeded
                } else {
                    ToolStatus::Failed
                };
                self.set_tool(
                    &tool_id,
                    self.tool_name(&tool_id)
                        .unwrap_or_else(|| "Subagent".into()),
                    first_line(&summary),
                    status,
                    self.tool_duration(&tool_id).unwrap_or_default(),
                    None,
                    None,
                )
            }
            AgentUpdate::TasksChanged { tasks, .. } => {
                state.tasks = tasks;
                Change::None
            }
            AgentUpdate::SubagentsChanged { runs } => {
                state.subagents = runs;
                Change::None
            }
            AgentUpdate::TokenUsage(usage) => {
                state.usage = Some(usage);
                Change::None
            }
            AgentUpdate::TurnStats {
                turns_taken,
                max_turns,
            } => {
                state.turns = Some((turns_taken, max_turns));
                Change::None
            }
            AgentUpdate::ModelInfo(params) => {
                state.model = Some(params);
                Change::None
            }
            AgentUpdate::TaskComplete(summary) => {
                self.end_turn();
                state.running = false;
                let index = self.rows.len();
                self.rows.push(TranscriptRow::System {
                    text: if summary.trim().is_empty() {
                        "Task complete.".to_string()
                    } else {
                        format!("Task complete — {}", first_line(&summary))
                    },
                });
                Change::Appended(self.rows.len() - index)
            }
            AgentUpdate::TaskCancelled => {
                self.end_turn();
                state.running = false;
                let index = self.rows.len();
                self.rows.push(TranscriptRow::System {
                    text: "Cancelled the active turn.".to_string(),
                });
                Change::Appended(self.rows.len() - index)
            }
            AgentUpdate::Error(error) => {
                self.end_turn();
                state.running = false;
                let index = self.rows.len();
                self.rows.push(TranscriptRow::Error {
                    text: error.to_string(),
                });
                Change::Appended(self.rows.len() - index)
            }
            AgentUpdate::Info(message) => {
                let index = self.rows.len();
                self.rows.push(TranscriptRow::System { text: message });
                Change::Appended(self.rows.len() - index)
            }
            AgentUpdate::MdInfo(markdown) | AgentUpdate::SessionStats(markdown) => {
                let index = self.rows.len();
                self.rows.push(TranscriptRow::Assistant {
                    markdown,
                    streaming: false,
                    sent_at: now_unix(),
                    model: None,
                });
                Change::Appended(self.rows.len() - index)
            }
            AgentUpdate::RequestSelect {
                request_id,
                prompt,
                options,
                ..
            } => {
                state.request = Some(Request {
                    id: request_id,
                    prompt,
                    options,
                    multi: false,
                    selected: Vec::new(),
                });
                Change::None
            }
            AgentUpdate::RequestMultiSelect {
                request_id,
                prompt,
                options,
            } => {
                state.request = Some(Request {
                    id: request_id,
                    prompt,
                    options,
                    multi: true,
                    selected: Vec::new(),
                });
                Change::None
            }
        }
    }

    /// Append streamed assistant text, opening a row when needed.
    fn append_stream(&mut self, text: String, model: Option<String>) -> Change {
        let index = match self.open_assistant {
            Some(index)
                if matches!(self.rows.get(index), Some(TranscriptRow::Assistant { .. })) =>
            {
                index
            }
            _ => {
                let index = self.rows.len();
                self.rows.push(TranscriptRow::Assistant {
                    markdown: String::new(),
                    streaming: true,
                    sent_at: now_unix(),
                    model,
                });
                self.open_assistant = Some(index);
                index
            }
        };
        if let Some(TranscriptRow::Assistant { markdown, .. }) = self.rows.get_mut(index) {
            markdown.push_str(&text);
        }
        Change::Resized(index)
    }

    /// Fold a thinking lifecycle event into a collapsible reasoning row.
    fn apply_thinking(&mut self, chunk: ThinkingChunk) -> Change {
        match chunk {
            ThinkingChunk::Started => {
                self.seal_open_assistant();
                let index = self.rows.len();
                self.rows.push(TranscriptRow::Thinking {
                    text: String::new(),
                    duration_seconds: None,
                    expanded: true,
                });
                self.thinking_started_at = Some(std::time::Instant::now());
                self.open_thinking = Some(index);
                Change::Appended(1)
            }
            ThinkingChunk::Delta(text) => {
                let index = match self.open_thinking {
                    Some(index) => index,
                    None => {
                        self.seal_open_assistant();
                        let index = self.rows.len();
                        self.rows.push(TranscriptRow::Thinking {
                            text: String::new(),
                            duration_seconds: None,
                            expanded: true,
                        });
                        self.thinking_started_at = Some(std::time::Instant::now());
                        self.open_thinking = Some(index);
                        index
                    }
                };
                if let Some(TranscriptRow::Thinking { text: body, .. }) = self.rows.get_mut(index) {
                    body.push_str(&text);
                }
                Change::Resized(index)
            }
            ThinkingChunk::Finished => {
                // Stamp the sealed row with what it cost. `Instant` gives
                // sub-second resolution; a block that finished inside the
                // first second still reads as "Thought for 1s" rather than
                // claiming zero.
                let elapsed = self
                    .thinking_started_at
                    .take()
                    .map(|started| started.elapsed().as_secs().max(1));
                if let Some(index) = self.open_thinking
                    && let Some(TranscriptRow::Thinking {
                        duration_seconds, ..
                    }) = self.rows.get_mut(index)
                {
                    *duration_seconds = elapsed;
                }
                self.open_thinking = None;
                Change::None
            }
        }
    }

    /// Stream tool output into the matching row's detail line.
    fn apply_tool_progress(&mut self, tool_id: &str, chunks: &[ToolOutputChunk]) -> Change {
        let Some(latest) = chunks
            .iter()
            .rev()
            .map(|chunk| chunk.text.trim())
            .find(|text| !text.is_empty())
            .map(last_line)
        else {
            return Change::None;
        };
        let index = match self.open_tools.get(tool_id).copied() {
            Some(index) => index,
            None => return Change::None,
        };
        if let Some(TranscriptRow::Tool { detail, output, .. }) = self.rows.get_mut(index) {
            *detail = latest;
            for chunk in chunks {
                output.push_str(&chunk.text);
            }
            retain_output_tail(output);
        }
        Change::Resized(index)
    }

    /// Create a tool row for `tool_id` if this is the first mention of it.
    fn ensure_tool(
        &mut self,
        tool_id: &str,
        name: &str,
        detail: &str,
        status: ToolStatus,
        kind: Option<ToolVisualKind>,
        diff_stats: Option<(u32, u32)>,
    ) -> Change {
        if self.open_tools.contains_key(tool_id) {
            return Change::None;
        }
        // The card takes the position of its first appearance and keeps it:
        // every later update writes into this same row.
        self.seal_open_assistant();
        let index = self.rows.len();
        self.rows.push(TranscriptRow::Tool {
            display_name: name.to_string(),
            detail: first_line(detail),
            output: String::new(),
            duration: String::new(),
            status,
            expanded: false,
            visual_kind: kind.unwrap_or_default(),
            diff_stats,
        });
        self.open_tools.insert(tool_id.to_string(), index);
        Change::Appended(1)
    }

    /// Update an existing tool row, creating it when the id is new.
    #[allow(clippy::too_many_arguments)]
    fn set_tool(
        &mut self,
        tool_id: &str,
        name: String,
        detail: String,
        status: ToolStatus,
        duration: String,
        kind: Option<ToolVisualKind>,
        diff_stats: Option<(u32, u32)>,
    ) -> Change {
        let index = match self.open_tools.get(tool_id).copied() {
            Some(index) => index,
            None => return self.ensure_tool(tool_id, &name, &detail, status, kind, diff_stats),
        };
        if let Some(TranscriptRow::Tool {
            display_name,
            detail: current,
            duration: current_duration,
            status: current_status,
            visual_kind: current_kind,
            diff_stats: current_stats,
            ..
        }) = self.rows.get_mut(index)
        {
            *display_name = name;
            *current = detail;
            *current_status = status;
            *current_duration = duration;
            if let Some(kind) = kind {
                *current_kind = kind;
            }
            if diff_stats.is_some() {
                *current_stats = diff_stats;
            }
        }
        Change::Resized(index)
    }

    /// Display name of a tool row, if it exists.
    fn tool_name(&self, tool_id: &str) -> Option<String> {
        let index = *self.open_tools.get(tool_id)?;
        match self.rows.get(index) {
            Some(TranscriptRow::Tool { display_name, .. }) => Some(display_name.clone()),
            _ => None,
        }
    }

    /// Flip the expanded state of one collapsible row.
    ///
    /// Returns whether the row exists and is collapsible, so the caller knows
    /// whether it has to remeasure.
    pub(crate) fn toggle_expanded(&mut self, index: usize) -> bool {
        match self.rows.get_mut(index) {
            Some(TranscriptRow::Tool { expanded, .. }) => {
                *expanded = !*expanded;
                true
            }
            // The prototype's reasoning card is a `<details>` too: clicking
            // its summary collapses the body without changing the transcript
            // detail level.
            Some(TranscriptRow::Thinking { expanded, .. }) => {
                *expanded = !*expanded;
                true
            }
            _ => false,
        }
    }

    /// Open a tool row and return its transcript index.
    ///
    /// Plan rows link back to the tool card that ran them. The card may have
    /// been collapsed automatically when it finished, so the jump also opens
    /// it rather than merely scrolling to a summary.
    pub(crate) fn reveal_tool(&mut self, tool_id: &str) -> Option<usize> {
        let index = *self.open_tools.get(tool_id)?;
        if let Some(TranscriptRow::Tool { expanded, .. }) = self.rows.get_mut(index) {
            *expanded = true;
        }
        Some(index)
    }

    /// Detail line of a tool row, if it exists.
    fn tool_detail(&self, tool_id: &str) -> Option<String> {
        let index = *self.open_tools.get(tool_id)?;
        match self.rows.get(index) {
            Some(TranscriptRow::Tool { detail, .. }) => Some(detail.clone()),
            _ => None,
        }
    }

    /// Status of a tool row, if it exists.
    fn tool_status(&self, tool_id: &str) -> Option<ToolStatus> {
        let index = *self.open_tools.get(tool_id)?;
        match self.rows.get(index) {
            Some(TranscriptRow::Tool { status, .. }) => Some(*status),
            _ => None,
        }
    }

    /// Duration already recorded for a tool row, if any.
    fn tool_duration(&self, tool_id: &str) -> Option<String> {
        let index = *self.open_tools.get(tool_id)?;
        match self.rows.get(index) {
            Some(TranscriptRow::Tool { duration, .. }) => Some(duration.clone()),
            _ => None,
        }
    }

    /// The transcript as Markdown, for the toolbar's copy button.
    ///
    /// Every row the scroller shows is represented: prose and Markdown pass
    /// through unchanged, while the cards that only exist as UI (thinking, tool
    /// summaries) become block quotes and list items so a pasted transcript
    /// still reads as a conversation rather than a wall of text.
    pub(crate) fn to_markdown(&self) -> String {
        let mut blocks: Vec<String> = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            match row {
                TranscriptRow::User { text, .. } => blocks.push(text.trim().to_string()),
                TranscriptRow::Assistant { markdown, .. } => {
                    blocks.push(markdown.trim().to_string());
                }
                TranscriptRow::Thinking { text, .. } => blocks.push(quoted(text)),
                TranscriptRow::Tool {
                    display_name,
                    detail,
                    output,
                    duration,
                    status,
                    ..
                } => blocks.push(tool_markdown(
                    display_name,
                    detail,
                    output,
                    duration,
                    *status,
                )),
                TranscriptRow::System { text } | TranscriptRow::Error { text } => {
                    blocks.push(quoted(text));
                }
                TranscriptRow::Approval { request, result } => {
                    blocks.push(quoted(&format!("{} -> {result}", request.prompt.trim())));
                }
            }
        }
        blocks.retain(|block| !block.is_empty());
        blocks.join("\n\n")
    }
}

/// Quote `text` as a Markdown block quote, one `>` per line.
fn quoted(text: &str) -> String {
    text.trim()
        .lines()
        .map(|line| {
            if line.is_empty() {
                ">".to_string()
            } else {
                format!("> {line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One tool card as a Markdown list item plus its output in a fence.
fn tool_markdown(
    display_name: &str,
    detail: &str,
    output: &str,
    duration: &str,
    status: ToolStatus,
) -> String {
    let state = match status {
        ToolStatus::Running => "running",
        ToolStatus::Succeeded => "succeeded",
        ToolStatus::Failed => "failed",
    };
    let meta = if duration.is_empty() {
        state.to_string()
    } else {
        format!("{state} · {duration}")
    };
    let mut item = if detail.trim().is_empty() {
        format!("- **{display_name}** ({meta})")
    } else {
        format!("- **{display_name}** `{}` ({meta})", detail.trim())
    };
    let output = output.trim();
    if !output.is_empty() {
        item.push_str("\n\n```\n");
        item.push_str(output);
        item.push_str("\n```");
    }
    item
}

/// Record that the tool behind a plan step finished.
///
/// The GUI only ever appended plan steps, so the pane claimed every step was
/// still pending after its tool had already run. The latest terminal event
/// owns the visible result, and the pane times the step from the first stamp.
fn mark_plan_step(state: &mut SessionState, tool_id: &str, output: String, failed: bool) {
    let Some((index, step)) = state
        .plan
        .iter_mut()
        .enumerate()
        .find(|(_, step)| !step.tool_id.is_empty() && step.tool_id == tool_id)
    else {
        return;
    };
    step.output = Some(output);
    state.plan_done_at.entry(index).or_insert_with(now_unix);
    if failed {
        state.plan_failed.insert(index);
    } else {
        state.plan_failed.remove(&index);
    }
}

/// Marker for a row that has no wall-clock stamp of its own.
///
/// A row redrawn from stored history knows what was said but not when: the
/// store keeps no per-message timestamp. Zero is not a real Unix second here —
/// [`crate::transcript`] prints no clock for it — so it cannot be mistaken for
/// the epoch.
pub(crate) const NO_TIMESTAMP: i64 = 0;

/// Bytes of tool output kept per row.
const TOOL_OUTPUT_LIMIT: usize = 8 * 1024;

/// Keep only the tail of a tool card's output.
///
/// The card shows a 150px window, but a long-running command can stream
/// megabytes; a row that remembered all of it would cost memory out of all
/// proportion to anything the card can display.
fn retain_output_tail(output: &mut String) {
    if output.len() <= TOOL_OUTPUT_LIMIT {
        return;
    }
    let excess = output.len() - TOOL_OUTPUT_LIMIT;
    let start = output
        .char_indices()
        .map(|(index, _)| index)
        .find(|index| *index >= excess)
        .unwrap_or(output.len());
    *output = output[start..].to_string();
}

/// Prefer the producer's display name, falling back to the raw tool name.
fn display_name(display: &str, tool: &str) -> String {
    if display.is_empty() {
        tool.to_string()
    } else {
        display.to_string()
    }
}

/// A tool row shows one line: collapse any newlines and trim.
fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// The most recent non-empty line of a progress or output block.
fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// Tool durations are microseconds on the wire; show the precision that
/// matters at each scale rather than four digits of noise.
fn format_duration(duration_us: Option<u64>) -> String {
    match duration_us {
        None => String::new(),
        Some(us) if us >= 1_000_000 => format!("{:.1} s", us as f64 / 1_000_000.0),
        Some(us) if us >= 1_000 => format!("{} ms", us / 1_000),
        Some(us) => format!("{us} µs"),
    }
}

/// The GPUI task that drains a session's event stream.
///
/// Stored by the shell so the pump is cancelled when the window closes.
pub(crate) type Pump = Task<()>;

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{ToolPresentationInfo, ToolVisualKind};

    fn presentation(name: &str) -> ToolPresentationInfo {
        ToolPresentationInfo {
            visual_kind: ToolVisualKind::Generic,
            display_name: name.to_string(),
            keep_full_live_output: false,
            detail: tact_protocol::ToolDetailKind::Result,
            popup: tact_protocol::ToolPopupKind::None,
            compact_result_to_meta: false,
            keep_live: false,
        }
    }

    #[test]
    fn stream_chunks_accumulate_into_one_row() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(AgentUpdate::StreamChunk("Hello".into()), &mut state);
        let change = conversation.apply(AgentUpdate::StreamChunk(" world".into()), &mut state);

        assert_eq!(conversation.len(), 1);
        assert_eq!(change, Change::Resized(0));
        // `sent_at` is the wall clock the row opened, so the shape assertion
        // covers the fields this test is about.
        let [
            TranscriptRow::Assistant {
                markdown,
                streaming,
                ..
            },
        ] = conversation.rows()
        else {
            panic!("one assistant row: {:?}", conversation.rows());
        };
        assert_eq!(markdown, "Hello world");
        assert!(streaming, "the row is still streaming");
    }

    #[test]
    fn a_new_turn_streams_into_its_own_row_after_the_last_one_ends() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(AgentUpdate::StreamChunk("one".into()), &mut state);
        conversation.apply(AgentUpdate::TaskComplete("one".into()), &mut state);

        conversation.push_user("again".into());
        conversation.apply(AgentUpdate::StreamChunk("two".into()), &mut state);

        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Assistant { streaming: false, markdown, .. } if markdown == "one"
        ));
        let last = conversation.rows().last().expect("second answer");
        assert!(matches!(
            last,
            TranscriptRow::Assistant { streaming: true, markdown, .. } if markdown == "two"
        ));
    }

    #[test]
    fn a_queued_user_row_does_not_seal_the_open_stream() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(AgentUpdate::StreamChunk("one".into()), &mut state);
        conversation.push_user("while running".into());
        conversation.apply(AgentUpdate::StreamChunk("more".into()), &mut state);

        assert_eq!(conversation.len(), 2);
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Assistant { streaming: true, markdown, .. } if markdown == "onemore"
        ));
    }

    #[test]
    fn prose_after_a_tool_opens_a_new_row_below_the_card() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(AgentUpdate::StreamChunk("before".into()), &mut state);
        conversation.apply(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool_1".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: String::new(),
                presentation: presentation("Bash"),
            },
            &mut state,
        );
        conversation.apply(AgentUpdate::StreamChunk("after".into()), &mut state);
        // The card keeps the slot it was allocated in when it first appeared;
        // finishing the step updates that row instead of appending a new one.
        conversation.apply(
            AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "tool_1".into(),
                result: tact_protocol::StepResult {
                    tool: "bash".into(),
                    arg_summary: "cargo test".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "ok".into(),
                    detail: None,
                    duration_us: Some(1_000),
                    permission_label: None,
                    presentation: presentation("Bash"),
                },
            },
            &mut state,
        );

        assert_eq!(conversation.len(), 3, "{:?}", conversation.rows());
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Assistant { markdown, streaming: false, .. } if markdown == "before"
        ));
        assert!(matches!(
            &conversation.rows()[1],
            TranscriptRow::Tool { display_name, status: ToolStatus::Succeeded, .. }
                if display_name == "Bash"
        ));
        assert!(matches!(
            &conversation.rows()[2],
            TranscriptRow::Assistant { markdown, streaming: true, .. } if markdown == "after"
        ));
    }

    #[test]
    fn prose_after_a_thought_opens_a_new_row_below_it() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(AgentUpdate::StreamChunk("before".into()), &mut state);
        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &mut state,
        );
        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("weighing".into())),
            &mut state,
        );
        conversation.apply(AgentUpdate::StreamChunk("after".into()), &mut state);

        // thinking sits between the two prose rows, in arrival order.
        assert_eq!(conversation.len(), 3, "{:?}", conversation.rows());
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Assistant { markdown, streaming: false, .. } if markdown == "before"
        ));
        assert!(matches!(
            &conversation.rows()[1],
            TranscriptRow::Thinking { text, .. } if text == "weighing"
        ));
        assert!(matches!(
            &conversation.rows()[2],
            TranscriptRow::Assistant { markdown, streaming: true, .. } if markdown == "after"
        ));
    }

    #[test]
    fn step_lifecycle_fills_one_tool_row() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool_1".into(),
                tool_name: "read_file".into(),
                arg_summary: "src/main.rs".into(),
                arg_full: String::new(),
                presentation: presentation("Read"),
            },
            &mut state,
        );
        conversation.apply(
            AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "tool_1".into(),
                result: tact_protocol::StepResult {
                    tool: "read_file".into(),
                    arg_summary: "src/main.rs".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "ok".into(),
                    detail: None,
                    duration_us: Some(12_000),
                    permission_label: None,
                    presentation: presentation("Read"),
                },
            },
            &mut state,
        );

        assert_eq!(conversation.len(), 1);
        match &conversation.rows()[0] {
            TranscriptRow::Tool {
                display_name,
                detail,
                duration,
                status,
                ..
            } => {
                assert_eq!(display_name, "Read");
                assert_eq!(detail, "src/main.rs");
                assert_eq!(duration, "12 ms");
                assert_eq!(*status, ToolStatus::Succeeded);
            }
            other => panic!("expected a tool row, got {other:?}"),
        }
    }

    #[test]
    fn plan_step_tracks_failure_and_clears_it_when_the_tool_succeeds() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();
        state.plan.push(PlanStep::new(
            "Run the focused test",
            "bash",
            "tool_1",
            Vec::<(String, String)>::new(),
        ));

        conversation.apply(
            AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "tool_1".into(),
                result: tact_protocol::StepResult {
                    tool: "bash".into(),
                    arg_summary: "cargo test -p tact-gui".into(),
                    arg_full: None,
                    status: StepStatus::Failed,
                    message: "test failed".into(),
                    detail: None,
                    duration_us: Some(10_000),
                    permission_label: None,
                    presentation: presentation("Bash"),
                },
            },
            &mut state,
        );

        assert!(state.plan_failed.contains(&0));
        assert_eq!(state.plan[0].output.as_deref(), Some("test failed"));

        conversation.apply(
            AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "tool_1".into(),
                result: tact_protocol::StepResult {
                    tool: "bash".into(),
                    arg_summary: "cargo test -p tact-gui".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "test passed".into(),
                    detail: None,
                    duration_us: Some(20_000),
                    permission_label: None,
                    presentation: presentation("Bash"),
                },
            },
            &mut state,
        );

        assert!(!state.plan_failed.contains(&0));
        assert_eq!(state.plan[0].output.as_deref(), Some("test passed"));
    }

    #[test]
    fn step_failed_records_the_error_on_the_plan_step() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();
        state.plan.push(PlanStep::new(
            "Read the file",
            "read_file",
            "tool_1",
            Vec::<(String, String)>::new(),
        ));

        conversation.apply(
            AgentUpdate::StepFailed {
                idx: 0,
                tool_id: "tool_1".into(),
                arg_summary: "src/main.rs".into(),
                error: "permission denied".into(),
            },
            &mut state,
        );

        assert!(state.plan_failed.contains(&0));
        assert_eq!(state.plan[0].output.as_deref(), Some("permission denied"));
    }

    #[test]
    fn reveal_tool_opens_the_tool_card_and_returns_its_row() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();
        conversation.apply(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool_1".into(),
                tool_name: "read_file".into(),
                arg_summary: "src/main.rs".into(),
                arg_full: String::new(),
                presentation: presentation("Read"),
            },
            &mut state,
        );

        assert_eq!(conversation.reveal_tool("tool_1"), Some(0));
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Tool { expanded: true, .. }
        ));
        assert_eq!(conversation.reveal_tool("missing"), None);
    }

    #[test]
    fn tool_progress_accumulates_into_the_row_output() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool_1".into(),
                tool_name: "bash".into(),
                arg_summary: "cargo test".into(),
                arg_full: String::new(),
                presentation: presentation("Bash"),
            },
            &mut state,
        );
        conversation.apply(
            AgentUpdate::ToolProgress {
                tool_id: "tool_1".into(),
                chunks: vec![
                    ToolOutputChunk::stdout("Compiling tact-gui\n"),
                    ToolOutputChunk::stderr("warning: unused import\n"),
                ],
            },
            &mut state,
        );
        let change = conversation.apply(
            AgentUpdate::ToolProgress {
                tool_id: "tool_1".into(),
                chunks: vec![ToolOutputChunk::stdout("   Finished test [unoptimized]\n")],
            },
            &mut state,
        );

        assert_eq!(change, Change::Resized(0));
        match &conversation.rows()[0] {
            TranscriptRow::Tool { output, detail, .. } => {
                // Every chunk is kept for the card's output block, in arrival
                // order and across streams.
                assert_eq!(
                    output,
                    "Compiling tact-gui\nwarning: unused import\n   Finished test [unoptimized]\n"
                );
                // The summary line tracks the newest non-empty line, trimmed.
                assert_eq!(detail, "Finished test [unoptimized]");
            }
            other => panic!("expected a tool row, got {other:?}"),
        }
    }

    #[test]
    fn tool_output_keeps_its_tail_without_splitting_a_character() {
        // Three-byte characters: the byte cut lands inside one, so the kept
        // tail has to start at the next character boundary.
        let long = "行".repeat(3_000);
        let mut output = long.clone();
        retain_output_tail(&mut output);

        assert!(
            output.len() <= TOOL_OUTPUT_LIMIT,
            "{} bytes exceeds the {} limit",
            output.len(),
            TOOL_OUTPUT_LIMIT
        );
        assert!(long.ends_with(&output), "only the head is dropped");
        assert!(
            output.chars().all(|character| character == '行'),
            "a split character would not survive as itself"
        );
        assert!(
            output.len() > TOOL_OUTPUT_LIMIT - 4,
            "the cut advances at most one character past the limit, kept {} bytes",
            output.len()
        );

        let mut short = "abc".to_string();
        retain_output_tail(&mut short);
        assert_eq!(short, "abc", "output under the limit is untouched");
    }

    #[test]
    fn file_write_results_land_in_the_diff_pane() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(
            AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "write_1".into(),
                result: tact_protocol::StepResult {
                    tool: "write_file".into(),
                    arg_summary: "src/main.rs".into(),
                    arg_full: Some("src/main.rs".into()),
                    status: StepStatus::Success,
                    message: "wrote file".into(),
                    detail: Some("+fn main() {}\n".into()),
                    duration_us: Some(5_000),
                    permission_label: None,
                    presentation: ToolPresentationInfo {
                        visual_kind: ToolVisualKind::FileWrite,
                        ..presentation("Write")
                    },
                },
            },
            &mut state,
        );

        assert_eq!(state.diff.len(), 1);
        assert_eq!(state.diff[0].path, "src/main.rs");
        assert_eq!(state.diff[0].detail, "+fn main() {}\n");
    }

    #[test]
    fn thinking_deltas_grow_one_row_until_finished() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &mut state,
        );
        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("checking".into())),
            &mut state,
        );
        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
            &mut state,
        );
        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("again".into())),
            &mut state,
        );

        assert_eq!(conversation.len(), 2);
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Thinking { text, .. } if text == "checking"
        ));
    }

    #[test]
    fn a_finished_thought_is_stamped_with_its_elapsed_seconds() {
        let mut conversation = Conversation::default();
        let mut state = SessionState::default();

        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started),
            &mut state,
        );
        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta("weighing".into())),
            &mut state,
        );
        // While the block is live there is nothing to report yet, so the
        // summary reads "Thinking" rather than a bogus duration.
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Thinking {
                duration_seconds: None,
                expanded: true,
                ..
            }
        ));

        conversation.apply(
            AgentUpdate::ThinkingChunk(ThinkingChunk::Finished),
            &mut state,
        );
        // A block that finished within the first second still reads as one
        // second, matching the prototype's whole-second "Thought for 8s".
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Thinking {
                duration_seconds: Some(1),
                expanded: true,
                ..
            }
        ));

        assert!(
            conversation.toggle_expanded(0),
            "the summary collapses the reasoning body"
        );
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Thinking {
                expanded: false,
                ..
            }
        ));
    }

    #[test]
    fn task_complete_stops_the_turn_and_summarises() {
        let mut conversation = Conversation::default();
        let mut state = SessionState {
            running: true,
            ..SessionState::default()
        };

        conversation.apply(AgentUpdate::StreamChunk("done".into()), &mut state);
        let change = conversation.apply(AgentUpdate::TaskComplete("done".into()), &mut state);

        assert!(!state.running);
        assert_eq!(conversation.len(), 2);
        assert!(matches!(change, Change::Appended(1)));
        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::Assistant {
                streaming: false,
                ..
            }
        ));
    }

    #[test]
    fn the_transcript_serializes_to_markdown() {
        let mut conversation = Conversation::default();
        conversation.push_user("Ship the toolbar".to_string());
        conversation.push_row(TranscriptRow::Assistant {
            markdown: "Done — see `shell.rs`.".to_string(),
            streaming: false,
            sent_at: 0,
            model: None,
        });
        conversation.push_row(TranscriptRow::Thinking {
            text: "The prototype has two buttons.".to_string(),
            duration_seconds: Some(8),
            expanded: true,
        });
        conversation.push_row(TranscriptRow::Tool {
            display_name: "Read".to_string(),
            detail: "crates/tact-gui/src/shell.rs".to_string(),
            output: "line one\nline two".to_string(),
            duration: "1.2s".to_string(),
            status: ToolStatus::Succeeded,
            expanded: false,
            visual_kind: ToolVisualKind::FileRead,
            diff_stats: None,
        });
        // An answered approval is a row like any other, so the copy button has
        // to write the question and the decision it settled on.
        conversation.push_approval(
            Request {
                id: 3,
                prompt: "Run command: cargo check -p tact-gui".to_string(),
                options: vec!["Allow once".to_string(), "Deny".to_string()],
                multi: false,
                selected: Vec::new(),
            },
            "Allow once".to_string(),
        );

        assert_eq!(
            conversation.to_markdown(),
            concat!(
                "Ship the toolbar\n\n",
                "Done — see `shell.rs`.\n\n",
                "> The prototype has two buttons.\n\n",
                "- **Read** `crates/tact-gui/src/shell.rs` (succeeded · 1.2s)\n\n",
                "```\nline one\nline two\n```\n\n",
                "> Run command: cargo check -p tact-gui -> Allow once",
            )
        );
    }

    /// A stored session: prose, reasoning, a tool, its result, then the answer.
    fn stored_turns() -> Vec<HistoryMessage> {
        vec![
            HistoryMessage {
                role: HistoryRole::User,
                blocks: vec![HistoryBlock::Text("check the build".into())],
            },
            HistoryMessage {
                role: HistoryRole::Assistant,
                blocks: vec![
                    HistoryBlock::Thinking("weighing".into()),
                    HistoryBlock::ToolUse {
                        id: "tool_1".into(),
                        name: "bash".into(),
                        detail: "cargo test".into(),
                    },
                ],
            },
            HistoryMessage {
                role: HistoryRole::User,
                blocks: vec![HistoryBlock::ToolResult {
                    tool_use_id: "tool_1".into(),
                    output: "ok".into(),
                }],
            },
            HistoryMessage {
                role: HistoryRole::Assistant,
                blocks: vec![HistoryBlock::Text("it passes".into())],
            },
        ]
    }

    #[test]
    fn a_stored_transcript_redraws_thinking_tool_and_answer_in_order() {
        let mut conversation = Conversation::default();

        assert_eq!(conversation.load_history(&stored_turns()), 4);

        assert!(matches!(
            &conversation.rows()[0],
            TranscriptRow::User { text, sent_at } if text == "check the build" && *sent_at == NO_TIMESTAMP
        ));
        assert!(matches!(
            &conversation.rows()[1],
            TranscriptRow::Thinking { text, .. } if text == "weighing"
        ));
        assert!(matches!(
            &conversation.rows()[2],
            TranscriptRow::Tool { display_name, detail, output, status: ToolStatus::Succeeded, .. }
                if display_name == "bash" && detail == "cargo test" && output == "ok"
        ));
        assert!(matches!(
            &conversation.rows()[3],
            TranscriptRow::Assistant { markdown, streaming: false, sent_at, .. }
                if markdown == "it passes" && *sent_at == NO_TIMESTAMP
        ));
    }

    #[test]
    fn a_redrawn_tool_card_keeps_its_use_slot_and_a_stray_result_is_ignored() {
        let mut conversation = Conversation::default();
        let messages = vec![
            HistoryMessage {
                role: HistoryRole::Assistant,
                blocks: vec![
                    HistoryBlock::Text("looking".into()),
                    HistoryBlock::ToolUse {
                        id: "tool_1".into(),
                        name: "read_file".into(),
                        detail: "src/main.rs".into(),
                    },
                    HistoryBlock::Text("done".into()),
                ],
            },
            // A turn that ended before the tool answered: the card stays live
            // rather than claiming a result the store never recorded.
            HistoryMessage {
                role: HistoryRole::Assistant,
                blocks: vec![HistoryBlock::ToolUse {
                    id: "tool_2".into(),
                    name: "bash".into(),
                    detail: "sleep 1".into(),
                }],
            },
            HistoryMessage {
                role: HistoryRole::User,
                blocks: vec![HistoryBlock::ToolResult {
                    tool_use_id: "never_seen".into(),
                    output: "orphan".into(),
                }],
            },
        ];

        assert_eq!(conversation.load_history(&messages), 4);

        assert_eq!(
            conversation
                .rows()
                .iter()
                .map(|row| match row {
                    TranscriptRow::Assistant { markdown, .. } => markdown.clone(),
                    TranscriptRow::Tool { display_name, .. } => format!("<{display_name}>"),
                    other => panic!("unexpected row {other:?}"),
                })
                .collect::<Vec<_>>(),
            vec!["looking", "<read_file>", "done", "<bash>"],
            "a card holds the slot of its own ToolUse block"
        );
        assert!(matches!(
            &conversation.rows()[3],
            TranscriptRow::Tool {
                status: ToolStatus::Running,
                ..
            }
        ));
    }

    #[test]
    fn durations_scale_with_magnitude() {
        assert_eq!(format_duration(None), "");
        assert_eq!(format_duration(Some(250)), "250 µs");
        assert_eq!(format_duration(Some(12_000)), "12 ms");
        assert_eq!(format_duration(Some(2_500_000)), "2.5 s");
    }

    #[test]
    fn ages_coarsen_from_minutes_to_days() {
        assert_eq!(age_label(0), "just now");
        assert_eq!(age_label(59), "just now");
        assert_eq!(age_label(60), "1m ago");
        assert_eq!(age_label(7_200), "2h ago");
        assert_eq!(age_label(3 * 86_400), "3d ago");
    }
}
