use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use tact_agent_api::test_support::{InMemoryTranscriptStore, RecordingUsageSink};
use tact_agent_api::{
    AgentContentBlock, AgentEvent, AgentImageSource, AgentMessage, AgentRole, AgentState,
    CancelToken, CompactionOutcome, CompactionRequest, ContextReplacement, CursorUpdate,
    HostYieldReason, MessageDelta, MessageOrigin, ModelCallKind, ModelCursor, ModelError,
    ModelErrorKind, ModelEvent, ProviderActivity, ProviderActivityKind, ProviderActivityPhase,
    ResourceAccess, RunInput, RunStop, SessionRef, StopReason, ThinkingDelta, TokenUsage,
    ToolDirective, ToolExecutionError, ToolExecutionErrorKind, ToolExecutionResult,
    ToolExecutionStatus, ToolOutcome, ToolOutputChunk, ToolOutputStream, TranscriptStore,
};

use super::AgentKernel;
use crate::{
    KernelDeps, KernelExtensions,
    test_support::{
        AfterAction, BeforeAction, FailingTranscriptStore, FailingUsageSink, ModelScript,
        RecordingEventPort, RecordingObserver, RecordingTranscriptStore, RequestPreparation,
        RunPreparation, ScriptedContext, ScriptedGate, ScriptedModel, ScriptedTools, StoreCall,
        ToolBehavior, ToolDefinition, events, events_with_deltas, text_response, tool_response,
    },
};

struct Harness {
    kernel: AgentKernel,
    model: Arc<ScriptedModel>,
    tools: Arc<ScriptedTools>,
    gate: Arc<ScriptedGate>,
    context: Arc<ScriptedContext>,
    store: Arc<InMemoryTranscriptStore>,
    usage: Arc<RecordingUsageSink>,
    events: Arc<RecordingEventPort>,
}

fn harness(
    model: Arc<ScriptedModel>,
    tools: Arc<ScriptedTools>,
    gate: Arc<ScriptedGate>,
    context: Arc<ScriptedContext>,
) -> Harness {
    let store = Arc::new(InMemoryTranscriptStore::new());
    let usage = Arc::new(RecordingUsageSink::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model: model.clone(),
        tools: tools.clone(),
        gate: gate.clone(),
        context: context.clone(),
        transcript: store.clone(),
        usage: usage.clone(),
        events: events.clone(),
    });
    Harness {
        kernel,
        model,
        tools,
        gate,
        context,
        store,
        usage,
        events,
    }
}

fn session() -> SessionRef {
    SessionRef::new("test-session")
}

fn state() -> AgentState {
    AgentState {
        initialized: true,
        ..AgentState::default()
    }
}

fn input(text: &str) -> RunInput {
    RunInput {
        session: session(),
        inputs: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::User,
            text,
        )],
        max_total_turns: None,
    }
}

fn success(text: &str) -> ToolExecutionResult {
    ToolExecutionResult::success(ToolOutcome::text(text))
}

fn failure(text: &str) -> ToolExecutionResult {
    ToolExecutionResult::failure(ToolExecutionError {
        kind: ToolExecutionErrorKind::Tool,
        message: text.into(),
    })
}

fn with_directive(status: ToolExecutionStatus, directive: ToolDirective) -> ToolExecutionResult {
    let error = (status == ToolExecutionStatus::Failed).then(|| ToolExecutionError {
        kind: ToolExecutionErrorKind::Tool,
        message: "failed".into(),
    });
    ToolExecutionResult {
        status,
        outcome: ToolOutcome {
            content: vec![AgentContentBlock::Text {
                text: "result".into(),
            }],
            directives: vec![directive],
            metadata: None,
        },
        error,
    }
}

fn tool(name: &str, behavior: ToolBehavior) -> ToolDefinition {
    ToolDefinition::new(name, ResourceAccess::independent(), behavior)
}

fn tool_call<'a>(id: &'a str, name: &'a str) -> (&'a str, &'a str, serde_json::Value) {
    (id, name, serde_json::json!({}))
}

fn tool_finished(h: &Harness, call_id: &str) -> ToolExecutionResult {
    h.events
        .events()
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::ToolFinished {
                call_id: finished,
                result,
                ..
            } if finished == call_id => Some(result),
            _ => None,
        })
        .expect("tool finished event")
}

async fn replacement_case(
    transcript: Option<Vec<AgentMessage>>,
    cursor: CursorUpdate,
) -> (Vec<StoreCall>, AgentState) {
    let store = RecordingTranscriptStore::new();
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Command { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript,
        cursor,
    }));
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context,
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });
    let mut state = state();
    kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();
    (store.calls(), state)
}

#[derive(Clone)]
struct TraceEventPort {
    trace: Arc<Mutex<Vec<&'static str>>>,
}

impl tact_agent_api::EventPort for TraceEventPort {
    fn emit(&self, _event: AgentEvent) {
        self.trace.lock().expect("event trace lock").push("port");
    }
}

struct TraceObserver {
    name: &'static str,
    trace: Arc<Mutex<Vec<&'static str>>>,
}

