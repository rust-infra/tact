//! Deterministic fake adapters for kernel tests.

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use futures_util::{StreamExt, stream};
use tact_agent_api::test_support::InMemoryTranscriptStore;
use tact_agent_api::{
    AgentContentBlock, AgentEvent, AgentMessage, AgentRole, CancelToken, ContextError,
    ContextPolicy, ContextPreparation, ContextView, CursorUpdate, EventObserver, EventPort,
    GateDecision, MessageId, MessageOrigin, MessageReceipt, ModelCapabilities, ModelCursor,
    ModelError, ModelErrorKind, ModelEvent, ModelMessage, ModelPort, ModelRequest, ModelResponse,
    ModelStream, PreparationDecision, PreparedRequest, ReplaceReceipt, RequestPlan,
    ResolvedToolCall, ResourceAccess, RunInput, RunStop, SessionRef, StopReason, StoreError,
    ToolCall, ToolDescriptor, ToolError, ToolExecutionContext, ToolExecutionResult, ToolGate,
    ToolGateContext, ToolGateError, ToolPort, ToolResolver, ToolSnapshot, ToolSpec,
    TranscriptSnapshot, TranscriptStore,
};

#[derive(Clone)]
pub enum ModelScript {
    Events(Vec<Result<ModelEvent, ModelError>>),
    Error(ModelError),
    WaitForCancel(Vec<Result<ModelEvent, ModelError>>),
}

pub struct ScriptedModel {
    capabilities: ModelCapabilities,
    streams: Mutex<VecDeque<ModelScript>>,
    completes: Mutex<VecDeque<ModelScript>>,
    native_compactions: Mutex<VecDeque<ModelScript>>,
    requests: Mutex<Vec<ModelRequest>>,
    stream_calls: Mutex<u32>,
}

impl ScriptedModel {
    pub fn new(scripts: Vec<ModelScript>) -> Arc<Self> {
        Self::with_capabilities(default_capabilities(), scripts)
    }

    pub fn with_capabilities(
        capabilities: ModelCapabilities,
        scripts: Vec<ModelScript>,
    ) -> Arc<Self> {
        Arc::new(Self {
            capabilities,
            streams: Mutex::new(scripts.into()),
            completes: Mutex::new(VecDeque::new()),
            native_compactions: Mutex::new(VecDeque::new()),
            requests: Mutex::new(Vec::new()),
            stream_calls: Mutex::new(0),
        })
    }

    pub fn push_stream(&self, script: ModelScript) {
        self.streams
            .lock()
            .expect("stream scripts lock")
            .push_back(script);
    }

    pub fn push_complete(&self, script: ModelScript) {
        self.completes
            .lock()
            .expect("complete scripts lock")
            .push_back(script);
    }

    pub fn push_native_compaction(&self, script: ModelScript) {
        self.native_compactions
            .lock()
            .expect("native compaction scripts lock")
            .push_back(script);
    }

    pub fn requests(&self) -> Vec<ModelRequest> {
        self.requests.lock().expect("model requests lock").clone()
    }

    pub fn stream_calls(&self) -> u32 {
        *self.stream_calls.lock().expect("stream calls lock")
    }
}

#[async_trait]
impl ModelPort for ScriptedModel {
    fn capabilities(&self) -> ModelCapabilities {
        self.capabilities.clone()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        self.requests
            .lock()
            .expect("model requests lock")
            .push(request);
        *self.stream_calls.lock().expect("stream calls lock") += 1;
        let script = self
            .streams
            .lock()
            .expect("stream scripts lock")
            .pop_front()
            .ok_or_else(|| ModelError::new(ModelErrorKind::Permanent, "no stream script"))?;
        model_stream(script, cancel)
    }

    async fn complete(
        &self,
        request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.requests
            .lock()
            .expect("model requests lock")
            .push(request);
        let script = self
            .completes
            .lock()
            .expect("complete scripts lock")
            .pop_front()
            .ok_or_else(|| ModelError::new(ModelErrorKind::Permanent, "no complete script"))?;
        response_from_script(script, cancel).await
    }

    async fn compact(
        &self,
        request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.requests
            .lock()
            .expect("model requests lock")
            .push(request);
        let script = self
            .native_compactions
            .lock()
            .expect("native compaction scripts lock")
            .pop_front()
            .ok_or_else(|| ModelError::new(ModelErrorKind::Permanent, "no native script"))?;
        response_from_script(script, cancel).await
    }
}

