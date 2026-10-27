use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU8, AtomicU64, Ordering},
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tact_agent_api::{
    AgentEvent, CancelToken, ModelCallKind, ModelCapabilities, ModelCursor, ModelError, ModelEvent,
    ModelPort, ModelRequest, ModelResponse, ModelStream,
};

use crate::deps::EventBus;

#[derive(Clone)]
pub(crate) struct ObservedCall {
    pub(crate) call_id: String,
    pub(crate) kind: ModelCallKind,
    pub(crate) response: ModelResponse,
}

struct ObservedState {
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
    pub(crate) fn new(inner: Arc<dyn ModelPort>, events: EventBus) -> Self {
        Self {
            inner,
            events,
            state: Arc::new(ObservedState {
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

    fn start(&self, kind: ModelCallKind, metadata: &serde_json::Value) -> String {
        let id = self.state.next_call.fetch_add(1, Ordering::SeqCst);
        let call_id = format!("model-call-{id}");
        let capabilities = self.inner.capabilities();
        self.events.emit(AgentEvent::ModelCallStarted {
            call_id: call_id.clone(),
            kind,
            model: capabilities.model,
            max_output_tokens: capabilities.max_output_tokens,
            metadata: metadata.clone(),
        });
        call_id
    }

    fn record(&self, call_id: String, kind: ModelCallKind, response: ModelResponse) {
        self.state
            .calls
            .lock()
            .expect("observed model calls lock")
            .push(ObservedCall {
                call_id,
                kind,
                response,
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
        let call_id = self.start(kind, &request.metadata);
        let stream = self.inner.stream(request, cursor, cancel).await?;
        let state = self.state.clone();
        let observed = stream.inspect(move |item| {
            if let Ok(ModelEvent::Finished(response)) = item {
                state
                    .calls
                    .lock()
                    .expect("observed model calls lock")
                    .push(ObservedCall {
                        call_id: call_id.clone(),
                        kind,
                        response: response.clone(),
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
        let call_id = self.start(kind, &request.metadata);
        let response = self.inner.complete(request, cursor, cancel).await?;
        self.record(call_id, kind, response.clone());
        Ok(response)
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let kind = ModelCallKind::NativeCompact;
        let call_id = self.start(kind, &request.metadata);
        let response = self.inner.compact(request, cursor, cancel).await?;
        self.record(call_id, kind, response.clone());
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