impl tact_agent_api::EventObserver for TraceObserver {
    fn observe(&self, _event: &AgentEvent) {
        self.trace.lock().expect("event trace lock").push(self.name);
    }
}

#[tokio::test]
async fn end_turn_text_stream_completes() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "hello",
        StopReason::EndTurn,
    ))]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("hi"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.total_turns, 1);
    assert_eq!(state.transcript.len(), 2);
    assert_eq!(state.transcript[1].origin, MessageOrigin::Assistant);
    assert_eq!(h.store.transcript(&session()), state.transcript);
    assert_eq!(h.usage.records().len(), 1);
    assert!(h.usage.flush_count() >= 1);
}

#[tokio::test]
async fn ordinary_usage_assigns_last_token_total_from_total_tokens() {
    let mut response = text_response("a1", "hello", StopReason::EndTurn);
    response.usage = Some(TokenUsage {
        input_tokens: 9,
        output_tokens: 4,
        total_tokens: 37,
        ..TokenUsage::default()
    });
    let model = ScriptedModel::new(vec![events(response)]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    h.kernel
        .run(&mut state, input("hi"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(state.last_token_total, Some(37));
}

#[tokio::test]
async fn parallel_tool_results_preserve_assistant_source_order() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("slow", "slow"), tool_call("fast", "fast")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![
        tool(
            "slow",
            ToolBehavior::Delayed {
                delay: Duration::from_millis(30),
                result: success("slow result"),
            },
        ),
        tool("fast", ToolBehavior::Immediate(success("fast result"))),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.tool_calls, 2);
    let result_message = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped result");
    assert_eq!(
        result_message.tool_result_ids().collect::<Vec<_>>(),
        ["slow", "fast"]
    );
    let results = result_message
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results, ["slow result", "fast result"]);

    let requests = h.model.requests();
    let second = requests.get(1).expect("second request");
    let model_results = second
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            AgentContentBlock::ToolResult {
                tool_use_id,
                content,
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        model_results,
        [("slow", "slow result"), ("fast", "fast result")]
    );
}

#[tokio::test]
async fn image_bearing_tool_result_keeps_companion_image_after_its_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-image", "screenshot")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let image = AgentContentBlock::Image {
        source: AgentImageSource {
            type_: "base64".into(),
            media_type: "image/png".into(),
            data: "AAAA".into(),
        },
    };
    let result = ToolExecutionResult::success(ToolOutcome {
        content: vec![
            AgentContentBlock::Text {
                text: "preview".into(),
            },
            image.clone(),
        ],
        directives: Vec::new(),
        metadata: None,
    });
    let tools = ScriptedTools::new(vec![tool(
        "screenshot",
        ToolBehavior::Immediate(result.clone()),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    h.kernel
        .run(&mut state, input("capture"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(
        result.outcome.content,
        [result.outcome.content[0].clone(), image.clone()]
    );
    let finished = tool_finished(&h, "call-image");
    assert_eq!(finished.outcome.content, result.outcome.content);

    let grouped = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped image result");
    assert_eq!(
        grouped.content,
        [
            AgentContentBlock::ToolResult {
                tool_use_id: "call-image".into(),
                content: "preview".into(),
            },
            image,
        ]
    );
}

#[tokio::test]
async fn mixed_tool_batch_keeps_each_image_next_to_its_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[
                tool_call("call-image", "screenshot"),
                tool_call("call-text", "lookup"),
            ],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let image = AgentContentBlock::Image {
        source: AgentImageSource {
            type_: "base64".into(),
            media_type: "image/png".into(),
            data: "BBBB".into(),
        },
    };
    let tools = ScriptedTools::new(vec![
        tool(
            "screenshot",
            ToolBehavior::Immediate(ToolExecutionResult::success(ToolOutcome {
                content: vec![
                    AgentContentBlock::Text {
                        text: "preview".into(),
                    },
                    image.clone(),
                ],
                directives: Vec::new(),
                metadata: None,
            })),
        ),
        tool("lookup", ToolBehavior::Immediate(success("lookup result"))),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    h.kernel
        .run(&mut state, input("capture both"), CancelToken::new())
        .await
        .unwrap();

    let expected = vec![
        AgentContentBlock::ToolResult {
            tool_use_id: "call-image".into(),
            content: "preview".into(),
        },
        image,
        AgentContentBlock::ToolResult {
            tool_use_id: "call-text".into(),
            content: "lookup result".into(),
        },
    ];
    let grouped = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped tool result");
    assert_eq!(grouped.content, expected);

    let second_request = &h.model.requests()[1];
    let request_result = second_request
        .messages
        .iter()
        .find(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, AgentContentBlock::ToolResult { .. }))
        })
        .expect("the next model request keeps the grouped result");
    assert_eq!(request_result.content, expected);
}

#[tokio::test]
async fn gate_rewrite_is_re_resolved_before_execution() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "benign")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "credential",
        ToolBehavior::Immediate(success("secret")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::ResolveAs {
        name: "credential".into(),
        label: Some("high risk".into()),
    });
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.tools.executions(), ["credential"]);
    assert_eq!(h.gate.after_calls()[0].name, "credential");
}

