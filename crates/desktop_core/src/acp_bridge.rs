//! Map ACP session updates back onto Tact's own [`AgentUpdate`] vocabulary.
//!
//! The desktop renders its transcript from [`AgentUpdate`] through a reducer
//! that already handles every event Tact can produce (tool groups, gates,
//! thinking regions, subagent transcripts, plan rows). Driving that reducer
//! from an ACP agent means the ACP traffic has to be expressed in those terms
//! again, so this module is the inverse of the forward mapping in
//! `tact-acp`.
//!
//! It is deliberately a pure function over data: no connection, no I/O, so
//! every mapping decision is unit-testable.
//!
//! # What cannot round-trip
//!
//! ACP is a smaller vocabulary than Tact's. Four families have no faithful
//! reverse mapping, and this module is explicit about each rather than
//! inventing structure:
//!
//! - **Hook context and hook status** — no ACP update kind exists.
//! - **`SurfaceSnapshot`** — `/stats`, `/mcp list`, `/hooks list`, the system
//!   prompt and history read-outs arrive as `notice` text, not structure.
//!   Because the desktop cannot ask for one, the ACP transport answers such a
//!   command with `HostSendError::Unsupported` instead of a silent success.
//! - **`ToolTranscript`** structure — a child session's entries arrive as
//!   ordinary agent/thought/tool updates under the child id; the parent's
//!   structured popup is rebuilt by the reducer, not by this module.
//! - **Per-model-call granularity** — ACP has one foreground level, not a
//!   counter, so the kernel's per-call `TurnStats` is synthesized once per run
//!   (see [`state_update`]).
//!
//! Two families *do* round-trip and must not be dropped: `agent_message` /
//! `agent_thought` (whole-message upserts, which is how `session/resume` replays
//! history) and `plan_removed` (which the shell needs to drop a finished
//! checklist). `plan_removed` is stable from schema 1.20 on, so it needs no
//! feature flag; before that it was behind `unstable_plan_operations`, and an
//! update without the feature deserialized as an unknown `Other` variant and
//! was silently ignored.

use agent_client_protocol::schema::MaybeUndefined;
use agent_client_protocol::v2::schema;
use tact_protocol::{
    AgentErrorKind, AgentUpdate, StepResult, StepStatus, ToolPresentationInfo, ToolVisualKind,
};
use tact_protocol::{CompactionEvent, CompactionKind, CompactionState};

/// The run id every mapped update carries.
///
/// ACP addresses one foreground activity per session, so one id is enough and
/// a `RunStarted`/`RunFinished` pair always names the same run.
pub const RUN_ID: &str = "acp";

/// One ACP update, plus the state needed to interpret it.
///
/// ACP tool updates are patches: `None`/`undefined` fields mean "unchanged",
/// not "cleared". The caller keeps the accumulated per-tool state; this
/// struct only carries what the current message said.
pub struct UpdateContext<'a> {
    /// The session this update belongs to. Child sessions address the parent's
    /// `spawn_subagent` card, so the id decides which card an update feeds.
    pub session_id: &'a str,
    /// Whether this session is a child of the one the desktop is showing.
    pub is_child: bool,
    /// The tool call id a child session's updates should be attributed to,
    /// when the parent launched it from a tool call.
    pub parent_tool_id: Option<&'a str>,
}

