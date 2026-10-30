use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tact_agent_api::{
    AgentEvent, CancelToken, ModelCallKind, ModelCallOptions, ModelCapabilities, ModelCursor,
    ModelError, ModelEvent, ModelPort, ModelRequest, ModelResponse, ModelStream, RunContext,
    TokenUsage,
};

use crate::deps::EventBus;

#[derive(Clone)]
pub(crate) struct ObservedCall {
    pub(crate) call_id: String,
    pub(crate) run: RunContext,
    pub(crate) kind: ModelCallKind,
    pub(crate) usage: Option<TokenUsage>,
    pub(crate) request_body: Option<Vec<u8>>,
}

struct ObservedState {
    context: RunContext,
    next_call: AtomicU64,
    call_kind: AtomicU8,
    calls: Mutex<Vec<ObservedCall>>,
}

/// Per-activation wrapper that gives every model call one event/usage identity.
#[derive(Clone)]
pub(crate) struct ObservedModelPort {
    inner: Arc<dyn ModelPort>,
    events: EventBus,
    state: Arc<ObservedState>,
}

impl ObservedModelPort {
    pub(crate) fn new(inner: Arc<dyn ModelPort>, events: EventBus, context: RunContext) -> Self {
        Self {
            inner,
            events,
            state: Arc::new(ObservedState {
                context,
                next_call: AtomicU64::new(1),
                call_kind: AtomicU8::new(kind_code(ModelCallKind::Complete)),
                calls: Mutex::new(Vec::new()),
            }),
        }
    }

    pub(crate) fn set_call_kind(&self, kind: ModelCallKind) {
        self.state
            .call_kind
            .store(kind_code(kind), Ordering::SeqCst);
    }

    pub(crate) fn take_latest(&self) -> Option<ObservedCall> {
        self.state
            .calls
            .lock()
            .expect("observed model calls lock")
            .pop()
    }

    pub(crate) fn drain_calls(&self) -> Vec<ObservedCall> {
        std::mem::take(&mut *self.state.calls.lock().expect("observed model calls lock"))
    }

    fn start(&self, kind: ModelCallKind, options: &ModelCallOptions) -> String {
        let id = self.state.next_call.fetch_add(1, Ordering::SeqCst);
        let call_id = format!("{}:model-call-{id}", self.state.context.run_id.as_str());
        let capabilities = self.inner.capabilities();
        self.events.emit(AgentEvent::ModelCallStarted {
            call_id: call_id.clone(),
            kind,
            model: capabilities.model,
            max_output_tokens: capabilities.max_output_tokens,
            options: options.clone(),
        });
        call_id
    }

    fn record(
        &self,
        call_id: String,
        run: RunContext,
        kind: ModelCallKind,
        usage: Option<TokenUsage>,
        request_body: Option<Vec<u8>>,
    ) {
        self.state
            .calls
            .lock()
            .expect("observed model calls lock")
            .push(ObservedCall {
                call_id,
                run,
                kind,
                usage,
                request_body,
            });
    }
}

#[async_trait]
impl ModelPort for ObservedModelPort {
    fn capabilities(&self) -> ModelCapabilities {
        self.inner.capabilities()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let kind = ModelCallKind::Stream;
        let call_id = self.start(kind, &request.options);
        let stream = self
            .inner
            .stream_with_context(&self.state.context, request, cursor, cancel)
            .await?;
        let state = self.state.clone();
        let run = self.state.context.clone();
        let observed = stream.inspect(move |item| {
            if let Ok(ModelEvent::Finished(response)) = item {
                let usage = response.usage;
                let request_body = response.request_body.clone();
                state
                    .calls
                    .lock()
                    .expect("observed model calls lock")
                    .push(ObservedCall {
                        call_id: call_id.clone(),
                        run: run.clone(),
                        kind,
                        usage,
                        request_body,
                    });
            }
        });
        Ok(Box::pin(observed))
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let kind = kind_from_code(self.state.call_kind.load(Ordering::SeqCst));
        let call_id = self.start(kind, &request.options);
        let response = self
            .inner
            .complete_with_context(&self.state.context, request, cursor, cancel)
            .await?;
        self.record(
            call_id,
            self.state.context.clone(),
            kind,
            response.usage,
            response.request_body.clone(),
        );
        Ok(response)
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let kind = ModelCallKind::NativeCompact;
        let call_id = self.start(kind, &request.options);
        let response = self
            .inner
            .compact_with_context(&self.state.context, request, cursor, cancel)
            .await?;
        self.record(
            call_id,
            self.state.context.clone(),
            kind,
            response.usage,
            response.request_body.clone(),
        );
        Ok(response)
    }
}

fn kind_code(kind: ModelCallKind) -> u8 {
    match kind {
        ModelCallKind::Stream => 0,
        ModelCallKind::Complete => 1,
        ModelCallKind::LocalCompact => 2,
        ModelCallKind::NativeCompact => 3,
    }
}

fn kind_from_code(code: u8) -> ModelCallKind {
    match code {
        0 => ModelCallKind::Stream,
        2 => ModelCallKind::LocalCompact,
        3 => ModelCallKind::NativeCompact,
        _ => ModelCallKind::Complete,
    }
}