#[tokio::test]
async fn gate_rewrite_uses_the_resolved_access_for_wave_scheduling() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "benign"), tool_call("call-2", "read")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![
        ToolDefinition::new(
            "benign",
            ResourceAccess::independent(),
            ToolBehavior::Immediate(success("benign")),
        ),
        ToolDefinition::new(
            "barrier",
            ResourceAccess::barrier(),
            ToolBehavior::Immediate(success("barrier")),
        ),
        tool("read", ToolBehavior::Immediate(success("read"))),
    ]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::ResolveAs {
        name: "barrier".into(),
        label: Some("rewritten".into()),
    });
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.tools.executions(), ["barrier", "read"]);
    assert_eq!(h.gate.after_calls()[0].name, "barrier");
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::ToolFinished {
            call_id,
            permission_label: Some(label),
            ..
        } if call_id == "call-1" && label == "rewritten"
    )));
}

#[tokio::test]
async fn denied_tool_never_executes_but_gets_a_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("should not run")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::Deny("no".into()));
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(h.tools.executions().is_empty());
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|tool_id| tool_id == "call-1")
    }));
}

#[tokio::test]
async fn ordinary_failure_preserves_status_and_operational_error() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool("tool", ToolBehavior::Immediate(failure("boom")))]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    assert_eq!(
        finished.error.as_ref().map(|error| error.message.as_str()),
        Some("boom")
    );
    assert!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin == MessageOrigin::ToolResult)
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                AgentContentBlock::ToolResult { content, .. } if content == "boom"
            ))
    );
}

#[tokio::test]
async fn adapter_error_becomes_a_typed_failed_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Error(tact_agent_api::ToolError::new(
            tact_agent_api::ToolErrorKind::Adapter,
            "adapter exploded",
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    assert_eq!(
        finished.error.as_ref().map(|error| error.kind),
        Some(ToolExecutionErrorKind::Adapter)
    );
}

#[tokio::test]
async fn post_tool_rewrite_cannot_turn_failure_into_success() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Failed,
            ToolDirective::YieldToHost {
                reason: HostYieldReason::NeedsUser,
            },
        )),
    )]);
    let gate = ScriptedGate::new();
    let mut rewritten = success("rewritten while failed");
    rewritten
        .outcome
        .directives
        .push(ToolDirective::CompleteRun);
    gate.push_after(AfterAction::Replace(rewritten));
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    assert_eq!(
        finished.error.as_ref().map(|error| error.message.as_str()),
        Some("failed")
    );
    assert!(finished.outcome.directives.is_empty());
    assert_eq!(h.model.stream_calls(), 2);
}

#[tokio::test]
async fn post_tool_gate_failure_never_restores_success() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("visible success")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_after(AfterAction::Error("post processing failed".into()));
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    let error = finished.error.expect("gate failure");
    assert_eq!(error.kind, ToolExecutionErrorKind::Adapter);
    assert!(error.message.contains("post processing failed"));
}

#[tokio::test]
async fn failed_result_clears_terminal_directives() {
    for directive in [
        ToolDirective::CompleteRun,
        ToolDirective::YieldToHost {
            reason: HostYieldReason::NeedsUser,
        },
        ToolDirective::RequestCompaction {
            focus: Some("failed request".into()),
        },
    ] {
        let model = ScriptedModel::new(vec![
            events(tool_response(
                "a1",
                &[tool_call("call-1", "tool")],
                StopReason::ToolUse,
            )),
            events(text_response("a2", "continued", StopReason::EndTurn)),
        ]);
        let tools = ScriptedTools::new(vec![tool(
            "tool",
            ToolBehavior::Immediate(with_directive(ToolExecutionStatus::Failed, directive)),
        )]);
        let context = ScriptedContext::new();
        let h = harness(model, tools, ScriptedGate::new(), context.clone());
        let mut state = state();

        let outcome = h
            .kernel
            .run(&mut state, input("go"), CancelToken::new())
            .await
            .unwrap();

        assert_eq!(outcome.stop, RunStop::Completed);
        assert_eq!(h.model.stream_calls(), 2);
        assert!(context.compact_requests().is_empty());
    }
}

#[tokio::test]
async fn tool_progress_is_forwarded_in_order() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Progress {
            chunks: vec![
                ToolOutputChunk {
                    stream: ToolOutputStream::Stdout,
                    text: "one".into(),
                },
                ToolOutputChunk {
                    stream: ToolOutputStream::Stderr,
                    text: "two".into(),
                },
            ],
            result: success("done"),
        },
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let progress = h
        .events
        .events()
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::ToolProgress { call_id, chunks } => Some((call_id, chunks)),
            _ => None,
        })
        .expect("progress event");
    assert_eq!(progress.0, "call-1");
    assert_eq!(progress.1[0].text, "one");
    assert_eq!(progress.1[1].text, "two");
}