/// Translate one ACP session update.
///
/// Returns zero or more [`AgentUpdate`]s: ACP folds into one message several
/// facts Tact reports separately (a tool completion carries its status, name
/// and output), so one update can become several.
pub fn to_agent_updates(
    update: &schema::SessionUpdate,
    context: &UpdateContext<'_>,
) -> Vec<AgentUpdate> {
    match update {
        schema::SessionUpdate::AgentMessageChunk(chunk) => text_of(&chunk.content)
            .map(AgentUpdate::StreamChunk)
            .into_iter()
            .collect(),
        schema::SessionUpdate::AgentThoughtChunk(chunk) => text_of(&chunk.content)
            .map(|text| AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(text)))
            .into_iter()
            .collect(),
        // A whole message at once rather than a chunk. `session/resume` replays
        // history this way, so dropping it lost every replayed answer — the
        // transcript opened with the tool calls and no text between them.
        //
        // It is also a *replacement*: two updates for one `message_id` patch
        // the same message. Starting the message fresh and then streaming its
        // text is what a chunked message looks like to the reducer, so the
        // replacement reuses that path instead of appending twice.
        schema::SessionUpdate::AgentMessage(message) => {
            let text = message_text(message);
            let mut updates = vec![AgentUpdate::MessageStarted {
                message_id: message.message_id.0.to_string(),
            }];
            if !text.is_empty() {
                updates.push(AgentUpdate::StreamChunk(text));
            }
            updates.push(AgentUpdate::MessageFinished {
                message_id: message.message_id.0.to_string(),
                phase: None,
            });
            updates
        }
        schema::SessionUpdate::AgentThought(thought) => {
            let text = message_text_of(&thought.content);
            if text.is_empty() {
                return Vec::new();
            }
            vec![
                AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
                AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(text)),
                AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Finished),
            ]
        }
        schema::SessionUpdate::UserMessageChunk(_) | schema::SessionUpdate::UserMessage(_) => {
            // The desktop already pushed the user's row when it submitted; an
            // echo would duplicate it.
            Vec::new()
        }
        schema::SessionUpdate::StateUpdate(state) => state_update(state, context),
        schema::SessionUpdate::ToolCallUpdate(tool) => tool_update(tool, context),
        schema::SessionUpdate::ToolCallContentChunk(chunk) => {
            // Streamed tool output arrives as a chunked content block; it is
            // the ACP form of Tact's `ToolProgress`.
            let text = tool_content_text_of(&chunk.content).unwrap_or_default();
            if text.is_empty() {
                Vec::new()
            } else {
                vec![AgentUpdate::ToolProgress {
                    run_id: None,
                    tool_id: chunk.tool_call_id.0.to_string(),
                    chunks: vec![tact_protocol::ToolOutputChunk::stdout(text)],
                }]
            }
        }
        schema::SessionUpdate::PlanUpdate(plan) => {
            let schema::PlanUpdateContent::Items(items) = &plan.plan else {
                return Vec::new();
            };
            let updates = vec![AgentUpdate::TasksChanged {
                tasks: items
                    .entries
                    .iter()
                    .enumerate()
                    .map(|(index, entry)| tact_protocol::TaskSnapshot {
                        // ACP entries carry no id, so the ordinal is the only
                        // stable identity a plan row can key on.
                        id: index as u64,
                        subject: entry.content.clone(),
                        status: match entry.status {
                            schema::PlanEntryStatus::Pending => {
                                tact_protocol::TaskStatusSnapshot::Pending
                            }
                            schema::PlanEntryStatus::InProgress => {
                                tact_protocol::TaskStatusSnapshot::InProgress
                            }
                            schema::PlanEntryStatus::Completed => {
                                tact_protocol::TaskStatusSnapshot::Completed
                            }
                            _ => tact_protocol::TaskStatusSnapshot::Pending,
                        },
                        ..Default::default()
                    })
                    .collect(),
                reason: tact_protocol::TasksChangeReason::Updated,
            }];
            updates
        }
        // The agent's own "this plan is over" signal. It is *removal*, not an
        // empty replacement, because a client that keeps drawing an empty list
        // would show a finished checklist as still up. An emptied `Items` list
        // in the branch above means the same thing, but only this variant tells
        // the shell to drop the strip.
        schema::SessionUpdate::PlanRemoved(_) => vec![AgentUpdate::TasksChanged {
            tasks: Vec::new(),
            reason: tact_protocol::TasksChangeReason::Updated,
        }],
        schema::SessionUpdate::UsageUpdate(usage) => {
            vec![AgentUpdate::TokenUsage(tact_protocol::TokenUsageInfo {
                prompt: usage.used.min(u64::from(u32::MAX)) as u32,
                completion: 0,
                total: usage.used.min(u64::from(u32::MAX)) as u32,
                prompt_cache_hit_tokens: 0,
                prompt_cache_miss_tokens: 0,
                reasoning_tokens: 0,
            })]
        }
        schema::SessionUpdate::Notice(notice) => notice_update(notice),
        schema::SessionUpdate::CompactionUpdate(compaction) => compaction_updates(compaction),
        schema::SessionUpdate::AvailableCommandsUpdate(_)
        | schema::SessionUpdate::ConfigOptionUpdate(_)
        | schema::SessionUpdate::SessionInfoUpdate(_)
        | schema::SessionUpdate::ReplayComplete(_)
        | schema::SessionUpdate::TerminalUpdate(_)
        | schema::SessionUpdate::TerminalOutputChunk(_)
        | schema::SessionUpdate::CompactionSummaryChunk(_)
        | schema::SessionUpdate::SubagentUpdate(_)
        | schema::SessionUpdate::SessionMessage(_)
        | schema::SessionUpdate::SessionMessageChunk(_)
        | _ => Vec::new(),
    }
}