fn model_stream(script: ModelScript, cancel: CancelToken) -> Result<ModelStream, ModelError> {
    match script {
        ModelScript::Events(events) => Ok(Box::pin(stream::iter(events))),
        ModelScript::Error(error) => Err(error),
        ModelScript::WaitForCancel(events) => {
            let wait = async move {
                cancel.cancelled().await;
            };
            Ok(Box::pin(
                stream::once(wait).flat_map(move |_| stream::iter(events.clone())),
            ))
        }
    }
}

async fn response_from_script(
    script: ModelScript,
    cancel: CancelToken,
) -> Result<ModelResponse, ModelError> {
    use futures_util::StreamExt;

    let mut stream = model_stream(script, cancel)?;
    let mut response = None;
    while let Some(item) = stream.next().await {
        if let ModelEvent::Finished(value) = item? {
            response = Some(value);
        }
    }
    response.ok_or_else(|| ModelError::new(ModelErrorKind::Permanent, "script had no response"))
}

pub fn default_capabilities() -> ModelCapabilities {
    ModelCapabilities {
        model: "fake".into(),
        context_window: Some(100_000),
        max_output_tokens: 4096,
        supports_streaming: true,
        supports_native_compaction: false,
    }
}

pub fn text_response(id: &str, text: &str, stop_reason: StopReason) -> ModelResponse {
    ModelResponse {
        message_id: MessageId::new(id),
        message: AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, text),
        stop_reason,
        usage: None,
        request_body: None,
        cursor: CursorUpdate::Unchanged,
    }
}

pub fn tool_response(
    id: &str,
    calls: &[(&str, &str, serde_json::Value)],
    stop_reason: StopReason,
) -> ModelResponse {
    ModelResponse {
        message_id: MessageId::new(id),
        message: AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            calls
                .iter()
                .map(|(call_id, name, input)| AgentContentBlock::ToolUse {
                    id: (*call_id).into(),
                    name: (*name).into(),
                    input: input.clone(),
                })
                .collect(),
        ),
        stop_reason,
        usage: None,
        request_body: None,
        cursor: CursorUpdate::Unchanged,
    }
}

pub fn events(response: ModelResponse) -> ModelScript {
    ModelScript::Events(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ])
}

pub fn events_with_deltas(
    response: ModelResponse,
    deltas: Vec<tact_agent_api::MessageDelta>,
) -> ModelScript {
    let mut items = vec![Ok(ModelEvent::MessageStarted {
        message_id: response.message_id.clone(),
        role: AgentRole::Assistant,
    })];
    items.extend(deltas.into_iter().map(|delta| {
        Ok(ModelEvent::MessageDelta {
            message_id: response.message_id.clone(),
            delta,
        })
    }));
    items.push(Ok(ModelEvent::Finished(response)));
    ModelScript::Events(items)
}

#[derive(Clone)]
pub enum RunPreparation {
    Continue {
        appended: Vec<AgentMessage>,
    },
    Compact {
        request: tact_agent_api::CompactionRequest,
        if_skipped: Vec<AgentMessage>,
    },
    Block(RunStop),
}

#[derive(Clone)]
pub enum RequestPreparation {
    Continue {
        appended: Vec<AgentMessage>,
    },
    Compact {
        request: tact_agent_api::CompactionRequest,
        if_skipped: Vec<AgentMessage>,
    },
    Block(RunStop),
}

pub struct ScriptedContext {
    run: Mutex<VecDeque<RunPreparation>>,
    request: Mutex<VecDeque<RequestPreparation>>,
    compact: Mutex<VecDeque<tact_agent_api::CompactionOutcome>>,
    compact_model_calls: Mutex<u32>,
    native_compaction_calls: Mutex<u32>,
    compact_requests: Mutex<Vec<tact_agent_api::CompactionRequest>>,
    compaction_commits: Mutex<Vec<tact_agent_api::CompactionRequest>>,
    prepare_run_calls: Mutex<u32>,
    prepare_request_calls: Mutex<u32>,
}