#[tokio::test]
async fn thinking_lifecycle_is_forwarded_as_deltas() {
    let response = text_response("a1", "answer", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![events_with_deltas(
        response,
        vec![
            MessageDelta::Text {
                delta: r#"{"tool":"not-a-call"}"#.into(),
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Started,
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Delta {
                    delta: "reason".into(),
                },
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Finished,
            },
        ],
    )]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let deltas = h
        .events
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::MessageDelta { message_id, delta } => Some((message_id, delta)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas.len(), 4);
    assert!(
        deltas
            .iter()
            .all(|(message_id, _)| message_id.as_str() == "a1")
    );
    assert!(h.tools.executions().is_empty());
}

#[tokio::test]
async fn cancellation_before_stream_consumes_no_turn() {
    let model = ScriptedModel::new(Vec::new());
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    let cancel = CancelToken::new();
    cancel.cancel();

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(outcome.turns, 0);
    assert_eq!(h.model.stream_calls(), 0);
}

#[tokio::test]
async fn cancellation_during_incomplete_stream_commits_nothing() {
    let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(vec![Ok(
        ModelEvent::MessageStarted {
            message_id: tact_agent_api::MessageId::new("a1"),
            role: AgentRole::Assistant,
        },
    )])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(
        state
            .transcript
            .iter()
            .all(|message| message.role != AgentRole::Assistant)
    );
}

#[tokio::test]
async fn cancellation_after_complete_response_commits_assistant() {
    let response = text_response("a1", "complete", StopReason::EndTurn);
    let events = vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ];
    let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(events)]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(
        state
            .transcript
            .iter()
            .any(|message| message.role == AgentRole::Assistant)
    );
}

#[tokio::test]
async fn cancellation_after_complete_tool_response_pairs_every_call() {
    let response = tool_response(
        "a1",
        &[tool_call("call-1", "tool"), tool_call("call-2", "tool")],
        StopReason::ToolUse,
    );
    let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("must not run")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(h.tools.executions().is_empty());
    let grouped = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped cancellation result");
    assert_eq!(
        grouped.tool_result_ids().collect::<Vec<_>>(),
        ["call-1", "call-2"]
    );
}

#[tokio::test]
async fn cancellation_during_gate_approval_never_executes_the_tool() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("should not run")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::WaitForCancel);
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(h.tools.executions().is_empty());
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-1")
    }));
}

#[tokio::test]
async fn cancellation_waits_for_in_flight_tool_and_pairs_results() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::WaitForCancel(success("stopped")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions(), ["tool"]);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-1")
    }));
}

#[tokio::test]
async fn cancellation_between_waves_suppresses_later_calls() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "slow"), tool_call("call-2", "later")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        ToolDefinition::new(
            "slow",
            ResourceAccess::barrier(),
            ToolBehavior::Delayed {
                delay: Duration::from_millis(40),
                result: success("slow"),
            },
        ),
        ToolDefinition::new(
            "later",
            ResourceAccess::barrier(),
            ToolBehavior::Immediate(success("later")),
        ),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions(), ["slow"]);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-2")
    }));
}

#[tokio::test]
async fn repeated_cancellation_has_one_terminal_event_and_bounded_drain() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "slow")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "slow",
        ToolBehavior::WaitForCancel(success("stopped")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        for _ in 0..32 {
            cancel_task.cancel();
            tokio::task::yield_now().await;
        }
    });

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        h.kernel.run(&mut state, input("go"), cancel),
    )
    .await
    .expect("cancel drain must be bounded")
    .unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions(), ["slow"]);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-1")
    }));

    let events = h.events.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::RunFinished { .. } | AgentEvent::RunFailed { .. }
            ))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::RunFinished { .. })
    ));
    assert!(h.usage.flush_count() >= 1);
}

#[tokio::test]
async fn absolute_turn_cap_is_checked_before_model_dispatch() {
    let model = ScriptedModel::new(Vec::new());
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    state.total_turns = 2;
    let mut input = input("go");
    input.max_total_turns = Some(2);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTurns);
    assert_eq!(outcome.turns, 0);
    assert_eq!(h.model.stream_calls(), 0);
}

#[tokio::test]
async fn transport_retry_counts_as_one_turn() {
    let model = ScriptedModel::new(vec![
        ModelScript::Error(ModelError {
            kind: ModelErrorKind::Transient,
            message: "retry".into(),
            retry_after: Some(Duration::ZERO),
        }),
        events(text_response("a1", "done", StopReason::EndTurn)),
    ]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.total_turns, 1);
    assert_eq!(outcome.recovery.transport_attempts, 1);
    assert_eq!(h.model.stream_calls(), 2);
}

#[tokio::test]
async fn cancellation_during_transport_backoff_keeps_the_retry_count() {
    let model = ScriptedModel::new(vec![ModelScript::Error(ModelError {
        kind: ModelErrorKind::Transient,
        message: "retry".into(),
        retry_after: Some(Duration::from_secs(5)),
    })]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h
        .kernel
        .run(&mut state, input("go"), cancel)
        .await
        .expect("cancellation during backoff returns a typed outcome");

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(outcome.recovery.transport_attempts, 1);
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn context_overflow_compacts_and_retries_as_a_second_dispatch() {
    let model = ScriptedModel::new(vec![
        ModelScript::Error(ModelError::new(ModelErrorKind::ContextOverflow, "overflow")),
        events(text_response("a1", "done", StopReason::EndTurn)),
    ]);
    let context = ScriptedContext::new();
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.turns, 2);
    assert_eq!(outcome.recovery.compact_attempts, 1);
    assert_eq!(
        h.context.compact_requests(),
        [CompactionRequest::Recovery { focus: None }]
    );
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::ContextCompacted {
            kind: tact_agent_api::CompactionKind::Recovery
        }
    )));
}

