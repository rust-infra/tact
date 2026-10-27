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
    ResourceAccess, RunId, RunInput, RunStop, SessionRef, StopReason, ThinkingDelta, TokenUsage,
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
        run_id: RunId::new("test-run"),
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

mod cancellation;
mod continuation_directives;
mod execution;
mod persistence_events;
mod turns_compaction;
