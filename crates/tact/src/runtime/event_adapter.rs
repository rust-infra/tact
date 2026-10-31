//! Kernel event to host-protocol bridge.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
};

use tact_agent_api::{
    AgentEvent, AgentNotice, EventDelivery, EventPort, MessageDelta, ModelCallKind,
    ProviderActivity, ProviderActivityKind, ProviderActivityPhase, ThinkingDelta, ToolDescriptor,
    ToolExecutionResult, ToolExecutionStatus, ToolOutputChunk,
};
use tact_protocol::{
    AgentUpdate, ModelCallParams, PlanStep, StepResult, StepStatus, ThinkingChunk,
    ToolOutputChunk as HostToolOutputChunk, ToolPresentationInfo, UpdateSink,
};

use super::{
    host_neutral_token_usage,
    tool_presentation::{presentation_from_metadata, step_detail_for_presentation},
};

#[derive(Clone)]
struct TrackedTool {
    idx: usize,
    tool_name: String,
    arg_summary: String,
    arg_full: String,
    presentation: ToolPresentationInfo,
    input: serde_json::Value,
}

/// Forwards kernel events exactly once to the existing `AgentUpdate` host.
pub(crate) struct TactEventBridge {
    events: UpdateSink,
    state: Mutex<BridgeState>,
}

struct BridgeState {
    next_step: usize,
    tools: HashMap<String, TrackedTool>,
    activity_started: HashSet<String>,
    activity_terminal: HashSet<String>,
}

impl TactEventBridge {
    pub(crate) fn new(events: UpdateSink) -> Self {
        Self {
            events,
            state: Mutex::new(BridgeState {
                next_step: 0,
                tools: HashMap::new(),
                activity_started: HashSet::new(),
                activity_terminal: HashSet::new(),
            }),
        }
    }

    fn track_tool(
        &self,
        state: &mut BridgeState,
        call_id: &str,
        call_name: &str,
        input: &serde_json::Value,
        descriptor: Option<&ToolDescriptor>,
    ) -> TrackedTool {
        if let Some(tracked) = state.tools.get(call_id) {
            return tracked.clone();
        }
        let idx = state.next_step;
        state.next_step += 1;
        let (arg_full, arg_summary, presentation) =
            descriptor.map(descriptor_host_fields).unwrap_or_else(|| {
                (
                    input.to_string(),
                    generic_arg_summary(input),
                    ToolPresentationInfo::generic(call_name),
                )
            });
        let tracked = TrackedTool {
            idx,
            tool_name: call_name.to_string(),
            arg_summary,
            arg_full: arg_full.clone(),
            presentation,
            input: input.clone(),
        };
        state.tools.insert(call_id.to_string(), tracked.clone());
        self.events.send(AgentUpdate::StepAdded(PlanStep::new(
            if tracked.arg_summary.is_empty() {
                tracked.tool_name.clone()
            } else {
                format!("{} ({})", tracked.tool_name, tracked.arg_summary)
            },
            tracked.tool_name.clone(),
            call_id.to_string(),
            input.as_object().cloned().unwrap_or_default(),
        )));
        self.events.send(AgentUpdate::StepStarted {
            idx,
            tool_id: call_id.to_string(),
            tool_name: tracked.tool_name.clone(),
            arg_summary: tracked.arg_summary.clone(),
            arg_full: tracked.arg_full.clone(),
            presentation: tracked.presentation.clone(),
        });
        tracked
    }

    fn forward_activity(&self, state: &mut BridgeState, activity: ProviderActivity) {
        let ProviderActivityKind::WebSearch {
            phase,
            query,
            failure,
            duration,
        } = &activity.kind;
        if *phase == ProviderActivityPhase::Started
            && !state.activity_started.insert(activity.id.clone())
        {
            return;
        }
        if *phase != ProviderActivityPhase::Started {
            if !state.activity_terminal.insert(activity.id.clone()) {
                return;
            }
            if !state.activity_started.contains(&activity.id) {
                self.events.send(web_search_started(
                    &activity.id,
                    activity.step,
                    query.as_deref().unwrap_or_default(),
                ));
                state.activity_started.insert(activity.id.clone());
            }
        }

        match phase {
            ProviderActivityPhase::Started => self.events.send(web_search_started(
                &activity.id,
                activity.step,
                query.as_deref().unwrap_or_default(),
            )),
            ProviderActivityPhase::Completed => self.events.send(web_search_finished(
                &activity.id,
                activity.step,
                query.as_deref().unwrap_or_default(),
                *duration,
            )),
            ProviderActivityPhase::Failed => self.events.send(AgentUpdate::StepFailed {
                idx: activity.step as usize,
                run_id: None,
                tool_id: activity.id.clone(),
                arg_summary: query.clone().unwrap_or_default(),
                error: failure
                    .clone()
                    .unwrap_or_else(|| "web search failed".into()),
            }),
        }
    }
}