/// `running` / `idle` / `requires_action` → Tact's run boundaries.
///
/// Tact reports a run start, one `TurnStats` per model call, and a terminal
/// event. ACP reports one foreground state, so this function has to supply the
/// two pieces ACP leaves implicit:
///
/// - **`TurnStats` after `RunStarted`.** The reducer only opens a visible run
///   — and therefore only honours a later `RunFinished` — once `TurnStats`
///   names an active run id. Without it every assistant row stayed `Running`
///   and `RunFinished` was dropped as an unmatched activation, so the turn
///   never settled: the transcript kept a spinner and the controller kept
///   `RunPhase::Streaming`, which blocks the queue forever.
/// - **the stop reason's terminal event.** ACP folds "the turn failed" into
///   the idle's stop reason. `RunFinished` alone would close the turn as a
///   success, so `Error` becomes `RunFailed` and `Cancelled` becomes
///   `TaskCancelled`.
fn state_update(state: &schema::StateUpdate, context: &UpdateContext<'_>) -> Vec<AgentUpdate> {
    let _ = context;
    match state {
        schema::StateUpdate::Running(_) => vec![
            AgentUpdate::RunStarted {
                run_id: RUN_ID.to_string(),
            },
            // ACP has no turn counter, and the desktop only needs the
            // activation: one outer turn is what the reader is watching.
            AgentUpdate::TurnStats {
                run_id: Some(RUN_ID.to_string()),
                turns_taken: 1,
                max_turns: None,
            },
        ],
        schema::StateUpdate::Idle(idle) => {
            let run_id = RUN_ID.to_string();
            match &idle.stop_reason {
                Some(schema::StopReason::Error(error)) => {
                    let message = error
                        .error
                        .as_ref()
                        .map(|error| error.message.clone())
                        .filter(|message| !message.trim().is_empty())
                        .unwrap_or_else(|| "the ACP agent ended the turn with an error".into());
                    vec![
                        AgentUpdate::RunFailed {
                            run_id,
                            retryable: true,
                            message: Some(message),
                        },
                        AgentUpdate::TaskCancelled,
                    ]
                }
                Some(schema::StopReason::Cancelled) => {
                    vec![
                        AgentUpdate::RunFinished { run_id },
                        AgentUpdate::TaskCancelled,
                    ]
                }
                // A reply cut off by the output limit, or one the provider
                // refused: the turn ended, but not on the agent's own terms.
                // Reporting these as a plain finish is what let a truncated
                // answer read as done — the exact mistake this bridge fixes in
                // the other direction when it reports them upward.
                Some(stop @ (schema::StopReason::MaxTokens | schema::StopReason::Refusal)) => {
                    let message = match stop {
                        schema::StopReason::MaxTokens => {
                            "the ACP agent stopped because its reply reached the output token limit"
                                .to_string()
                        }
                        _ => "the ACP agent refused this request; its next turn will not include \
                              this message"
                            .to_string(),
                    };
                    let kind = match stop {
                        schema::StopReason::MaxTokens => AgentErrorKind::Truncated(message.clone()),
                        _ => AgentErrorKind::Refused(message.clone()),
                    };
                    vec![
                        AgentUpdate::RunFailed {
                            run_id,
                            retryable: false,
                            message: Some(message),
                        },
                        AgentUpdate::Error(kind),
                        AgentUpdate::TaskCancelled,
                    ]
                }
                // `EndTurn`, a turn cap, or no reason at all: the turn ended and
                // the agent said nothing failed.
                _ => vec![AgentUpdate::RunFinished { run_id }],
            }
        }
        schema::StateUpdate::RequiresAction(_) => Vec::new(),
        _ => Vec::new(),
    }
}

