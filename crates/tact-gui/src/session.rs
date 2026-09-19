//! Bridge between the headless agent session and the GPUI shell.
//!
//! `tact-session` owns the agent and its command driver. This module owns the
//! presentation half of the contract: it starts a session, hands the shell a
//! command sender, and folds [`AgentUpdate`]s into the transcript model. No
//! GPUI type crosses into the headless crates and no protocol type reaches the
//! row renderer — [`Conversation`] is the seam in both directions.

use std::{collections::HashMap, path::PathBuf};

use gpui_kit::Task;
use tact_protocol::{
    AgentUpdate, ModelCallParams, PlanStep, StepStatus, SubagentRunSnapshot, TaskSnapshot,
    ThinkingChunk, TokenUsageInfo, ToolOutputChunk, ToolVisualKind, UserCommand,
};
use tact_session::{RecentSession, SessionOptions, SessionRuntime};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::pane::DiffEntry;
use crate::transcript::{ToolStatus, TranscriptRow};

/// Commands and identity for one running session.
pub(crate) struct SessionHandle {
    session_id: String,
    commands: UnboundedSender<UserCommand>,
}

impl SessionHandle {
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
    let handle = SessionHandle {
        session_id: runtime.session_id().to_string(),
        commands: runtime.commands.clone(),
    };
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
    /// A blocking choice the agent is waiting on.
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

    /// Start a new turn: seal live rows so the next chunk opens fresh ones.
    fn end_turn(&mut self) {
        if let Some(index) = self.open_assistant.take()
            && let Some(TranscriptRow::Assistant { streaming, .. }) = self.rows.get_mut(index)
        {
            *streaming = false;
        }
        self.open_thinking = None;
        self.open_tools.clear();
    }

    /// Append the user's own message.
    ///
    /// Sealing live rows is the *turn end*'s job ([`Self::end_turn`] on
    /// `TaskComplete`), not the submit's: a prompt queued mid-turn must not
    /// close the assistant row that is still streaming.
    pub(crate) fn push_user(&mut self, text: String) -> usize {
        self.rows.push(TranscriptRow::User { text });
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
        });
        self.rows.len()
    }

    /// Append a local system note (startup failures, cancelled turns).
    pub(crate) fn push_system(&mut self, text: String) -> usize {
        self.rows.push(TranscriptRow::System { text });
        self.rows.len()
    }

    /// Fold one protocol update into the transcript and session state.
    pub(crate) fn apply(&mut self, update: AgentUpdate, state: &mut SessionState) -> Change {
        match update {
            AgentUpdate::StreamChunk(text) => self.append_stream(text),
            AgentUpdate::ThinkingChunk(chunk) => self.apply_thinking(chunk),
            AgentUpdate::StepAdded(step) => {
                state.plan.push(step.clone());
                // The transcript shows a placeholder as soon as a step is
                // planned, so the plan pane and the transcript cannot disagree
                // about how many steps exist.
                if step.tool_id.is_empty() {
                    Change::None
                } else {
                    self.ensure_tool(
                        &step.tool_id,
                        &step.tool,
                        &step.description,
                        ToolStatus::Running,
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
                self.set_tool(&tool_id, name, detail, ToolStatus::Running, String::new())
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
                if matches!(
                    result.presentation.visual_kind,
                    ToolVisualKind::FileWrite | ToolVisualKind::FileEdit
                ) && let Some(changed) = result
                    .detail
                    .as_ref()
                    .filter(|text| !text.trim().is_empty())
                {
                    state.diff.push(DiffEntry::new(
                        result
                            .arg_full
                            .clone()
                            .unwrap_or_else(|| result.arg_summary.clone()),
                        changed.clone(),
                    ));
                }
                self.set_tool(
                    &tool_id,
                    display_name(&result.presentation.display_name, &result.tool),
                    detail,
                    status,
                    duration,
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
                self.set_tool(
                    &tool_id,
                    self.tool_name(&tool_id).unwrap_or_else(|| "Tool".into()),
                    detail,
                    ToolStatus::Failed,
                    String::new(),
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
    fn append_stream(&mut self, text: String) -> Change {
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
                let index = self.rows.len();
                self.rows.push(TranscriptRow::Thinking {
                    text: String::new(),
                });
                self.open_thinking = Some(index);
                Change::Appended(1)
            }
            ThinkingChunk::Delta(text) => {
                let index = match self.open_thinking {
                    Some(index) => index,
                    None => {
                        let index = self.rows.len();
                        self.rows.push(TranscriptRow::Thinking {
                            text: String::new(),
                        });
                        self.open_thinking = Some(index);
                        index
                    }
                };
                if let Some(TranscriptRow::Thinking { text: body }) = self.rows.get_mut(index) {
                    body.push_str(&text);
                }
                Change::Resized(index)
            }
            ThinkingChunk::Finished => {
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
        if let Some(TranscriptRow::Tool { detail, .. }) = self.rows.get_mut(index) {
            *detail = latest;
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
    ) -> Change {
        if self.open_tools.contains_key(tool_id) {
            return Change::None;
        }
        let index = self.rows.len();
        self.rows.push(TranscriptRow::Tool {
            display_name: name.to_string(),
            detail: first_line(detail),
            duration: String::new(),
            status,
        });
        self.open_tools.insert(tool_id.to_string(), index);
        Change::Appended(1)
    }

    /// Update an existing tool row, creating it when the id is new.
    fn set_tool(
        &mut self,
        tool_id: &str,
        name: String,
        detail: String,
        status: ToolStatus,
        duration: String,
    ) -> Change {
        let index = match self.open_tools.get(tool_id).copied() {
            Some(index) => index,
            None => return self.ensure_tool(tool_id, &name, &detail, status),
        };
        if let Some(TranscriptRow::Tool {
            display_name,
            detail: current,
            duration: current_duration,
            status: current_status,
        }) = self.rows.get_mut(index)
        {
            *display_name = name;
            *current = detail;
            *current_status = status;
            *current_duration = duration;
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
        assert_eq!(
            conversation.rows(),
            &[TranscriptRow::Assistant {
                markdown: "Hello world".into(),
                streaming: true,
            }]
        );
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
            TranscriptRow::Assistant { streaming: false, markdown } if markdown == "one"
        ));
        let last = conversation.rows().last().expect("second answer");
        assert!(matches!(
            last,
            TranscriptRow::Assistant { streaming: true, markdown } if markdown == "two"
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
            TranscriptRow::Assistant { streaming: true, markdown } if markdown == "onemore"
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
            TranscriptRow::Thinking { text } if text == "checking"
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