impl Default for ScriptedContext {
    fn default() -> Self {
        Self {
            run: Mutex::new(VecDeque::new()),
            request: Mutex::new(VecDeque::new()),
            compact: Mutex::new(VecDeque::new()),
            compact_model_calls: Mutex::new(0),
            native_compaction_calls: Mutex::new(0),
            compact_requests: Mutex::new(Vec::new()),
            compaction_commits: Mutex::new(Vec::new()),
            prepare_run_calls: Mutex::new(0),
            prepare_request_calls: Mutex::new(0),
        }
    }
}

impl ScriptedContext {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn push_run(&self, preparation: RunPreparation) {
        self.run
            .lock()
            .expect("run scripts lock")
            .push_back(preparation);
    }

    pub fn push_request(&self, preparation: RequestPreparation) {
        self.request
            .lock()
            .expect("request scripts lock")
            .push_back(preparation);
    }

    pub fn push_compaction(&self, outcome: tact_agent_api::CompactionOutcome) {
        self.compact
            .lock()
            .expect("compaction scripts lock")
            .push_back(outcome);
    }

    pub fn set_compaction_model_calls(&self, calls: u32) {
        *self
            .compact_model_calls
            .lock()
            .expect("compaction model calls lock") = calls;
    }

    pub fn set_native_compaction_calls(&self, calls: u32) {
        *self
            .native_compaction_calls
            .lock()
            .expect("native compaction calls lock") = calls;
    }

    pub fn compact_requests(&self) -> Vec<tact_agent_api::CompactionRequest> {
        self.compact_requests
            .lock()
            .expect("compaction requests lock")
            .clone()
    }

    pub fn compaction_commits(&self) -> Vec<tact_agent_api::CompactionRequest> {
        self.compaction_commits
            .lock()
            .expect("compaction commits lock")
            .clone()
    }

    pub fn prepare_run_calls(&self) -> u32 {
        *self
            .prepare_run_calls
            .lock()
            .expect("prepare run calls lock")
    }

    pub fn prepare_request_calls(&self) -> u32 {
        *self
            .prepare_request_calls
            .lock()
            .expect("prepare request calls lock")
    }

    fn request_plan(
        state: &ContextView<'_>,
        appended: Vec<AgentMessage>,
        options: tact_agent_api::ModelCallOptions,
    ) -> RequestPlan {
        let mut messages = state
            .transcript
            .iter()
            .map(ModelMessage::from)
            .collect::<Vec<_>>();
        messages.extend(state.pending_inputs.iter().map(ModelMessage::from));
        messages.extend(appended.iter().map(ModelMessage::from));
        RequestPlan {
            request: PreparedRequest {
                system: "fake system".into(),
                messages,
                cursor: state.cursor.cloned(),
                options,
            },
            appended_messages: appended,
        }
    }
}