/// A tool call patch → the `StepQueued`/`StepStarted`/`StepFinished` lifecycle.
fn tool_update(tool: &schema::ToolCallUpdate, context: &UpdateContext<'_>) -> Vec<AgentUpdate> {
    let tool_id = tool.tool_call_id.0.to_string();
    let title = tool.title.clone().take();
    let name = tool
        .name
        .clone()
        .take()
        .or_else(|| title.clone())
        .unwrap_or_else(|| "tool".to_string());
    let arg_summary = title.unwrap_or_else(|| name.clone());
    let arg_full = tool
        .raw_input
        .clone()
        .take()
        .map(|value| value.to_string())
        .unwrap_or_default();
    let presentation = presentation_for(tool, &name);
    let status = tool.status.clone().take();

    if context.is_child {
        // A child session's tool calls are not the parent's cards. The parent
        // card is the `spawn_subagent` call; the child's own steps belong to
        // its transcript popup, which the reducer rebuilds from the child's
        // StreamChunk/ThinkingChunk stream.
        return Vec::new();
    }

    match status {
        // A new call: nothing has been reported about it yet.
        None | Some(schema::ToolCallStatus::Pending) => vec![AgentUpdate::StepQueued {
            idx: 0,
            tool_id,
            tool_name: name,
            arg_summary,
            arg_full,
            presentation,
        }],
        Some(schema::ToolCallStatus::InProgress) => vec![AgentUpdate::StepStarted {
            idx: 0,
            tool_id,
            tool_name: name,
            arg_summary,
            arg_full,
            presentation,
        }],
        Some(
            status @ (schema::ToolCallStatus::Completed
            | schema::ToolCallStatus::Failed
            | schema::ToolCallStatus::Cancelled),
        ) => {
            let text = tool_content_text(tool);
            let step_status = match status {
                schema::ToolCallStatus::Completed => StepStatus::Success,
                schema::ToolCallStatus::Failed => StepStatus::Failed,
                _ => StepStatus::Cancelled,
            };
            vec![AgentUpdate::StepFinished {
                idx: 0,
                run_id: None,
                tool_id,
                result: StepResult {
                    tool: name,
                    arg_summary,
                    arg_full: (!arg_full.is_empty()).then_some(arg_full),
                    status: step_status,
                    message: if matches!(step_status, StepStatus::Success) {
                        String::new()
                    } else {
                        text.clone()
                    },
                    detail: (!text.is_empty()).then_some(text),
                    duration_us: None,
                    permission_label: None,
                    presentation,
                },
            }]
        }
        _ => Vec::new(),
    }
}

/// Recover Tact's presentation hints from an ACP tool kind and title.
///
/// ACP's `ToolKind` is coarser than `ToolVisualKind` but the mapping is total
/// in the direction that matters: every visual kind the desktop styles
/// specially has a kind that implies it.
fn presentation_for(tool: &schema::ToolCallUpdate, name: &str) -> ToolPresentationInfo {
    let visual_kind = match tool.kind.clone().take() {
        Some(schema::ToolKind::Edit)
        | Some(schema::ToolKind::Delete)
        | Some(schema::ToolKind::Move) => ToolVisualKind::FileEdit,
        Some(schema::ToolKind::Read) => ToolVisualKind::FileRead,
        Some(schema::ToolKind::Execute) => ToolVisualKind::Command,
        Some(schema::ToolKind::Think) => ToolVisualKind::Generic,
        _ if name == "spawn_subagent" => ToolVisualKind::Subagent,
        _ => ToolVisualKind::Generic,
    };
    ToolPresentationInfo {
        visual_kind,
        display_name: name.to_string(),
        keep_full_live_output: false,
        detail: tact_protocol::ToolDetailKind::Result,
        popup: if visual_kind == ToolVisualKind::Subagent {
            tact_protocol::ToolPopupKind::SubagentTranscript
        } else {
            tact_protocol::ToolPopupKind::None
        },
        compact_result_to_meta: false,
        keep_live: false,
    }
}

/// The text content of a tool call, in order.
fn tool_content_text(tool: &schema::ToolCallUpdate) -> String {
    let Some(content) = tool.content.clone().take() else {
        return String::new();
    };
    content
        .iter()
        .filter_map(tool_content_text_of)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The `notice` update carries Tact's own notices, including the Goal and
/// compaction read-outs the forward mapping produced.
fn notice_update(notice: &schema::Notice) -> Vec<AgentUpdate> {
    let severity = notice.severity.clone();
    if matches!(
        severity,
        schema::NoticeSeverity::Error | schema::NoticeSeverity::Warning
    ) {
        return vec![AgentUpdate::Error(AgentErrorKind::Other(
            notice
                .description
                .clone()
                .filter(|body| !body.trim().is_empty())
                .unwrap_or_else(|| notice.title.clone()),
        ))];
    }
    // A notice with a body is prose the reader should keep formatted; one
    // without is a one-line status.
    match notice.description.as_deref() {
        Some(body) if !body.trim().is_empty() => vec![AgentUpdate::MdInfo(body.to_string())],
        _ => vec![AgentUpdate::Info(notice.title.clone())],
    }
}

/// ACP's typed compaction lifecycle → Tact's typed `Compaction`.
fn compaction_updates(compaction: &schema::CompactionUpdate) -> Vec<AgentUpdate> {
    // ACP reports one compaction status; Tact's event is a kind plus a state.
    // The kind is a display label here (the desktop card reads the state), so
    // a generic compaction is the honest choice — the adapter does not tell us
    // whether this was automatic, a `/compact`, or a recovery pass.
    // `status` is a patch field: omitted means "unchanged", and the client is
    // expected to keep the status it already had. This bridge is stateless, so
    // an omitted status carries no new lifecycle step and yields no update.
    let status = match &compaction.status {
        MaybeUndefined::Undefined => return Vec::new(),
        MaybeUndefined::Null => schema::CompactionStatus::InProgress,
        MaybeUndefined::Value(status) => status.clone(),
    };
    let state = match status {
        schema::CompactionStatus::InProgress => CompactionState::Started,
        schema::CompactionStatus::Completed => CompactionState::Completed {
            summary: compaction
                .summary
                .clone()
                .take()
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(text_of)
                        .collect::<Vec<_>>()
                        .join("\n")
                })
                .filter(|body| !body.trim().is_empty()),
        },
        schema::CompactionStatus::Failed => CompactionState::Failed {
            error: compaction
                .error
                .clone()
                .take()
                .unwrap_or_else(|| "compaction failed".to_string()),
        },
        schema::CompactionStatus::Cancelled => CompactionState::Cancelled {
            reason: compaction
                .error
                .clone()
                .take()
                .unwrap_or_else(|| "compaction cancelled".to_string()),
        },
        _ => return Vec::new(),
    };
    vec![AgentUpdate::Compaction(CompactionEvent {
        kind: CompactionKind::Auto,
        state,
    })]
}