impl EventPort for TactEventBridge {
    fn emit(&self, event: AgentEvent) {
        let delivery = event.delivery();
        self.emit_with_delivery(event, delivery);
    }

    fn emit_with_delivery(&self, event: AgentEvent, delivery: EventDelivery) {
        let _ = delivery;
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.emit_inner(&mut state, event);
    }
}

impl TactEventBridge {
    fn emit_inner(&self, state: &mut BridgeState, event: AgentEvent) {
        match event {
            AgentEvent::RunStarted { run_id, .. } => {
                self.events.send(AgentUpdate::RunStarted {
                    run_id: run_id.to_string(),
                });
            }
            AgentEvent::RunFinished { run, .. } => {
                self.events.send(AgentUpdate::RunFinished {
                    run_id: run.run_id.to_string(),
                });
            }
            AgentEvent::RunFailed { run, .. } => {
                self.events.send(AgentUpdate::RunFinished {
                    run_id: run.run_id.to_string(),
                });
            }
            AgentEvent::MessageStarted { message_id, .. } => {
                self.events.send(AgentUpdate::MessageStarted {
                    message_id: message_id.to_string(),
                });
            }
            AgentEvent::MessageFinished { .. } => {}
            AgentEvent::TurnStarted {
                turn,
                max_total_turns,
                ..
            } => {
                self.events.send(AgentUpdate::TurnStats {
                    turns_taken: turn,
                    max_turns: max_total_turns,
                });
            }
            AgentEvent::TurnFinished { .. } => {}
            AgentEvent::ModelCallStarted {
                kind: ModelCallKind::Stream | ModelCallKind::Complete,
                model,
                max_output_tokens,
                options,
                ..
            } => {
                self.events.send(AgentUpdate::ModelInfo(ModelCallParams {
                    model,
                    max_tokens: max_output_tokens,
                    thinking_budget: options.thinking_budget,
                    reasoning_effort: options.reasoning_effort,
                    extra_body: options.extra_value("extra_body").map(|value| {
                        value
                            .as_str()
                            .map(str::to_string)
                            .unwrap_or_else(|| value.to_string())
                    }),
                }));
            }
            AgentEvent::ModelCallStarted { .. } => {}
            AgentEvent::MessageDelta { delta, .. } => match delta {
                MessageDelta::Text { delta } => {
                    self.events.send_progress(AgentUpdate::StreamChunk(delta));
                }
                MessageDelta::Thinking { phase } => {
                    self.events
                        .send_progress(AgentUpdate::ThinkingChunk(match phase {
                            ThinkingDelta::Started => ThinkingChunk::Started,
                            ThinkingDelta::Delta { delta } => ThinkingChunk::Delta(delta),
                            ThinkingDelta::Finished => ThinkingChunk::Finished,
                        }));
                }
            },
            AgentEvent::ProviderActivity { activity } => {
                self.forward_activity(state, activity);
            }
            AgentEvent::ToolStarted { call, descriptor } => {
                self.track_tool(
                    state,
                    &call.id,
                    &call.name,
                    &call.input,
                    descriptor.as_ref(),
                );
            }
            AgentEvent::ToolProgress {
                run_id,
                call_id,
                chunks,
            } => {
                self.events.send_progress(AgentUpdate::ToolProgress {
                    run_id: Some(run_id.to_string()),
                    tool_id: call_id,
                    chunks: chunks.into_iter().map(host_chunk).collect(),
                });
            }
            AgentEvent::ToolFinished {
                run_id,
                call_id,
                result,
                permission_label,
                duration,
            } => {
                let tracked = state.tools.remove(&call_id).unwrap_or_else(|| {
                    let name = call_id.clone();
                    TrackedTool {
                        idx: state.next_step,
                        tool_name: name.clone(),
                        arg_summary: String::new(),
                        arg_full: String::new(),
                        presentation: ToolPresentationInfo::generic(name),
                        input: serde_json::Value::Null,
                    }
                });
                match result.status {
                    ToolExecutionStatus::Succeeded => {
                        let output = result_text(&result);
                        let detail = step_detail_for_presentation(
                            &tracked.presentation.detail,
                            &tracked.input,
                            &output,
                            StepStatus::Success,
                        );
                        self.events.send(AgentUpdate::StepFinished {
                            idx: tracked.idx,
                            run_id: Some(run_id.to_string()),
                            tool_id: call_id,
                            result: StepResult {
                                tool: tracked.tool_name,
                                arg_summary: tracked.arg_summary,
                                arg_full: Some(tracked.arg_full),
                                status: StepStatus::Success,
                                detail,
                                message: output,
                                duration_us: Some(duration_micros(duration)),
                                permission_label,
                                presentation: tracked.presentation,
                            },
                        });
                    }
                    ToolExecutionStatus::Failed => {
                        let error = result
                            .error
                            .as_ref()
                            .map(|error| error.message.clone())
                            .unwrap_or_else(|| result_text(&result));
                        self.events.send(AgentUpdate::StepFailed {
                            idx: tracked.idx,
                            run_id: Some(run_id.to_string()),
                            tool_id: call_id,
                            arg_summary: tracked.arg_summary,
                            error,
                        });
                    }
                }
            }
            AgentEvent::Usage { usage, .. } => {
                if let Some(usage) = usage {
                    self.events
                        .send(AgentUpdate::TokenUsage(host_neutral_token_usage(&usage)));
                }
            }
            AgentEvent::ContextCompacted { kind } => {
                self.events.send(AgentUpdate::Info(format!(
                    "Compaction complete ({kind:?})."
                )));
            }
            AgentEvent::Notice { notice } => self.emit_notice(notice),
        }
    }

