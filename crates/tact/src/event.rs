use std::sync::Arc;

use async_trait::async_trait;
use tact_protocol::RuntimeEvent;
use tokio::sync::broadcast;

use super::{EventService, KernelError};

#[derive(Clone)]
pub struct EventTransport {
    sender: broadcast::Sender<RuntimeEvent>,
    observers: Arc<Vec<Arc<dyn EventObserver>>>,
}

#[async_trait]
pub trait EventObserver: Send + Sync {
    async fn observe(&self, event: &RuntimeEvent) -> Result<(), KernelError>;
}

pub trait RuntimeEventSink: Send + Sync {
    fn emit(&self, event: RuntimeEvent) -> Result<(), KernelError>;
}

pub struct EventSubscription {
    receiver: broadcast::Receiver<RuntimeEvent>,
}

impl EventTransport {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            observers: Arc::new(Vec::new()),
        }
    }

    #[must_use]
    pub fn with_observers(capacity: usize, observers: Vec<Arc<dyn EventObserver>>) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender,
            observers: Arc::new(observers),
        }
    }

    pub fn publish(&self, event: RuntimeEvent) -> Result<usize, String> {
        self.sender.send(event).map_err(|error| error.to_string())
    }

    #[must_use]
    pub fn subscribe(&self) -> EventSubscription {
        EventSubscription {
            receiver: self.sender.subscribe(),
        }
    }
}

impl EventSubscription {
    pub async fn recv(&mut self) -> Result<RuntimeEvent, broadcast::error::RecvError> {
        self.receiver.recv().await
    }

    /// Receives the next event, skipping a lagged window instead of ending.
    ///
    /// A durable writer that briefly falls behind must not stop recording for
    /// the rest of the session. `broadcast` reports a gap as
    /// [`broadcast::error::RecvError::Lagged`] and then keeps delivering the
    /// events still retained, so the lag is logged and the loop continues;
    /// only a closed channel ends the stream with `None`. The skipped events
    /// themselves are already gone from the bus — a reconnect from a
    /// Trajectory sequence is what recovers those — but every subsequent fact
    /// is still persisted.
    pub async fn recv_skipping_lag(&mut self) -> Option<RuntimeEvent> {
        loop {
            match self.receiver.recv().await {
                Ok(event) => return Some(event),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    tracing::warn!(
                        skipped,
                        "runtime event subscriber fell behind; skipped events while catching up"
                    );
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    pub fn try_recv(&mut self) -> Result<RuntimeEvent, broadcast::error::TryRecvError> {
        self.receiver.try_recv()
    }
}

#[async_trait]
impl EventService for EventTransport {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        for observer in self.observers.iter() {
            observer.observe(&event).await?;
        }
        self.sender.send(event).map(|_| ()).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                error.to_string(),
                "event_transport",
                true,
            )
        })
    }
}

impl RuntimeEventSink for EventTransport {
    fn emit(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        self.sender.send(event).map(|_| ()).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                error.to_string(),
                "event_transport",
                true,
            )
        })
    }
}