#[async_trait]
impl ContextPolicy for ScriptedContext {
    async fn prepare_run(
        &self,
        input: &RunInput,
        _state: &ContextView<'_>,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError> {
        *self
            .prepare_run_calls
            .lock()
            .expect("prepare run calls lock") += 1;
        let preparation = self.run.lock().expect("run scripts lock").pop_front();
        Ok(match preparation {
            Some(RunPreparation::Block(stop)) => PreparationDecision::Block(stop),
            Some(RunPreparation::Compact {
                request,
                if_skipped,
            }) => PreparationDecision::Compact {
                request,
                if_skipped: ContextPreparation {
                    inputs: input.inputs.clone(),
                    appended_messages: if_skipped,
                },
            },
            Some(RunPreparation::Continue { appended }) => {
                PreparationDecision::Continue(ContextPreparation {
                    inputs: input.inputs.clone(),
                    appended_messages: appended,
                })
            }
            None => PreparationDecision::Continue(ContextPreparation {
                inputs: input.inputs.clone(),
                appended_messages: Vec::new(),
            }),
        })
    }

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError> {
        *self
            .prepare_request_calls
            .lock()
            .expect("prepare request calls lock") += 1;
        let preparation = self
            .request
            .lock()
            .expect("request scripts lock")
            .pop_front();
        Ok(match preparation {
            Some(RequestPreparation::Block(stop)) => PreparationDecision::Block(stop),
            Some(RequestPreparation::Compact {
                request,
                if_skipped,
            }) => PreparationDecision::Compact {
                request,
                if_skipped: Self::request_plan(
                    state,
                    if_skipped,
                    tact_agent_api::ModelCallOptions::default(),
                ),
            },
            Some(RequestPreparation::Continue { appended }) => PreparationDecision::Continue(
                Self::request_plan(state, appended, tact_agent_api::ModelCallOptions::default()),
            ),
            None => PreparationDecision::Continue(Self::request_plan(
                state,
                Vec::new(),
                tact_agent_api::ModelCallOptions::default(),
            )),
        })
    }

    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: tact_agent_api::CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<tact_agent_api::CompactionOutcome, ContextError> {
        self.compact_requests
            .lock()
            .expect("compaction requests lock")
            .push(request);
        let calls = {
            let mut calls = self
                .compact_model_calls
                .lock()
                .expect("compaction model calls lock");
            let value = *calls;
            *calls = 0;
            value
        };
        for _ in 0..calls {
            let messages = state.transcript.iter().map(ModelMessage::from).collect();
            let capabilities = model.capabilities();
            let _ = model
                .complete(
                    ModelRequest {
                        model: capabilities.model,
                        system: "compact".into(),
                        messages,
                        tools: Vec::new(),
                        max_output_tokens: capabilities.max_output_tokens,
                        options: tact_agent_api::ModelCallOptions {
                            extra: serde_json::Map::from_iter([(
                                "compaction".into(),
                                serde_json::Value::Bool(true),
                            )]),
                            ..Default::default()
                        },
                    },
                    state.cursor,
                    cancel.clone(),
                )
                .await
                .map_err(|error| ContextError::Compaction(error.to_string()))?;
        }
        let native_calls = {
            let mut calls = self
                .native_compaction_calls
                .lock()
                .expect("native compaction calls lock");
            let value = *calls;
            *calls = 0;
            value
        };
        for _ in 0..native_calls {
            let messages = state.transcript.iter().map(ModelMessage::from).collect();
            let capabilities = model.capabilities();
            let _ = model
                .compact(
                    ModelRequest {
                        model: capabilities.model,
                        system: "native compact".into(),
                        messages,
                        tools: Vec::new(),
                        max_output_tokens: capabilities.max_output_tokens,
                        options: tact_agent_api::ModelCallOptions {
                            extra: serde_json::Map::from_iter([(
                                "native_compaction".into(),
                                serde_json::Value::Bool(true),
                            )]),
                            ..Default::default()
                        },
                    },
                    state.cursor,
                    cancel.clone(),
                )
                .await
                .map_err(|error| ContextError::Compaction(error.to_string()))?;
        }
        self.compact
            .lock()
            .expect("compaction scripts lock")
            .pop_front()
            .ok_or_else(|| ContextError::Compaction("no compaction script".into()))
    }

    async fn compaction_committed(
        &self,
        _state: &ContextView<'_>,
        request: &tact_agent_api::CompactionRequest,
    ) -> Result<(), ContextError> {
        self.compaction_commits
            .lock()
            .expect("compaction commits lock")
            .push(request.clone());
        Ok(())
    }
}

#[derive(Clone)]
pub enum ToolBehavior {
    Immediate(ToolExecutionResult),
    Delayed {
        delay: Duration,
        result: ToolExecutionResult,
    },
    WaitForCancel(ToolExecutionResult),
    CancelAndResult(ToolExecutionResult),
    Progress {
        chunks: Vec<tact_agent_api::ToolOutputChunk>,
        result: ToolExecutionResult,
    },
    Error(ToolError),
}

#[derive(Clone)]
pub struct ToolDefinition {
    pub descriptor: ToolDescriptor,
    pub access: ResourceAccess,
    pub behavior: ToolBehavior,
}

impl ToolDefinition {
    pub fn new(name: &str, access: ResourceAccess, behavior: ToolBehavior) -> Self {
        Self {
            descriptor: ToolDescriptor {
                name: name.into(),
                description: format!("{name} tool"),
                metadata: serde_json::Value::Null,
            },
            access,
            behavior,
        }
    }
}

pub struct ScriptedTools {
    definitions: Mutex<HashMap<String, ToolDefinition>>,
    executions: Mutex<Vec<String>>,
}

impl ScriptedTools {
    pub fn new(definitions: Vec<ToolDefinition>) -> Arc<Self> {
        Arc::new(Self {
            definitions: Mutex::new(
                definitions
                    .into_iter()
                    .map(|definition| (definition.descriptor.name.clone(), definition))
                    .collect(),
            ),
            executions: Mutex::new(Vec::new()),
        })
    }