/// The text a content block carries, if it carries any.
fn text_of(block: &schema::ContentBlock) -> Option<String> {
    match block {
        schema::ContentBlock::Text(text) => Some(text.text.clone()),
        _ => None,
    }
}

/// The text of an `agent_message` upsert's content blocks.
fn message_text(message: &schema::AgentMessage) -> String {
    message_text_of(&message.content)
}

/// Join the text blocks of a patchable content list.
///
/// ACP leaves `content` undefined when the update only patches metadata, so an
/// absent list is no text rather than an empty one.
fn message_text_of(
    content: &agent_client_protocol::schema::MaybeUndefined<Vec<schema::ContentBlock>>,
) -> String {
    content
        .value()
        .into_iter()
        .flatten()
        .filter_map(text_of)
        .collect::<Vec<_>>()
        .join("\n")
}

/// The phase a finished message carries.
pub fn message_finished(message_id: &str) -> AgentUpdate {
    AgentUpdate::MessageFinished {
        message_id: message_id.to_string(),
        // ACP does not say whether an agent message was mid-turn commentary
        // or the turn's final answer; the reducer infers it from position.
        phase: None,
    }
}

/// The text one tool-call content item carries, if it is text.
fn tool_content_text_of(item: &schema::ToolCallContent) -> Option<String> {
    match item {
        schema::ToolCallContent::Content(content) => text_of(&content.content),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{TasksChangeReason, ToolOutputChunk};

    fn parent() -> UpdateContext<'static> {
        UpdateContext {
            session_id: "s",
            is_child: false,
            parent_tool_id: None,
        }
    }

    #[test]
    fn agent_text_becomes_a_stream_chunk() {
        let update = schema::SessionUpdate::AgentMessageChunk(schema::ContentChunk::new(
            schema::ContentBlock::Text(schema::TextContent::new("hello")),
            "m1",
        ));
        let mapped = to_agent_updates(&update, &parent());
        assert!(
            matches!(&mapped[..], [AgentUpdate::StreamChunk(text)] if text == "hello"),
            "{mapped:?}"
        );
    }

    #[test]
    fn thinking_becomes_a_thinking_delta() {
        let update = schema::SessionUpdate::AgentThoughtChunk(schema::ContentChunk::new(
            schema::ContentBlock::Text(schema::TextContent::new("hmm")),
            "m1",
        ));
        let mapped = to_agent_updates(&update, &parent());
        assert!(
            matches!(
                &mapped[..],
                [AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(text))]
                    if text == "hmm"
            ),
            "{mapped:?}"
        );
    }

    #[test]
    fn a_user_echo_is_dropped_so_the_row_is_not_duplicated() {
        let update = schema::SessionUpdate::UserMessageChunk(schema::ContentChunk::new(
            schema::ContentBlock::Text(schema::TextContent::new("mine")),
            "m1",
        ));
        assert!(to_agent_updates(&update, &parent()).is_empty());
    }

    #[test]
    fn a_pending_tool_call_is_queued_and_an_in_progress_one_starts() {
        let queued = schema::SessionUpdate::ToolCallUpdate(
            schema::ToolCallUpdate::new("t1").name("read_file"),
        );
        assert!(matches!(
            &to_agent_updates(&queued, &parent())[..],
            [AgentUpdate::StepQueued { tool_id, .. }] if tool_id == "t1"
        ));

        let started = schema::SessionUpdate::ToolCallUpdate(
            schema::ToolCallUpdate::new("t1")
                .name("read_file")
                .status(schema::ToolCallStatus::InProgress),
        );
        assert!(matches!(
            &to_agent_updates(&started, &parent())[..],
            [AgentUpdate::StepStarted { tool_id, .. }] if tool_id == "t1"
        ));
    }

    #[test]
    fn a_completed_tool_call_carries_its_output_in_the_detail() {
        let done = schema::SessionUpdate::ToolCallUpdate(
            schema::ToolCallUpdate::new("t1")
                .name("read_file")
                .status(schema::ToolCallStatus::Completed)
                .content(vec![schema::ToolCallContent::Content(Box::new(
                    schema::Content::new(schema::ContentBlock::Text(schema::TextContent::new(
                        "file body",
                    ))),
                ))]),
        );
        let mapped = to_agent_updates(&done, &parent());
        assert!(
            matches!(
                &mapped[..],
                [AgentUpdate::StepFinished { result, .. }]
                    if result.status == StepStatus::Success
                        && result.detail.as_deref() == Some("file body")
            ),
            "{mapped:?}"
        );
    }

    #[test]
    fn an_edit_tool_keeps_the_visual_kind_the_desktop_styles() {
        let done = schema::SessionUpdate::ToolCallUpdate(
            schema::ToolCallUpdate::new("t1")
                .name("edit_file")
                .kind(schema::ToolKind::Edit)
                .status(schema::ToolCallStatus::InProgress),
        );
        assert!(matches!(
            &to_agent_updates(&done, &parent())[..],
            [AgentUpdate::StepStarted { presentation, .. }]
                if presentation.visual_kind == ToolVisualKind::FileEdit
        ));
    }

    #[test]
    fn a_child_session_tool_does_not_become_a_parent_card() {
        let started = schema::SessionUpdate::ToolCallUpdate(
            schema::ToolCallUpdate::new("child-tool").status(schema::ToolCallStatus::InProgress),
        );
        let child = UpdateContext {
            session_id: "child",
            is_child: true,
            parent_tool_id: Some("parent-tool"),
        };
        assert!(to_agent_updates(&started, &child).is_empty());
    }

    #[test]
    fn streamed_tool_output_becomes_progress() {
        let chunk = schema::SessionUpdate::ToolCallContentChunk(schema::ToolCallContentChunk::new(
            "t1",
            schema::ToolCallContent::Content(Box::new(schema::Content::new(
                schema::ContentBlock::Text(schema::TextContent::new("line\n")),
            ))),
        ));
        let mapped = to_agent_updates(&chunk, &parent());
        assert!(
            matches!(
                &mapped[..],
                [AgentUpdate::ToolProgress { tool_id, chunks, .. }]
                    if tool_id == "t1"
                        && chunks == &vec![ToolOutputChunk::stdout("line\n")]
            ),
            "{mapped:?}"
        );
    }

    #[test]
    fn an_idle_state_finishes_the_run() {
        let idle = schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(
            schema::IdleStateUpdate::new(),
        ));
        assert!(matches!(
            &to_agent_updates(&idle, &parent())[..],
            [AgentUpdate::RunFinished { .. }]
        ));
    }

    /// The whole point of the desktop's ACP transport is that a real turn
    /// reaches the same reducer the direct host feeds. A `running` + text +
    /// `idle` sequence is what one ACP turn looks like, so this asserts the
    /// outcome a reader sees: a settled assistant row, not a spinner.
    ///
    /// It is a regression test, not a mapping test — before `TurnStats` was
    /// synthesized here the row stayed `Running` forever, because the reducer
    /// only honours a terminal for a run that `TurnStats` activated.
    #[test]
    fn one_acp_turn_settles_the_transcript_row() {
        use crate::transcript::{
            ReduceContext, RowBody, RowStatus, SessionEpoch, TranscriptReducer,
        };

        let context = ReduceContext {
            session_epoch: SessionEpoch(0),
            show_hook_output: false,
        };
        let mut reducer = TranscriptReducer::new();
        for update in [
            schema::SessionUpdate::StateUpdate(schema::StateUpdate::Running(
                schema::RunningStateUpdate::new(),
            )),
            schema::SessionUpdate::AgentMessageChunk(schema::ContentChunk::new(
                schema::ContentBlock::Text(schema::TextContent::new("acp turn")),
                "m1",
            )),
            schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(
                schema::IdleStateUpdate::new(),
            )),
        ] {
            reducer.apply_batch(to_agent_updates(&update, &parent()), &context);
        }

        let statuses: Vec<RowStatus> = reducer
            .view()
            .iter()
            .filter_map(|row| match &row.body {
                RowBody::Assistant(assistant) => Some(assistant.status),
                _ => None,
            })
            .collect();
        assert_eq!(
            statuses,
            vec![RowStatus::Success],
            "an ACP turn must settle its assistant row, not leave it running"
        );
    }

    /// ACP folds "the turn failed" into the idle's stop reason, and the
    /// reducer only settles a failed run on `RunFailed`. Reporting the idle as
    /// a plain `RunFinished` would close a failed turn as a success.
    #[test]
    fn an_error_idle_fails_the_run_instead_of_finishing_it() {
        let idle = schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(
            schema::IdleStateUpdate::new().stop_reason(Some(schema::StopReason::Error(
                schema::ErrorStopReason::new(),
            ))),
        ));
        let mapped = to_agent_updates(&idle, &parent());
        assert!(
            matches!(
                &mapped[..],
                [
                    AgentUpdate::RunFailed {
                        retryable: true,
                        ..
                    },
                    AgentUpdate::TaskCancelled
                ]
            ),
            "{mapped:?}"
        );
    }

    /// A truncated or refused reply is not a clean finish, in either direction.
    ///
    /// ACP names both stops, and the agent that reports them upward must read
    /// them back the same way: as a failed turn with its own class. Treating
    /// `max_tokens` as `EndTurn` is what let a cut-off answer settle as done in
    /// the TUI, and it would make this bridge report success for a reply whose
    /// tail was never produced.
    #[test]
    fn max_tokens_and_refusal_are_failures_not_finishes() {
        for (stop, truncated) in [
            (schema::StopReason::MaxTokens, true),
            (schema::StopReason::Refusal, false),
        ] {
            let idle = schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(
                schema::IdleStateUpdate::new().stop_reason(Some(stop.clone())),
            ));
            let mapped = to_agent_updates(&idle, &parent());
            assert!(
                matches!(
                    &mapped[..],
                    [
                        AgentUpdate::RunFailed {
                            retryable: false,
                            ..
                        },
                        AgentUpdate::Error(_),
                        AgentUpdate::TaskCancelled
                    ]
                ),
                "{stop:?} must fail the run, not finish it: {mapped:?}"
            );
            let kind = mapped
                .iter()
                .find_map(|update| match update {
                    AgentUpdate::Error(kind) => Some(kind),
                    _ => None,
                })
                .expect("the failure carries its class");
            if truncated {
                assert!(
                    matches!(kind, AgentErrorKind::Truncated(_)),
                    "max_tokens must stay a truncation, got {kind:?}"
                );
            } else {
                assert!(
                    matches!(kind, AgentErrorKind::Refused(_)),
                    "refusal must stay a refusal, got {kind:?}"
                );
            }
        }
    }

    #[test]
    fn a_cancelled_idle_also_ends_the_turn() {
        let idle = schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(
            schema::IdleStateUpdate::new().stop_reason(Some(schema::StopReason::Cancelled)),
        ));
        let mapped = to_agent_updates(&idle, &parent());
        assert!(
            matches!(
                &mapped[..],
                [AgentUpdate::RunFinished { .. }, AgentUpdate::TaskCancelled]
            ),
            "{mapped:?}"
        );
    }

    #[test]
    fn a_plan_update_becomes_the_task_list_the_desktop_shows() {
        let plan = schema::SessionUpdate::PlanUpdate(schema::PlanUpdate::new(
            schema::PlanUpdateContent::Items(schema::PlanItems::new(
                "tasks",
                vec![
                    schema::PlanEntry::new(
                        "read the file",
                        schema::PlanEntryPriority::High,
                        schema::PlanEntryStatus::Completed,
                    ),
                    schema::PlanEntry::new(
                        "edit it",
                        schema::PlanEntryPriority::High,
                        schema::PlanEntryStatus::InProgress,
                    ),
                ],
            )),
        ));
        let mapped = to_agent_updates(&plan, &parent());
        assert!(
            matches!(
                &mapped[..],
                [AgentUpdate::TasksChanged { tasks, reason }]
                    if tasks.len() == 2
                        && tasks[0].subject == "read the file"
                        && tasks[1].status == tact_protocol::TaskStatusSnapshot::InProgress
                        && *reason == TasksChangeReason::Updated
            ),
            "{mapped:?}"
        );
    }

    #[test]
    fn usage_becomes_the_context_meter_reading() {
        let usage = schema::SessionUpdate::UsageUpdate(schema::UsageUpdate::new(1_500, 128_000));
        assert!(matches!(
            &to_agent_updates(&usage, &parent())[..],
            [AgentUpdate::TokenUsage(info)] if info.total == 1_500
        ));
    }

    #[test]
    fn an_error_notice_is_an_error_row() {
        let notice = schema::SessionUpdate::Notice(
            schema::Notice::new(schema::NoticeSeverity::Error, "Model call failed")
                .description("connection reset"),
        );
        assert!(matches!(
            &to_agent_updates(&notice, &parent())[..],
            [AgentUpdate::Error(AgentErrorKind::Other(message))] if message == "connection reset"
        ));
    }

    #[test]
    fn an_informational_notice_keeps_its_body() {
        let notice = schema::SessionUpdate::Notice(
            schema::Notice::new(schema::NoticeSeverity::Info, "Stats").description("turns: 3"),
        );
        assert!(matches!(
            &to_agent_updates(&notice, &parent())[..],
            [AgentUpdate::MdInfo(body)] if body == "turns: 3"
        ));
    }

    #[test]
    fn one_compaction_is_one_typed_event_per_status() {
        let started = schema::SessionUpdate::CompactionUpdate(
            schema::CompactionUpdate::new("c1").status(schema::CompactionStatus::InProgress),
        );
        assert!(matches!(
            &to_agent_updates(&started, &parent())[..],
            [AgentUpdate::Compaction(CompactionEvent {
                state: CompactionState::Started,
                ..
            })]
        ));

        let done = schema::SessionUpdate::CompactionUpdate(
            schema::CompactionUpdate::new("c1")
                .status(schema::CompactionStatus::Completed)
                .summary(vec![schema::ContentBlock::Text(schema::TextContent::new(
                    "summary text",
                ))]),
        );
        assert!(matches!(
            &to_agent_updates(&done, &parent())[..],
            [AgentUpdate::Compaction(CompactionEvent {
                state: CompactionState::Completed { summary: Some(body) },
                ..
            })] if body == "summary text"
        ));
    }

    #[test]
    fn an_empty_plan_empties_the_task_list() {
        let plan = schema::SessionUpdate::PlanUpdate(schema::PlanUpdate::new(
            schema::PlanUpdateContent::Items(schema::PlanItems::new("tasks", vec![])),
        ));
        assert!(matches!(
            &to_agent_updates(&plan, &parent())[..],
            [AgentUpdate::TasksChanged { tasks, .. }] if tasks.is_empty()
        ));
    }

    /// The agent's explicit "plan is over". A client that only handled an
    /// empty `plan_update` would keep the finished checklist on screen, which
    /// is the bug the agent-side `plan_removed` exists to avoid.
    #[test]
    fn a_removed_plan_empties_the_task_list() {
        let removed = schema::SessionUpdate::PlanRemoved(schema::PlanRemoved::new("tasks"));
        assert!(matches!(
            &to_agent_updates(&removed, &parent())[..],
            [AgentUpdate::TasksChanged { tasks, .. }] if tasks.is_empty()
        ));
    }

    /// `session/resume` replays history as whole messages, not chunks, so an
    /// unhandled `agent_message` opened a transcript with no answers in it.
    #[test]
    fn a_whole_agent_message_becomes_one_replayed_answer() {
        let message = schema::SessionUpdate::AgentMessage(
            schema::AgentMessage::new("replay-1").content(vec![schema::ContentBlock::Text(
                schema::TextContent::new("earlier answer"),
            )]),
        );
        assert!(matches!(
            &to_agent_updates(&message, &parent())[..],
            [
                AgentUpdate::MessageStarted { message_id },
                AgentUpdate::StreamChunk(text),
                AgentUpdate::MessageFinished { .. }
            ] if message_id == "replay-1" && text == "earlier answer"
        ));
    }

    #[test]
    fn a_whole_thought_message_becomes_one_thinking_region() {
        let thought = schema::SessionUpdate::AgentThought(
            schema::AgentThought::new("replay-1").content(vec![schema::ContentBlock::Text(
                schema::TextContent::new("earlier reasoning"),
            )]),
        );
        let mapped = to_agent_updates(&thought, &parent());
        assert!(
            matches!(
                &mapped[..],
                [
                    AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Started),
                    AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(text)),
                    AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Finished)
                ] if text == "earlier reasoning"
            ),
            "{mapped:?}"
        );
    }
}
