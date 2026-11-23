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
//! ACP is a smaller vocabulary than Tact's. Three families have no faithful
//! reverse mapping, and this module is explicit about each rather than
//! inventing structure:
//!
//! - **Hook context and hook status** — no ACP update kind exists.
//! - **`SurfaceSnapshot`** — `/stats`, `/mcp list`, `/hooks list`, the system
//!   prompt and history read-outs arrive as `notice` text, not structure.
//! - **`ToolTranscript`** structure — a child session's entries arrive as
//!   ordinary agent/thought/tool updates under the child id; the parent's
//!   structured popup is rebuilt by the reducer, not by this module.

use agent_client_protocol::v2::schema;
use tact_protocol::{
    AgentErrorKind, AgentUpdate, StepResult, StepStatus, ToolPresentationInfo, ToolVisualKind,
};
use tact_protocol::{CompactionEvent, CompactionKind, CompactionState};

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
        // `PlanRemoved` exists only under ACP's `unstable_plan_operations`
        // feature. Without it an emptied plan arrives as an `Items` list with
        // no entries, which the branch above already turns into an empty
        // `TasksChanged` — what the desktop hides the strip on.
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
/// Tact reports a run start and a turn end as separate events. ACP reports one
/// foreground state, so the transition is what matters: an idle after a
/// running is the turn end, and a running from idle is the turn start.
fn state_update(state: &schema::StateUpdate, context: &UpdateContext<'_>) -> Vec<AgentUpdate> {
    let _ = context;
    match state {
        schema::StateUpdate::Running(_) => vec![AgentUpdate::RunStarted {
            run_id: "acp".to_string(),
        }],
        schema::StateUpdate::Idle(idle) => {
            let mut updates = vec![AgentUpdate::RunFinished {
                run_id: "acp".to_string(),
            }];
            if matches!(
                idle.stop_reason,
                Some(schema::StopReason::Error(_)) | Some(schema::StopReason::Cancelled)
            ) {
                updates.push(AgentUpdate::TaskCancelled);
            }
            updates
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
    let state = match compaction.status {
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
        let started = schema::SessionUpdate::CompactionUpdate(schema::CompactionUpdate::new(
            "c1",
            schema::CompactionStatus::InProgress,
        ));
        assert!(matches!(
            &to_agent_updates(&started, &parent())[..],
            [AgentUpdate::Compaction(CompactionEvent {
                state: CompactionState::Started,
                ..
            })]
        ));

        let done = schema::SessionUpdate::CompactionUpdate(
            schema::CompactionUpdate::new("c1", schema::CompactionStatus::Completed).summary(vec![
                schema::ContentBlock::Text(schema::TextContent::new("summary text")),
            ]),
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
}