#[tokio::test]
async fn skipped_recovery_returns_the_original_overflow_error() {
    let model = ScriptedModel::new(vec![ModelScript::Error(ModelError::new(
        ModelErrorKind::ContextOverflow,
        "original overflow",
    ))]);
    let context = ScriptedContext::new();
    context.push_compaction(CompactionOutcome::Skipped {
        reason: "cannot compact".into(),
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context.clone(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("original overflow"));
    assert_eq!(
        context.compact_requests(),
        [CompactionRequest::Recovery { focus: None }]
    );
}

#[tokio::test]
async fn orphaned_tool_use_is_reconciled_before_the_first_request() {
    let mut state = state();
    state.transcript.push(AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        vec![AgentContentBlock::ToolUse {
            id: "orphan".into(),
            name: "tool".into(),
            input: serde_json::json!({}),
        }],
    ));
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "resumed",
        StopReason::EndTurn,
    ))]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );

    let _ = h
        .kernel
        .run(&mut state, input("continue"), CancelToken::new())
        .await
        .unwrap();

    let requests = h.model.requests();
    let first = requests.first().expect("first request");
    assert!(
        first
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| {
                matches!(
                    block,
                    AgentContentBlock::ToolResult { tool_use_id, content }
                        if tool_use_id == "orphan" && content.contains("interrupted")
                )
            })
    );
}

#[tokio::test]
async fn healthy_tool_pairing_is_not_rewritten_during_reconciliation() {
    let store = RecordingTranscriptStore::new();
    let mut state = state();
    state.transcript = vec![
        AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::ToolUse {
                id: "paired".into(),
                name: "tool".into(),
                input: serde_json::json!({}),
            }],
        ),
        AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            vec![AgentContentBlock::ToolResult {
                tool_use_id: "paired".into(),
                content: "already done".into(),
            }],
        ),
    ];
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "resumed",
        StopReason::EndTurn,
    ))]);
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });

    let _ = kernel
        .run(&mut state, input("continue"), CancelToken::new())
        .await
        .unwrap();

    assert!(
        !store
            .calls()
            .iter()
            .any(|call| matches!(call, StoreCall::Replace { .. }))
    );
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "paired")
    }));
}

#[tokio::test]
async fn injected_context_is_persisted_but_is_not_a_user_turn() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_run(RunPreparation::Continue {
        appended: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::GoalContext,
            "goal context",
        )],
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::GoalContext
            && message.text_content().contains("goal context")
    }));
    assert_eq!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin.is_user_turn())
            .count(),
        1
    );
}

#[tokio::test]
async fn injected_context_and_hosted_activity_keep_separate_non_user_channels() {
    let response = text_response("a1", "answer", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::ProviderActivity {
            activity: ProviderActivity {
                id: "search-1".into(),
                step: 0,
                kind: ProviderActivityKind::WebSearch {
                    phase: ProviderActivityPhase::Completed,
                    query: Some("tact kernel".into()),
                    failure: None,
                    duration: Some(Duration::from_millis(5)),
                },
            },
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let context = ScriptedContext::new();
    context.push_run(RunPreparation::Continue {
        appended: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "hook".into(),
            },
            "hook context",
        )],
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(h.tools.executions().is_empty());
    assert_eq!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin.is_user_turn())
            .count(),
        1
    );
    assert!(state.transcript.iter().any(|message| {
        message.origin
            == MessageOrigin::InjectedContext {
                source: "hook".into(),
            }
            && message.text_content() == "hook context"
    }));
    assert!(
        state
            .transcript
            .iter()
            .all(|message| message.origin != MessageOrigin::ToolResult)
    );
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::ProviderActivity { activity }
            if activity.id == "search-1"
                && matches!(
                    &activity.kind,
                    ProviderActivityKind::WebSearch {
                        query: Some(query),
                        ..
                    } if query == "tact kernel"
                )
    )));
    assert!(
        !h.events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolStarted { .. }))
    );
}

#[tokio::test]
async fn usage_record_failure_prevents_tool_execution() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool("tool", ToolBehavior::Immediate(success("done")))]);
    let store = Arc::new(InMemoryTranscriptStore::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model: model.clone(),
        tools: tools.clone(),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage: FailingUsageSink::new(true, false),
        events: events.clone(),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("usage"));
    assert!(tools.executions().is_empty());
    assert!(
        events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFailed { .. }))
    );
    assert!(
        !events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFinished { .. }))
    );
}

#[tokio::test]
async fn assistant_commit_failure_prevents_tool_execution() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("must not run")),
    )]);
    let store = FailingTranscriptStore::new(Some(2), None);
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: tools.clone(),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: events.clone(),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("forced append failure"));
    assert!(tools.executions().is_empty());
    assert_eq!(store.transcript(&session()).len(), 1);
    assert!(
        events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFailed { .. }))
    );
    assert!(
        !events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFinished { .. }))
    );
}