    fn emit_notice(&self, notice: AgentNotice) {
        let message = match notice {
            AgentNotice::TurnLimitReached { max } => {
                format!("[max turns] stopped after {max} turn(s)")
            }
            AgentNotice::CancellationRequested => "Cancelled by user".to_string(),
            AgentNotice::RecoveryBackoff {
                attempt,
                max_attempts,
                delay,
                summary,
            } => format!(
                "[Recovery] backoff ({attempt}/{max_attempts}): retrying in {:.1}s — {summary}",
                delay.as_secs_f64()
            ),
            AgentNotice::Continuation {
                attempt,
                max_attempts,
            } => format!("[Recovery] continue ({attempt}/{max_attempts}): output truncated"),
            AgentNotice::CompactionStarted { kind } => match kind {
                tact_agent_api::CompactionKind::Auto => "[auto compact]".to_string(),
                tact_agent_api::CompactionKind::Manual => "[manual compact]".to_string(),
                tact_agent_api::CompactionKind::Recovery => {
                    "[Recovery] compact: context too large".to_string()
                }
                tact_agent_api::CompactionKind::Command => "[compact]".to_string(),
                tact_agent_api::CompactionKind::Micro => "[micro compact]".to_string(),
            },
        };
        self.events.send(AgentUpdate::Info(message));
    }
}

fn descriptor_host_fields(descriptor: &ToolDescriptor) -> (String, String, ToolPresentationInfo) {
    let arg_full = descriptor
        .metadata
        .get("arg_full")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let arg_summary = descriptor
        .metadata
        .get("arg_summary")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();
    let presentation =
        presentation_from_metadata(&descriptor.name, descriptor.metadata.get("presentation"));
    (arg_full, arg_summary, presentation)
}

fn generic_arg_summary(input: &serde_json::Value) -> String {
    let full = input.to_string();
    if full.chars().count() <= 120 {
        full
    } else {
        format!("{}...", full.chars().take(117).collect::<String>())
    }
}