    pub fn executions(&self) -> Vec<String> {
        self.executions.lock().expect("executions lock").clone()
    }

    pub fn insert(&self, definition: ToolDefinition) {
        self.definitions
            .lock()
            .expect("definitions lock")
            .insert(definition.descriptor.name.clone(), definition);
    }
}

#[async_trait]
impl ToolPort for ScriptedTools {
    async fn refresh(&self, _cancel: CancelToken) -> Result<ToolSnapshot, ToolError> {
        let definitions = self.definitions.lock().expect("definitions lock");
        let mut specs = definitions
            .values()
            .map(|definition| ToolSpec {
                name: definition.descriptor.name.clone(),
                description: definition.descriptor.description.clone(),
                input_schema: serde_json::json!({"type": "object"}),
            })
            .collect::<Vec<_>>();
        specs.sort_by(|left, right| left.name.cmp(&right.name));
        Ok(ToolSnapshot {
            specs,
            generation: 1,
        })
    }

    async fn execute(
        &self,
        call: ResolvedToolCall,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        self.executions
            .lock()
            .expect("executions lock")
            .push(call.name.clone());
        let behavior = self
            .definitions
            .lock()
            .expect("definitions lock")
            .get(&call.name)
            .map(|definition| definition.behavior.clone())
            .ok_or_else(|| {
                ToolError::new(
                    tact_agent_api::ToolErrorKind::UnknownTool,
                    format!("unknown tool {}", call.name),
                )
            })?;
        match behavior {
            ToolBehavior::Immediate(result) => Ok(result),
            ToolBehavior::Delayed { delay, result } => {
                tokio::time::sleep(delay).await;
                Ok(result)
            }
            ToolBehavior::WaitForCancel(result) => {
                context.cancel.cancelled().await;
                Ok(result)
            }
            ToolBehavior::CancelAndResult(result) => {
                context.cancel.cancel();
                Ok(result)
            }
            ToolBehavior::Progress { chunks, result } => {
                context.progress.emit(chunks);
                Ok(result)
            }
            ToolBehavior::Error(error) => Err(error),
        }
    }
}

impl ToolResolver for ScriptedTools {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
        let definitions = self.definitions.lock().expect("definitions lock");
        let definition = definitions.get(&call.name).ok_or_else(|| {
            ToolError::new(
                tact_agent_api::ToolErrorKind::UnknownTool,
                format!("unknown tool {}", call.name),
            )
        })?;
        Ok(ResolvedToolCall {
            call_id: call.id.clone(),
            name: call.name.clone(),
            input: call.input.clone(),
            access: definition.access.clone(),
            descriptor: definition.descriptor.clone(),
        })
    }
}

#[derive(Clone)]
pub enum BeforeAction {
    ResolveCurrent { label: Option<String> },
    ResolveAs { name: String, label: Option<String> },
    Deny(String),
    Cancel,
    WaitForCancel,
    Error(String),
}

#[derive(Clone)]
pub enum AfterAction {
    Identity,
    Replace(ToolExecutionResult),
    Error(String),
}

pub struct ScriptedGate {
    before: Mutex<VecDeque<BeforeAction>>,
    after: Mutex<VecDeque<AfterAction>>,
    before_calls: Mutex<Vec<ToolCall>>,
    after_calls: Mutex<Vec<ResolvedToolCall>>,
}

impl Default for ScriptedGate {
    fn default() -> Self {
        Self {
            before: Mutex::new(VecDeque::new()),
            after: Mutex::new(VecDeque::new()),
            before_calls: Mutex::new(Vec::new()),
            after_calls: Mutex::new(Vec::new()),
        }
    }
}

impl ScriptedGate {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn push_before(&self, action: BeforeAction) {
        self.before
            .lock()
            .expect("before scripts lock")
            .push_back(action);
    }

    pub fn push_after(&self, action: AfterAction) {
        self.after
            .lock()
            .expect("after scripts lock")
            .push_back(action);
    }

    pub fn before_calls(&self) -> Vec<ToolCall> {
        self.before_calls.lock().expect("before calls lock").clone()
    }