#[tokio::test]
async fn malformed_stream_cardinality_fails_the_run() {
    let response = text_response("a1", "done", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::Finished(response.clone())),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("terminal"));
}

#[tokio::test]
async fn stream_without_a_terminal_response_fails_without_committing() {
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![Ok(
        ModelEvent::MessageStarted {
            message_id: tact_agent_api::MessageId::new("a1"),
            role: AgentRole::Assistant,
        },
    )])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("terminal"));
    assert!(
        state
            .transcript
            .iter()
            .all(|message| message.role != AgentRole::Assistant)
    );
}

#[tokio::test]
async fn terminal_message_id_must_match_started_id() {
    let response = text_response("a2", "done", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: tact_agent_api::MessageId::new("a1"),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("message id"));
}

#[tokio::test]
async fn auto_compaction_records_model_usage_before_retrying_preparation() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    model.push_complete(events(text_response(
        "compact",
        "summary",
        StopReason::EndTurn,
    )));
    let context = ScriptedContext::new();
    context.set_compaction_model_calls(1);
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    let records = h.usage.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.kind == ModelCallKind::LocalCompact)
    );
}

#[tokio::test]
async fn compaction_usage_assigns_last_token_total_from_total_tokens() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let mut compact_response = text_response("compact", "summary", StopReason::EndTurn);
    compact_response.usage = Some(TokenUsage {
        input_tokens: 12,
        output_tokens: 3,
        total_tokens: 91,
        ..TokenUsage::default()
    });
    model.push_complete(events(compact_response));
    let context = ScriptedContext::new();
    context.set_compaction_model_calls(1);
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    h.kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(state.last_token_total, Some(91));
}

#[tokio::test]
async fn compaction_commit_callback_runs_after_the_replacement_is_persisted() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context.clone(),
    );
    let mut state = state();

    h.kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(
        context.compaction_commits(),
        [CompactionRequest::Auto { focus: None }]
    );
}

#[tokio::test]
async fn failed_replacement_never_acknowledges_compaction_commit() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    let store = FailingTranscriptStore::new(None, Some(1));
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: context.clone(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("forced replace failure"));
    assert!(context.compaction_commits().is_empty());
    assert_eq!(store.transcript(&session()).len(), 1);
}

#[tokio::test]
async fn native_compaction_records_native_usage_and_diagnostics() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    model.push_native_compaction(events(text_response(
        "compact",
        "native summary",
        StopReason::EndTurn,
    )));
    let context = ScriptedContext::new();
    context.set_native_compaction_calls(1);
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Command { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Replace { cursor: None },
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let records = h.usage.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.kind == ModelCallKind::NativeCompact)
    );
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::Usage {
            kind: ModelCallKind::NativeCompact,
            ..
        }
    )));
}

#[tokio::test]
async fn every_preparation_compaction_request_uses_the_typed_protocol() {
    for request in [
        CompactionRequest::Auto { focus: None },
        CompactionRequest::Manual {
            focus: Some("manual".into()),
        },
        CompactionRequest::Command {
            focus: Some("command".into()),
        },
        CompactionRequest::Micro,
    ] {
        let model = ScriptedModel::new(vec![events(text_response(
            "a1",
            "done",
            StopReason::EndTurn,
        ))]);
        let context = ScriptedContext::new();
        context.push_request(RequestPreparation::Compact {
            request: request.clone(),
            if_skipped: Vec::new(),
        });
        context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
            transcript: None,
            cursor: CursorUpdate::Unchanged,
        }));
        let kind = request.kind();
        let h = harness(
            model,
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            context.clone(),
        );
        let mut state = state();

        let _ = h
            .kernel
            .run(&mut state, input("go"), CancelToken::new())
            .await
            .unwrap();

        assert_eq!(context.compact_requests(), [request]);
        assert_eq!(context.prepare_request_calls(), 2);
        assert!(h.events.events().iter().any(|event| matches!(
            event,
            AgentEvent::ContextCompacted { kind: observed } if *observed == kind
        )));
    }
}

#[tokio::test]
async fn skipped_compaction_uses_fallback_without_looping() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Micro,
        if_skipped: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "fallback".into(),
            },
            "fallback context",
        )],
    });
    context.push_compaction(CompactionOutcome::Skipped {
        reason: "not needed".into(),
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.context.compact_requests(), [CompactionRequest::Micro]);
    assert!(
        h.model.requests()[0]
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                AgentContentBlock::Text { text } if text == "fallback context"
            ))
    );
}

#[tokio::test]
async fn native_cursor_only_replacement_retains_the_transcript() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let cursor = ModelCursor {
        provider: "fake".into(),
        version: 1,
        payload: serde_json::json!({"compacted": true}),
    };
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Command { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Replace {
            cursor: Some(cursor.clone()),
        },
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(state.cursor, Some(cursor));
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::User && message.text_content() == "go"
    }));
}