fn result_text(result: &ToolExecutionResult) -> String {
    let mut text = result
        .outcome
        .content
        .iter()
        .filter_map(|block| match block {
            tact_agent_api::AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    if text.is_empty()
        && let Some(error) = &result.error
    {
        text = error.message.clone();
    }
    text
}

fn host_chunk(chunk: ToolOutputChunk) -> HostToolOutputChunk {
    match chunk.stream {
        tact_agent_api::ToolOutputStream::Stdout => HostToolOutputChunk::stdout(chunk.text),
        tact_agent_api::ToolOutputStream::Stderr => HostToolOutputChunk::stderr(chunk.text),
        tact_agent_api::ToolOutputStream::Other => HostToolOutputChunk::other(chunk.text),
    }
}

fn web_search_started(id: &str, step: u32, query: &str) -> AgentUpdate {
    AgentUpdate::StepStarted {
        idx: step as usize,
        tool_id: id.to_string(),
        tool_name: "web_search".to_string(),
        arg_summary: query.to_string(),
        arg_full: query.to_string(),
        presentation: ToolPresentationInfo::generic("web_search"),
    }
}

fn web_search_finished(
    id: &str,
    step: u32,
    query: &str,
    duration: Option<std::time::Duration>,
) -> AgentUpdate {
    AgentUpdate::StepFinished {
        idx: step as usize,
        run_id: None,
        tool_id: id.to_string(),
        result: StepResult {
            tool: "web_search".to_string(),
            arg_summary: query.to_string(),
            arg_full: None,
            status: StepStatus::Success,
            message: "web search completed".to_string(),
            detail: None,
            duration_us: duration.map(duration_micros),
            permission_label: None,
            presentation: ToolPresentationInfo::generic("web_search"),
        },
    }
}

fn duration_micros(duration: std::time::Duration) -> u64 {
    duration.as_micros().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use tact_agent_api::{
        AgentMessage, AgentRole, CancelToken, CursorUpdate, EventPort, MessageDelta, MessageId,
        MessageOrigin, ModelEvent, ProviderActivity, ProviderActivityKind, ProviderActivityPhase,
        StopReason, TokenUsage, ToolCall, ToolExecutionError, ToolExecutionErrorKind,
        ToolExecutionResult, ToolExecutionStatus, ToolOutcome, ToolOutputChunk as NeutralChunk,
        ToolOutputStream as NeutralStream,
    };
    use tokio::sync::mpsc;

    use super::*;

    fn bridge() -> (Arc<TactEventBridge>, mpsc::UnboundedReceiver<AgentUpdate>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let sink = UpdateSink::for_ui(tx);
        (Arc::new(TactEventBridge::new(sink)), rx)
    }

    fn drain(rx: &mut mpsc::UnboundedReceiver<AgentUpdate>) -> Vec<AgentUpdate> {
        let mut updates = Vec::new();
        while let Ok(update) = rx.try_recv() {
            updates.push(update);
        }
        updates
    }

    fn run_context(run_id: &str) -> tact_agent_api::RunContext {
        tact_agent_api::RunContext {
            session: tact_agent_api::SessionRef::new("session-1"),
            run_id: tact_agent_api::RunId::new(run_id),
        }
    }

    #[test]
    fn run_started_precedes_first_turn_stats() {
        let (bridge, mut rx) = bridge();
        bridge.emit(AgentEvent::RunStarted {
            run_id: tact_agent_api::RunId::new("run-1"),
            session: tact_agent_api::SessionRef::new("session-1"),
        });
        bridge.emit(AgentEvent::TurnStarted {
            run_id: tact_agent_api::RunId::new("run-1"),
            turn: 1,
            activation_turn: 1,
            max_total_turns: None,
        });

        let updates = drain(&mut rx);
        assert!(matches!(
            &updates[0],
            AgentUpdate::RunStarted { run_id } if run_id == "run-1"
        ));
        assert!(matches!(
            &updates[1],
            AgentUpdate::TurnStats { turns_taken: 1, .. }
        ));
    }

    #[test]
    fn run_finished_closes_the_run_cursor() {
        let (bridge, mut rx) = bridge();
        bridge.emit(AgentEvent::RunFinished {
            outcome: tact_agent_api::RunOutcome {
                stop: tact_agent_api::RunStop::Completed,
                turns: 1,
                total_turns: 1,
                tool_calls: 0,
                recovery: Default::default(),
            },
            run: run_context("run-1"),
        });
        bridge.emit(AgentEvent::RunFailed {
            error: tact_agent_api::RuntimeErrorInfo::new(
                tact_agent_api::RunPhase::Commit,
                "failed",
            ),
            run: run_context("run-2"),
        });

        let updates = drain(&mut rx);
        assert!(matches!(
            &updates[0],
            AgentUpdate::RunFinished { run_id } if run_id == "run-1"
        ));
        assert!(matches!(
            &updates[1],
            AgentUpdate::RunFinished { run_id } if run_id == "run-2"
        ));
    }

    #[test]
    fn compaction_run_does_not_open_an_activation() {
        let (bridge, mut rx) = bridge();
        bridge.emit(AgentEvent::RunStarted {
            run_id: tact_agent_api::RunId::new("compact-1"),
            session: tact_agent_api::SessionRef::new("session-1"),
        });
        bridge.emit(AgentEvent::ContextCompacted {
            kind: tact_agent_api::CompactionKind::Command,
        });
        bridge.emit(AgentEvent::RunFinished {
            outcome: tact_agent_api::RunOutcome {
                stop: tact_agent_api::RunStop::Completed,
                turns: 0,
                total_turns: 0,
                tool_calls: 0,
                recovery: Default::default(),
            },
            run: run_context("compact-1"),
        });

        let updates = drain(&mut rx);
        assert!(!updates.iter().any(|update| {
            matches!(
                update,
                AgentUpdate::TurnStats { .. } | AgentUpdate::StreamChunk(_)
            )
        }));
        assert!(matches!(
            &updates[0],
            AgentUpdate::RunStarted { run_id } if run_id == "compact-1"
        ));
        assert!(matches!(
            updates.last(),
            Some(AgentUpdate::RunFinished { run_id }) if run_id == "compact-1"
        ));
    }

    #[test]
    fn message_started_precedes_first_stream_chunk_when_provided() {
        let (bridge, mut rx) = bridge();
        bridge.emit(AgentEvent::MessageStarted {
            message_id: MessageId::new("message-1"),
            role: AgentRole::Assistant,
        });
        bridge.emit(AgentEvent::MessageDelta {
            message_id: MessageId::new("message-1"),
            delta: MessageDelta::Text {
                delta: "hello".into(),
            },
        });

        let updates = drain(&mut rx);
        assert!(matches!(
            &updates[0],
            AgentUpdate::MessageStarted { message_id } if message_id == "message-1"
        ));
        assert!(matches!(
            &updates[1],
            AgentUpdate::StreamChunk(text) if text == "hello"
        ));
    }

    #[test]
    fn maps_turn_model_deltas_usage_and_notices() {
        let (bridge, mut rx) = bridge();
        bridge.emit(AgentEvent::TurnStarted {
            run_id: tact_agent_api::RunId::new("run-1"),
            turn: 2,
            activation_turn: 1,
            max_total_turns: Some(4),
        });
        bridge.emit(AgentEvent::ModelCallStarted {
            call_id: "call-1".into(),
            kind: ModelCallKind::Stream,
            model: "mock".into(),
            max_output_tokens: 100,
            options: tact_agent_api::ModelCallOptions {
                thinking_budget: Some(8),
                ..Default::default()
            },
        });
        bridge.emit(AgentEvent::MessageDelta {
            message_id: MessageId::new("m1"),
            delta: MessageDelta::Text {
                delta: "hello".into(),
            },
        });
        bridge.emit(AgentEvent::MessageDelta {
            message_id: MessageId::new("m1"),
            delta: MessageDelta::Thinking {
                phase: tact_agent_api::ThinkingDelta::Delta {
                    delta: "considering".into(),
                },
            },
        });
        bridge.emit(AgentEvent::Usage {
            call_id: "call-1".into(),
            run: None,
            kind: ModelCallKind::Stream,
            usage: Some(TokenUsage {
                input_tokens: 2,
                output_tokens: 3,
                total_tokens: 5,
                ..TokenUsage::default()
            }),
            message_range: None,
        });
        bridge.emit(AgentEvent::Notice {
            notice: AgentNotice::Continuation {
                attempt: 1,
                max_attempts: 3,
            },
        });

        let updates = drain(&mut rx);
        assert!(matches!(
            &updates[0],
            AgentUpdate::TurnStats {
                turns_taken: 2,
                max_turns: Some(4)
            }
        ));
        assert!(matches!(
            &updates[1],
            AgentUpdate::ModelInfo(params)
                if params.model == "mock"
                    && params.max_tokens == 100
                    && params.thinking_budget == Some(8)
        ));
        assert!(matches!(
            &updates[2],
            AgentUpdate::StreamChunk(text) if text == "hello"
        ));
        assert!(matches!(
            &updates[3],
            AgentUpdate::ThinkingChunk(tact_protocol::ThinkingChunk::Delta(text))
                if text == "considering"
        ));
        assert!(matches!(
            &updates[4],
            AgentUpdate::TokenUsage(usage) if usage.total == 5
        ));
        assert!(matches!(
            &updates[5],
            AgentUpdate::Info(text) if text.contains("1/3")
        ));
    }

    #[test]
    fn maps_tool_lifecycle_progress_and_failures_exactly_once() {
        let (bridge, mut rx) = bridge();
        let call = ToolCall {
            id: "call-tool".into(),
            name: "read_file".into(),
            input: serde_json::json!({"path": "README.md"}),
        };
        bridge.emit(AgentEvent::ToolStarted {
            call: call.clone(),
            descriptor: Some(ToolDescriptor {
                name: "read_file".into(),
                description: "read".into(),
                metadata: serde_json::json!({
                    "arg_full": "README.md",
                    "arg_summary": "README.md",
                    "presentation": {
                        "visual_kind": "file_read",
                        "display_name": "Read",
                        "keep_full_live_output": false,
                        "detail": {"kind": "result"},
                        "popup": "none",
                        "compact_result_to_meta": false,
                        "keep_live": false
                    }
                }),
            }),
        });
        bridge.emit(AgentEvent::ToolProgress {
            run_id: tact_agent_api::RunId::new("run-1"),
            call_id: call.id.clone(),
            chunks: vec![NeutralChunk {
                stream: NeutralStream::Stdout,
                text: "live".into(),
            }],
        });
        bridge.emit(AgentEvent::ToolFinished {
            run_id: tact_agent_api::RunId::new("run-1"),
            call_id: call.id.clone(),
            result: ToolExecutionResult::success(ToolOutcome::text("done")),
            permission_label: Some("Allow once".into()),
            duration: Duration::from_millis(4),
        });
        bridge.emit(AgentEvent::ToolStarted {
            call: ToolCall {
                id: "call-fail".into(),
                name: "bash".into(),
                input: serde_json::json!({"command": "false"}),
            },
            descriptor: None,
        });
        bridge.emit(AgentEvent::ToolFinished {
            run_id: tact_agent_api::RunId::new("run-1"),
            call_id: "call-fail".into(),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Failed,
                outcome: ToolOutcome::text("denied"),
                error: Some(ToolExecutionError {
                    kind: ToolExecutionErrorKind::Tool,
                    message: "denied".into(),
                }),
            },
            permission_label: None,
            duration: Duration::ZERO,
        });

        let updates = drain(&mut rx);
        assert!(matches!(updates[0], AgentUpdate::StepAdded(_)));
        assert!(matches!(
            &updates[1],
            AgentUpdate::StepStarted { arg_summary, .. } if arg_summary == "README.md"
        ));
        assert!(matches!(
            &updates[2],
            AgentUpdate::ToolProgress { chunks, .. }
                if chunks.len() == 1 && chunks[0].text == "live"
        ));
        assert!(matches!(
            &updates[3],
            AgentUpdate::StepFinished { result, .. }
                if result.message == "done"
                    && result.permission_label.as_deref() == Some("Allow once")
        ));
        assert!(matches!(&updates[4], AgentUpdate::StepAdded(_)));
        assert!(matches!(&updates[5], AgentUpdate::StepStarted { .. }));
        assert!(matches!(
            &updates[6],
            AgentUpdate::StepFailed { error, .. } if error == "denied"
        ));
    }

    #[test]
    fn provider_activity_is_ordered_and_duplicate_terminals_are_suppressed() {
        let (bridge, mut rx) = bridge();
        for phase in [
            ProviderActivityPhase::Completed,
            ProviderActivityPhase::Completed,
        ] {
            bridge.emit(AgentEvent::ProviderActivity {
                activity: ProviderActivity {
                    id: "search-1".into(),
                    step: 3,
                    kind: ProviderActivityKind::WebSearch {
                        phase,
                        query: Some("rust".into()),
                        failure: None,
                        duration: Some(Duration::from_millis(2)),
                    },
                },
            });
        }

        let updates = drain(&mut rx);
        assert_eq!(updates.len(), 2);
        assert!(matches!(
            &updates[0],
            AgentUpdate::StepStarted { tool_name, .. } if tool_name == "web_search"
        ));
        assert!(matches!(
            &updates[1],
            AgentUpdate::StepFinished { result, .. }
                if result.tool == "web_search" && result.duration_us == Some(2_000)
        ));
    }

    #[test]
    fn a_synthetic_tool_finish_without_start_still_produces_a_terminal_update() {
        let (bridge, mut rx) = bridge();
        bridge.emit(AgentEvent::ToolFinished {
            run_id: tact_agent_api::RunId::new("run-1"),
            call_id: "orphan".into(),
            result: ToolExecutionResult::success(ToolOutcome::text("ok")),
            permission_label: None,
            duration: Duration::ZERO,
        });
        let updates = drain(&mut rx);
        assert!(matches!(
            &updates[0],
            AgentUpdate::StepFinished { tool_id, .. } if tool_id == "orphan"
        ));
    }

    #[tokio::test]
    async fn injected_context_and_hosted_activity_keep_separate_non_user_channels() {
        let (bridge, mut rx) = bridge();
        let response = tact_agent_api::ModelResponse {
            message_id: MessageId::new("assistant-1"),
            message: AgentMessage::new(
                AgentRole::Assistant,
                MessageOrigin::Assistant,
                vec![tact_agent_api::AgentContentBlock::Text {
                    text: "ready".into(),
                }],
            ),
            stop_reason: StopReason::EndTurn,
            usage: None,
            request_body: None,
            cursor: CursorUpdate::Unchanged,
        };
        let model = tact_agent_core::test_support::ScriptedModel::new(vec![
            tact_agent_core::test_support::ModelScript::Events(vec![
                Ok(ModelEvent::MessageStarted {
                    message_id: response.message_id.clone(),
                    role: AgentRole::Assistant,
                }),
                Ok(ModelEvent::ProviderActivity {
                    activity: ProviderActivity {
                        id: "search-1".into(),
                        step: 0,
                        kind: ProviderActivityKind::WebSearch {
                            phase: ProviderActivityPhase::Started,
                            query: Some("briefing".into()),
                            failure: None,
                            duration: None,
                        },
                    },
                }),
                Ok(ModelEvent::MessageDelta {
                    message_id: response.message_id.clone(),
                    delta: MessageDelta::Text {
                        delta: "ready".into(),
                    },
                }),
                Ok(ModelEvent::ProviderActivity {
                    activity: ProviderActivity {
                        id: "search-1".into(),
                        step: 0,
                        kind: ProviderActivityKind::WebSearch {
                            phase: ProviderActivityPhase::Completed,
                            query: Some("briefing".into()),
                            failure: None,
                            duration: Some(Duration::from_millis(3)),
                        },
                    },
                }),
                Ok(ModelEvent::Finished(response)),
            ]),
        ]);
        let context = tact_agent_core::test_support::ScriptedContext::new();
        context.push_run(tact_agent_core::test_support::RunPreparation::Continue {
            appended: vec![AgentMessage::text(
                AgentRole::User,
                MessageOrigin::InjectedContext {
                    source: "hook".into(),
                },
                "hook briefing",
            )],
        });
        let usage = Arc::new(tact_agent_api::test_support::RecordingUsageSink::new());
        let kernel = tact_agent_core::AgentKernel::new(tact_agent_core::KernelDeps {
            model,
            tools: tact_agent_core::test_support::ScriptedTools::new(Vec::new()),
            gate: tact_agent_core::test_support::ScriptedGate::new(),
            context,
            transcript: Arc::new(tact_agent_api::test_support::InMemoryTranscriptStore::new()),
            usage,
            events: bridge,
        });
        let mut state = tact_agent_api::AgentState {
            initialized: true,
            ..tact_agent_api::AgentState::default()
        };
        kernel
            .run(
                &mut state,
                tact_agent_api::RunInput {
                    run_id: tact_agent_api::RunId::new("injection-activity"),
                    session: tact_agent_api::SessionRef::new("injection-activity"),
                    inputs: vec![AgentMessage::text(
                        AgentRole::User,
                        MessageOrigin::User,
                        "continue",
                    )],
                    max_total_turns: None,
                },
                CancelToken::new(),
            )
            .await
            .expect("kernel run");

        let updates = drain(&mut rx);
        assert!(state.transcript.iter().any(|message| matches!(
            &message.origin,
            MessageOrigin::InjectedContext { source } if source == "hook"
        )));
        assert_eq!(
            state
                .transcript
                .iter()
                .filter(|message| message.origin == MessageOrigin::User)
                .count(),
            1
        );
        assert!(
            state
                .transcript
                .iter()
                .all(|message| message.origin != MessageOrigin::ToolResult)
        );
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(
                    update,
                    AgentUpdate::StepStarted { tool_name, .. } if tool_name == "web_search"
                ))
                .count(),
            1
        );
        assert_eq!(
            updates
                .iter()
                .filter(|update| matches!(
                    update,
                    AgentUpdate::StepFinished { result, .. } if result.tool == "web_search"
                ))
                .count(),
            1
        );
        assert!(
            updates
                .iter()
                .all(|update| !matches!(update, AgentUpdate::StepAdded(_)))
        );
    }

    #[test]
    fn kernel_and_adapter_updates_share_one_sink_exactly_once() {
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = UpdateSink::for_ui(tx);
        let adapter = sink.clone();
        let bridge = TactEventBridge::new(sink);

        adapter.send(AgentUpdate::HookStatus {
            id: 1,
            source: Some("hook".into()),
            message: "running".into(),
            elapsed_ms: None,
        });
        bridge.emit(AgentEvent::TurnStarted {
            run_id: tact_agent_api::RunId::new("run-1"),
            turn: 1,
            activation_turn: 1,
            max_total_turns: None,
        });
        adapter.send(AgentUpdate::HookContext {
            source: Some("plugin".into()),
            text: "briefing".into(),
        });
        adapter.send(AgentUpdate::RequestSelect {
            request_id: 7,
            prompt: "choose".into(),
            options: vec!["a".into(), "b".into()],
            log_confirm: false,
            from_subagent: false,
            purpose: tact_protocol::UiRequestPurpose::Question,
            origin: tact_protocol::UiRequestOrigin::main(),
        });
        adapter.send(AgentUpdate::TasksChanged {
            tasks: Vec::new(),
            reason: tact_protocol::TasksChangeReason::Updated,
        });
        adapter.send(AgentUpdate::GoalsChanged { goals: Vec::new() });
        adapter.send(AgentUpdate::ToolMeta {
            tool_id: "tool-1".into(),
            model: Some("mock".into()),
            token_usage: None,
            context_window: None,
            task_id: None,
            subagent_id: None,
        });
        adapter.send(AgentUpdate::BackgroundTaskFinished {
            tool_id: "tool-1".into(),
            success: true,
            message: "done".into(),
            output: "done".into(),
        });
        adapter.send(AgentUpdate::SubagentFinished {
            tool_id: "tool-1".into(),
            child_id: "child-1".into(),
            success: true,
            summary: "done".into(),
        });
        adapter.send(AgentUpdate::SubagentsChanged { runs: Vec::new() });

        let updates = drain(&mut rx);
        assert_eq!(updates.len(), 10);
        assert!(matches!(updates[0], AgentUpdate::HookStatus { .. }));
        assert!(matches!(
            updates[1],
            AgentUpdate::TurnStats {
                turns_taken: 1,
                max_turns: None
            }
        ));
        assert!(matches!(updates[2], AgentUpdate::HookContext { .. }));
        assert!(matches!(updates[3], AgentUpdate::RequestSelect { .. }));
        assert!(matches!(updates[4], AgentUpdate::TasksChanged { .. }));
        assert!(matches!(updates[5], AgentUpdate::GoalsChanged { .. }));
        assert!(matches!(updates[6], AgentUpdate::ToolMeta { .. }));
        assert!(matches!(
            updates[7],
            AgentUpdate::BackgroundTaskFinished { .. }
        ));
        assert!(matches!(updates[8], AgentUpdate::SubagentFinished { .. }));
        assert!(matches!(updates[9], AgentUpdate::SubagentsChanged { .. }));
    }
}