    pub fn after_calls(&self) -> Vec<ResolvedToolCall> {
        self.after_calls.lock().expect("after calls lock").clone()
    }
}

#[async_trait]
impl ToolGate for ScriptedGate {
    async fn before(
        &self,
        mut call: ToolCall,
        resolver: &dyn ToolResolver,
        _context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError> {
        self.before_calls
            .lock()
            .expect("before calls lock")
            .push(call.clone());
        let action = self
            .before
            .lock()
            .expect("before scripts lock")
            .pop_front()
            .unwrap_or(BeforeAction::ResolveCurrent { label: None });
        match action {
            BeforeAction::ResolveCurrent { label } => {
                let resolved = resolver
                    .resolve(&call)
                    .map_err(|error| ToolGateError::Failed(error.to_string()))?;
                Ok(GateDecision::Allow {
                    call: Box::new(resolved),
                    label,
                })
            }
            BeforeAction::ResolveAs { name, label } => {
                call.name = name;
                let resolved = resolver
                    .resolve(&call)
                    .map_err(|error| ToolGateError::Failed(error.to_string()))?;
                Ok(GateDecision::Allow {
                    call: Box::new(resolved),
                    label,
                })
            }
            BeforeAction::Deny(reason) => Ok(GateDecision::Deny { reason }),
            BeforeAction::Cancel => Err(ToolGateError::Cancelled),
            BeforeAction::WaitForCancel => {
                cancel.cancelled().await;
                Err(ToolGateError::Cancelled)
            }
            BeforeAction::Error(message) => Err(ToolGateError::Failed(message)),
        }
    }

    async fn after(
        &self,
        call: &ResolvedToolCall,
        result: ToolExecutionResult,
        _context: &ToolGateContext,
        _cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError> {
        self.after_calls
            .lock()
            .expect("after calls lock")
            .push(call.clone());
        match self
            .after
            .lock()
            .expect("after scripts lock")
            .pop_front()
            .unwrap_or(AfterAction::Identity)
        {
            AfterAction::Identity => Ok(result),
            AfterAction::Replace(result) => Ok(result),
            AfterAction::Error(message) => Err(ToolGateError::Failed(message)),
        }
    }
}

#[derive(Default)]
pub struct RecordingEventPort {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingEventPort {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("events lock").clone()
    }
}

impl EventPort for RecordingEventPort {
    fn emit(&self, event: AgentEvent) {
        self.events.lock().expect("events lock").push(event);
    }
}

#[derive(Default)]
pub struct RecordingObserver {
    events: Mutex<Vec<AgentEvent>>,
}

impl RecordingObserver {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().expect("observer events lock").clone()
    }
}

impl EventObserver for RecordingObserver {
    fn observe(&self, event: &AgentEvent) {
        self.events
            .lock()
            .expect("observer events lock")
            .push(event.clone());
    }
}

pub struct FailingUsageSink {
    fail_record: bool,
    fail_flush: bool,
}

/// In-memory store that fails a selected append/replace call.
pub struct FailingTranscriptStore {
    inner: Arc<InMemoryTranscriptStore>,
    fail_append_on: Option<usize>,
    fail_replace_on: Option<usize>,
    appends: Mutex<usize>,
    replaces: Mutex<usize>,
}

impl FailingTranscriptStore {
    pub fn new(fail_append_on: Option<usize>, fail_replace_on: Option<usize>) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(InMemoryTranscriptStore::new()),
            fail_append_on,
            fail_replace_on,
            appends: Mutex::new(0),
            replaces: Mutex::new(0),
        })
    }

    pub fn transcript(&self, session: &SessionRef) -> Vec<AgentMessage> {
        self.inner.transcript(session)
    }
}