#[tokio::test]
async fn replacement_retain_clear_and_unchanged_are_distinguishable() {
    let (retain_calls, retain_state) =
        replacement_case(None, CursorUpdate::Replace { cursor: None }).await;
    let (clear_calls, clear_state) =
        replacement_case(Some(Vec::new()), CursorUpdate::Replace { cursor: None }).await;
    let (unchanged_calls, unchanged_state) = replacement_case(None, CursorUpdate::Unchanged).await;

    assert!(retain_calls.iter().any(|call| matches!(
        call,
        StoreCall::Replace {
            transcript: None,
            cursor: CursorUpdate::Replace { cursor: None },
        }
    )));
    assert!(clear_calls.iter().any(|call| matches!(
        call,
        StoreCall::Replace {
            transcript: Some(messages),
            cursor: CursorUpdate::Replace { cursor: None },
        } if messages.is_empty()
    )));
    assert!(unchanged_calls.iter().any(|call| matches!(
        call,
        StoreCall::Replace {
            transcript: None,
            cursor: CursorUpdate::Unchanged,
        }
    )));
    assert!(
        retain_state
            .transcript
            .iter()
            .any(|message| message.text_content() == "go")
    );
    assert!(
        !clear_state
            .transcript
            .iter()
            .any(|message| message.text_content() == "go")
    );
    assert!(
        unchanged_state
            .transcript
            .iter()
            .any(|message| message.text_content() == "go")
    );
}

#[tokio::test]
async fn max_tokens_with_tools_executes_pairing_then_continues() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::MaxTokens,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("tool result")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.recovery.continuation_attempts, 1);
    assert_eq!(h.tools.executions(), ["tool"]);
    assert!(
        state
            .transcript
            .iter()
            .any(|message| { message.origin == MessageOrigin::Continuation })
    );
    assert_eq!(h.model.stream_calls(), 2);
    let second_request = &h.model.requests()[1];
    let assistant_index = second_request
        .messages
        .iter()
        .position(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, AgentContentBlock::ToolUse { .. }))
        })
        .expect("assistant tool use");
    assert!(
        second_request.messages[assistant_index + 1]
            .content
            .iter()
            .any(|block| matches!(
                block,
                AgentContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call-1"
            ))
    );
}

#[tokio::test]
async fn max_tokens_with_tools_runs_manual_compaction_before_continuation() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "compact")],
            StopReason::MaxTokens,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "compact",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::RequestCompaction {
                focus: Some("manual".into()),
            },
        )),
    )]);
    let context = ScriptedContext::new();
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(model, tools, ScriptedGate::new(), context.clone());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(
        context.compact_requests(),
        [CompactionRequest::Manual {
            focus: Some("manual".into())
        }]
    );
    assert_eq!(h.model.stream_calls(), 2);
    assert!(
        state
            .transcript
            .iter()
            .any(|message| message.origin == MessageOrigin::Continuation)
    );
}

#[tokio::test]
async fn exhausted_max_tokens_synthesizes_truncated_results() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::MaxTokens,
        )),
        events(tool_response(
            "a2",
            &[tool_call("call-2", "tool")],
            StopReason::MaxTokens,
        )),
        events(tool_response(
            "a3",
            &[tool_call("call-3", "tool")],
            StopReason::MaxTokens,
        )),
        events(tool_response(
            "a4",
            &[tool_call("call-4", "tool")],
            StopReason::MaxTokens,
        )),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("tool result")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTokens);
    assert_eq!(outcome.recovery.continuation_attempts, 3);
    assert_eq!(h.tools.executions(), ["tool", "tool", "tool"]);
    assert_eq!(h.model.stream_calls(), 4);
    assert!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin == MessageOrigin::ToolResult)
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                AgentContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } if tool_use_id == "call-4" && content.contains("truncated")
            ))
    );
}

#[tokio::test]
async fn a_truncated_final_turn_cannot_complete_a_goal() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "goal")],
        StopReason::MaxTokens,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "goal",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::CompleteRun,
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let mut input = input("finish");
    input.max_total_turns = Some(1);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTurns);
    assert_eq!(h.tools.executions(), ["goal"]);
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn goal_completion_only_ends_on_the_final_permitted_turn() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "goal")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "goal",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::CompleteRun,
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let mut input = input("finish");
    input.max_total_turns = Some(1);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn non_final_goal_completion_continues_the_activation() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "goal")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continue", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "goal",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::CompleteRun,
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let mut input = input("finish");
    input.max_total_turns = Some(2);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.model.stream_calls(), 2);
}

#[tokio::test]
async fn yield_to_host_suppresses_later_waves() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "yield"), tool_call("call-2", "later")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        ToolDefinition::new(
            "yield",
            ResourceAccess::barrier(),
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::YieldToHost {
                    reason: HostYieldReason::NeedsUser,
                },
            )),
        ),
        tool("later", ToolBehavior::Immediate(success("later"))),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(
        outcome.stop,
        RunStop::YieldToHost {
            reason: HostYieldReason::NeedsUser
        }
    );
    assert_eq!(h.tools.executions(), ["yield"]);
}

