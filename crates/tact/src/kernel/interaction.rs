//! Client-neutral request/reply transport for interactive Runtime operations.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use tact_protocol::{
    ErrorCategory, InteractionRequest, InteractionResponse, RequestId, RuntimeEvent,
};
use tokio::{
    sync::{broadcast, oneshot},
    time,
};

use super::{InvocationContext, KernelError};

#[async_trait]
pub trait InteractionService: Send + Sync {
    async fn request(
        &self,
        request: InteractionRequest,
        context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError>;
}

#[derive(Clone)]
pub struct InteractionBroker {
    requests: broadcast::Sender<InteractionRequest>,
    pending: Arc<Mutex<HashMap<RequestId, oneshot::Sender<InteractionResponse>>>>,
}

pub struct InteractionSubscription {
    receiver: broadcast::Receiver<InteractionRequest>,
}

struct PendingRequestGuard {
    broker: InteractionBroker,
    request_id: RequestId,
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        if let Ok(mut pending) = self.broker.pending.lock() {
            pending.remove(&self.request_id);
        }
    }
}

impl InteractionBroker {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (requests, _) = broadcast::channel(capacity.max(1));
        Self {
            requests,
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    #[must_use]
    pub fn subscribe(&self) -> InteractionSubscription {
        InteractionSubscription {
            receiver: self.requests.subscribe(),
        }
    }

    /// Completes the matching outstanding interaction; stale or duplicate
    /// responses cannot affect any other invocation.
    pub fn respond(&self, response: InteractionResponse) -> bool {
        let request_id = response_request_id(&response);
        let sender = self
            .pending
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(request_id));
        sender.is_some_and(|sender| sender.send(response).is_ok())
    }

    #[must_use]
    pub fn pending_count(&self) -> usize {
        self.pending
            .lock()
            .map(|pending| pending.len())
            .unwrap_or(0)
    }
}

impl InteractionSubscription {
    pub async fn recv(&mut self) -> Result<InteractionRequest, broadcast::error::RecvError> {
        self.receiver.recv().await
    }
}

#[async_trait]
impl InteractionService for InteractionBroker {
    async fn request(
        &self,
        request: InteractionRequest,
        context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError> {
        context.ensure_active()?;
        let request_id = request_request_id(&request);
        let (sender, receiver) = oneshot::channel();
        {
            let mut pending = self.pending.lock().map_err(|_| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    "interaction registry lock poisoned",
                    "interaction",
                    true,
                )
            })?;
            if pending.contains_key(&request_id) {
                return Err(KernelError::new(
                    ErrorCategory::InvalidRequest,
                    "interaction request ID is already pending",
                    "interaction",
                    false,
                ));
            }
            pending.insert(request_id.clone(), sender);
        }
        let _guard = PendingRequestGuard {
            broker: self.clone(),
            request_id: request_id.clone(),
        };

        let event = RuntimeEvent::InteractionRequested {
            request: request.clone(),
        };
        context
            .trajectory()
            .append(context.trajectory_id(), context.run_id(), event.clone())
            .await?;
        context.events().publish(event).await?;
        self.requests.send(request).map_err(|error| {
            KernelError::new(
                ErrorCategory::PluginUnavailable,
                format!("no interaction client is subscribed: {error}"),
                "interaction",
                true,
            )
        })?;

        let cancellation = context.cancellation_token();
        let response = match context.deadline() {
            Some(deadline) => tokio::select! {
                response = receiver => response.map_err(|_| unavailable())?,
                _ = cancellation.cancelled() => return Err(KernelError::cancelled()),
                _ = time::sleep_until(deadline) => return Err(KernelError::timeout()),
            },
            None => tokio::select! {
                response = receiver => response.map_err(|_| unavailable())?,
                _ = cancellation.cancelled() => return Err(KernelError::cancelled()),
            },
        };
        Ok(response)
    }
}

fn request_request_id(request: &InteractionRequest) -> &RequestId {
    match request {
        InteractionRequest::Permission { request_id, .. }
        | InteractionRequest::Select { request_id, .. }
        | InteractionRequest::MultiSelect { request_id, .. }
        | InteractionRequest::Confirm { request_id, .. }
        | InteractionRequest::Input { request_id, .. } => request_id,
    }
}

fn response_request_id(response: &InteractionResponse) -> &RequestId {
    match response {
        InteractionResponse::Approved { request_id }
        | InteractionResponse::Rejected { request_id }
        | InteractionResponse::Selected { request_id, .. }
        | InteractionResponse::Text { request_id, .. }
        | InteractionResponse::Cancelled { request_id } => request_id,
    }
}

fn unavailable() -> KernelError {
    KernelError::new(
        ErrorCategory::PluginUnavailable,
        "interaction client disconnected before responding",
        "interaction",
        true,
    )
}