#[async_trait]
impl TranscriptStore for FailingTranscriptStore {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError> {
        self.inner.load(session).await
    }

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError> {
        let fail = {
            let mut appends = self.appends.lock().expect("append count lock");
            *appends += 1;
            self.fail_append_on == Some(*appends)
        };
        if fail {
            return Err(StoreError::Append("forced append failure".into()));
        }
        self.inner.append(session, message, cursor).await
    }

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        let fail = {
            let mut replaces = self.replaces.lock().expect("replace count lock");
            *replaces += 1;
            self.fail_replace_on == Some(*replaces)
        };
        if fail {
            return Err(StoreError::Replace("forced replace failure".into()));
        }
        self.inner.replace(session, transcript, cursor).await
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreCall {
    Load,
    Append {
        message: AgentMessage,
        cursor: CursorUpdate,
    },
    Replace {
        transcript: Option<Vec<AgentMessage>>,
        cursor: CursorUpdate,
    },
}

/// In-memory store that records the exact persistence operations.
pub struct RecordingTranscriptStore {
    inner: Arc<InMemoryTranscriptStore>,
    calls: Mutex<Vec<StoreCall>>,
}

impl RecordingTranscriptStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(InMemoryTranscriptStore::new()),
            calls: Mutex::new(Vec::new()),
        })
    }

    pub fn calls(&self) -> Vec<StoreCall> {
        self.calls.lock().expect("store calls lock").clone()
    }

    pub fn transcript(&self, session: &SessionRef) -> Vec<AgentMessage> {
        self.inner.transcript(session)
    }
}

#[async_trait]
impl TranscriptStore for RecordingTranscriptStore {
    async fn load(&self, session: &SessionRef) -> Result<TranscriptSnapshot, StoreError> {
        self.calls
            .lock()
            .expect("store calls lock")
            .push(StoreCall::Load);
        self.inner.load(session).await
    }

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<MessageReceipt, StoreError> {
        self.calls
            .lock()
            .expect("store calls lock")
            .push(StoreCall::Append {
                message: message.clone(),
                cursor: cursor.clone(),
            });
        self.inner.append(session, message, cursor).await
    }

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<ReplaceReceipt, StoreError> {
        self.calls
            .lock()
            .expect("store calls lock")
            .push(StoreCall::Replace {
                transcript: transcript.map(ToOwned::to_owned),
                cursor: cursor.clone(),
            });
        self.inner.replace(session, transcript, cursor).await
    }
}

impl FailingUsageSink {
    pub fn new(fail_record: bool, fail_flush: bool) -> Arc<Self> {
        Arc::new(Self {
            fail_record,
            fail_flush,
        })
    }
}

#[async_trait]
impl tact_agent_api::UsageSink for FailingUsageSink {
    async fn record(
        &self,
        _record: tact_agent_api::UsageRecord,
    ) -> Result<(), tact_agent_api::UsageError> {
        if self.fail_record {
            Err(tact_agent_api::UsageError::Record("forced".into()))
        } else {
            Ok(())
        }
    }

    async fn flush(&self) -> Result<(), tact_agent_api::UsageError> {
        if self.fail_flush {
            Err(tact_agent_api::UsageError::Flush("forced".into()))
        } else {
            Ok(())
        }
    }
}

pub struct FailingStore {
    inner: Arc<tact_agent_api::test_support::InMemoryTranscriptStore>,
    fail_append: bool,
    fail_replace: bool,
}

impl FailingStore {
    pub fn new(fail_append: bool, fail_replace: bool) -> Arc<Self> {
        Arc::new(Self {
            inner: Arc::new(tact_agent_api::test_support::InMemoryTranscriptStore::new()),
            fail_append,
            fail_replace,
        })
    }

    pub fn inner(&self) -> Arc<tact_agent_api::test_support::InMemoryTranscriptStore> {
        self.inner.clone()
    }
}

#[async_trait]
impl tact_agent_api::TranscriptStore for FailingStore {
    async fn load(
        &self,
        session: &SessionRef,
    ) -> Result<tact_agent_api::TranscriptSnapshot, tact_agent_api::StoreError> {
        self.inner.load(session).await
    }

    async fn append(
        &self,
        session: &SessionRef,
        message: &AgentMessage,
        cursor: CursorUpdate,
    ) -> Result<tact_agent_api::MessageReceipt, tact_agent_api::StoreError> {
        if self.fail_append {
            return Err(tact_agent_api::StoreError::Append("forced".into()));
        }
        self.inner.append(session, message, cursor).await
    }

    async fn replace(
        &self,
        session: &SessionRef,
        transcript: Option<&[AgentMessage]>,
        cursor: CursorUpdate,
    ) -> Result<tact_agent_api::ReplaceReceipt, tact_agent_api::StoreError> {
        if self.fail_replace {
            return Err(tact_agent_api::StoreError::Replace("forced".into()));
        }
        self.inner.replace(session, transcript, cursor).await
    }
}