#[tokio::test]
async fn terminal_directive_precedence_is_independent_of_completion_order() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[
            tool_call("call-1", "complete"),
            tool_call("call-2", "yield"),
            tool_call("call-3", "compact"),
        ],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        tool(
            "complete",
            ToolBehavior::Delayed {
                delay: Duration::from_millis(30),
                result: with_directive(ToolExecutionStatus::Succeeded, ToolDirective::CompleteRun),
            },
        ),
        tool(
            "yield",
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::YieldToHost {
                    reason: HostYieldReason::NeedsUser,
                },
            )),
        ),
        tool(
            "compact",
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::RequestCompaction {
                    focus: Some("deferred".into()),
                },
            )),
        ),
    ]);
    let context = ScriptedContext::new();
    let h = harness(model, tools, ScriptedGate::new(), context.clone());
    let mut state = state();
    let mut input = input("go");
    input.max_total_turns = Some(1);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.tools.executions().len(), 3);
    assert!(context.compact_requests().is_empty());
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn cancellation_outranks_other_wave_directives() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[
            tool_call("call-1", "cancel"),
            tool_call("call-2", "complete"),
            tool_call("call-3", "yield"),
        ],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        tool(
            "cancel",
            ToolBehavior::CancelAndResult(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::RequestCompaction {
                    focus: Some("cancelled".into()),
                },
            )),
        ),
        tool(
            "complete",
            ToolBehavior::Delayed {
                delay: Duration::from_millis(20),
                result: with_directive(ToolExecutionStatus::Succeeded, ToolDirective::CompleteRun),
            },
        ),
        tool(
            "yield",
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::YieldToHost {
                    reason: HostYieldReason::NeedsUser,
                },
            )),
        ),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions().len(), 3);
}

#[tokio::test]
async fn provider_activity_is_not_treated_as_a_tool_call() {
    let response = text_response("a1", "answer", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::ProviderActivity {
            activity: ProviderActivity {
                id: "search".into(),
                step: 0,
                kind: ProviderActivityKind::WebSearch {
                    phase: ProviderActivityPhase::Completed,
                    query: Some("rust".into()),
                    failure: None,
                    duration: Some(Duration::from_millis(2)),
                },
            },
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(h.tools.executions().is_empty());
}

#[tokio::test]
async fn event_port_runs_before_observers_and_emits_one_terminal_event() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let tools = ScriptedTools::new(Vec::new());
    let gate = ScriptedGate::new();
    let context = ScriptedContext::new();
    let store = Arc::new(InMemoryTranscriptStore::new());
    let usage = Arc::new(RecordingUsageSink::new());
    let events = Arc::new(RecordingEventPort::new());
    let observer = RecordingObserver::new();
    let second_observer = RecordingObserver::new();
    let kernel = AgentKernel::with_extensions(
        KernelDeps {
            model,
            tools,
            gate,
            context,
            transcript: store,
            usage,
            events: events.clone(),
        },
        KernelExtensions {
            observers: vec![observer.clone(), second_observer.clone()],
        },
    );
    let mut state = state();

    let _ = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(events.events(), observer.events());
    assert_eq!(observer.events(), second_observer.events());
    let terminal = observer
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::RunFinished { .. } | AgentEvent::RunFailed { .. }
            )
        })
        .count();
    assert_eq!(terminal, 1);
}

#[tokio::test]
async fn event_port_precedes_observers_in_registration_order() {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let kernel = AgentKernel::with_extensions(
        KernelDeps {
            model: ScriptedModel::new(vec![events(text_response(
                "a1",
                "done",
                StopReason::EndTurn,
            ))]),
            tools: ScriptedTools::new(Vec::new()),
            gate: ScriptedGate::new(),
            context: ScriptedContext::new(),
            transcript: Arc::new(InMemoryTranscriptStore::new()),
            usage: Arc::new(RecordingUsageSink::new()),
            events: Arc::new(TraceEventPort {
                trace: trace.clone(),
            }),
        },
        KernelExtensions {
            observers: vec![
                Arc::new(TraceObserver {
                    name: "first",
                    trace: trace.clone(),
                }),
                Arc::new(TraceObserver {
                    name: "second",
                    trace: trace.clone(),
                }),
            ],
        },
    );
    let mut state = state();

    let _ = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let trace = trace.lock().expect("event trace lock");
    assert!(!trace.is_empty());
    assert!(
        trace
            .as_chunks::<3>()
            .0
            .iter()
            .all(|group| *group == ["port", "first", "second"])
    );
}

#[tokio::test]
async fn uninitialized_state_loads_session_before_applying_new_input() {
    let store = Arc::new(InMemoryTranscriptStore::new());
    let session = session();
    store
        .append(
            &session,
            &AgentMessage::text(AgentRole::User, MessageOrigin::User, "old"),
            CursorUpdate::Unchanged,
        )
        .await
        .unwrap();
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let usage = Arc::new(RecordingUsageSink::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model: model.clone(),
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage,
        events,
    });
    let mut state = AgentState::default();

    let _ = kernel
        .run(&mut state, input("new"), CancelToken::new())
        .await
        .unwrap();

    assert!(state.initialized);
    let requests = model.requests();
    let texts = requests[0]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["old", "new"]);
}

#[tokio::test]
async fn usage_flush_failure_emits_only_run_failed() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let store = Arc::new(InMemoryTranscriptStore::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage: FailingUsageSink::new(false, true),
        events: events.clone(),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("flush"));
    assert!(
        events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFailed { .. }))
    );
    assert!(
        !events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFinished { .. }))
    );
}
