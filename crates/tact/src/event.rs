use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tact_protocol::{ErrorCategory, RuntimeEvent, TrajectoryEvent, TrajectoryId};
use tokio::sync::broadcast;

use super::KernelError;
use crate::trajectory::TrajectoryService;

#[derive(Clone)]
pub struct EventTransport {
    /// `None` once the transport is closed. Dropping the last sender is what
    /// ends the live channel, so an existing subscription observes `Closed`
    /// instead of waiting for an event that will never arrive.
    sender: Arc<RwLock<Option<broadcast::Sender<RuntimeEvent>>>>,
    observers: Arc<Vec<Arc<dyn EventObserver>>>,
    /// Durable facts behind [`EventTransport::replay_from`]. The broadcast
    /// channel only carries live delivery, so replaying a sequence needs the
    /// recorder; a host that installs none gets an explicit failure.
    replay_source: Option<Arc<dyn TrajectoryService>>,
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
            sender: Arc::new(RwLock::new(Some(sender))),
            observers: Arc::new(Vec::new()),
            replay_source: None,
        }
    }

    #[must_use]
    pub fn with_observers(capacity: usize, observers: Vec<Arc<dyn EventObserver>>) -> Self {
        let (sender, _) = broadcast::channel(capacity.max(1));
        Self {
            sender: Arc::new(RwLock::new(Some(sender))),
            observers: Arc::new(observers),
            replay_source: None,
        }
    }

    /// Installs the durable recorder [`EventTransport::replay_from`] reads.
    #[must_use]
    pub fn with_replay_source(mut self, source: Arc<dyn TrajectoryService>) -> Self {
        self.replay_source = Some(source);
        self
    }

    pub fn publish(&self, event: RuntimeEvent) -> Result<usize, String> {
        let guard = self
            .sender
            .read()
            .map_err(|_| "event transport lock poisoned".to_string())?;
        let Some(sender) = guard.as_ref() else {
            return Err("event transport is closed".to_string());
        };
        sender.send(event).map_err(|error| error.to_string())
    }

    #[must_use]
    pub fn subscribe(&self) -> EventSubscription {
        let receiver = self
            .sender
            .read()
            .ok()
            .and_then(|guard| guard.as_ref().map(broadcast::Sender::subscribe));
        match receiver {
            Some(receiver) => EventSubscription { receiver },
            None => {
                // A closed transport must not hand out a live subscription, so
                // the caller sees the same `Closed` a drained channel gives.
                let (sender, receiver) = broadcast::channel(1);
                drop(sender);
                EventSubscription { receiver }
            }
        }
    }

    /// Stops delivery: live subscriptions end, later publishes are rejected,
    /// and later subscribes yield a closed subscription.
    pub fn close(&self) {
        if let Ok(mut guard) = self.sender.write() {
            guard.take();
        }
    }

    #[must_use]
    pub fn is_closed(&self) -> bool {
        self.sender
            .read()
            .map(|guard| guard.is_none())
            .unwrap_or(true)
    }

    /// Reads durable facts from `from_sequence` on, for a client that
    /// disconnected and must not lose (or duplicate) a Trajectory fact.
    ///
    /// The live channel keeps no history, so replay is answered by the
    /// installed Trajectory service rather than the bus.
    pub async fn replay_from(
        &self,
        trajectory_id: &TrajectoryId,
        from_sequence: u64,
    ) -> Result<Vec<TrajectoryEvent>, KernelError> {
        let Some(source) = self.replay_source.as_ref() else {
            return Err(KernelError::new(
                ErrorCategory::CapabilityNotFound,
                "event replay requires a trajectory source",
                "event",
                false,
            ));
        };
        source.query(trajectory_id, from_sequence).await
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
        if self.is_closed() {
            return Ok(());
        }
        for observer in self.observers.iter() {
            observer.observe(&event).await?;
        }
        // A broadcast channel only returns `SendError` when there are no
        // receivers at all; publishing to an empty audience is a no-op, not a
        // transport failure.
        let _ = EventTransport::publish(self, event);
        Ok(())
    }
}

impl RuntimeEventSink for EventTransport {
    fn emit(&self, event: RuntimeEvent) -> Result<(), KernelError> {
        let _ = EventTransport::publish(self, event);
        Ok(())
    }
}

/// Event delivery supplied by a Runtime host.
///
/// The trait deliberately takes protocol values rather than a concrete event
/// bus. Remote hosts can implement it by serializing the same value over IPC.
#[async_trait::async_trait]
pub trait EventService: Send + Sync {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::Mutex;

    use tact_protocol::{ActorId, RunId, Sensitivity, TrajectoryEventType};

    fn run_started() -> RuntimeEvent {
        RuntimeEvent::RunStarted {
            run_id: RunId::new("run-1").expect("run id"),
        }
    }

    /// `RuntimeEvent` carries no `PartialEq`, so the assertion matches the
    /// variant it expects instead of comparing whole values.
    fn assert_run_started(event: &RuntimeEvent) {
        match event {
            RuntimeEvent::RunStarted { run_id } => assert_eq!(run_id.as_str(), "run-1"),
            other => panic!("expected RunStarted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn publish_reaches_every_live_subscriber() {
        let transport = EventTransport::new(4);
        let mut first = transport.subscribe();
        let mut second = transport.subscribe();
        assert_eq!(transport.publish(run_started()).expect("publish"), 2);
        assert_run_started(&first.recv().await.expect("first subscriber"));
        assert_run_started(&second.recv().await.expect("second subscriber"));
    }

    #[tokio::test]
    async fn close_ends_live_subscriptions_and_rejects_later_publishes() {
        let transport = EventTransport::new(4);
        let mut subscription = transport.subscribe();
        transport
            .publish(run_started())
            .expect("publish before close");
        assert_run_started(&subscription.try_recv().expect("live event"));

        transport.close();
        assert!(transport.is_closed());
        assert!(
            transport.publish(run_started()).is_err(),
            "a closed transport rejects events"
        );
        assert!(matches!(
            subscription.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));

        let mut late = transport.subscribe();
        assert!(matches!(
            late.recv().await,
            Err(broadcast::error::RecvError::Closed)
        ));
    }

    #[derive(Default)]
    struct StubTrajectory {
        facts: Mutex<Vec<TrajectoryEvent>>,
    }

    fn fact(trajectory_id: &TrajectoryId, sequence: u64) -> TrajectoryEvent {
        TrajectoryEvent {
            trajectory_id: trajectory_id.clone(),
            run_id: RunId::new("run-1").expect("run id"),
            sequence,
            timestamp: chrono::Utc::now(),
            actor: ActorId::from("host"),
            event_type: TrajectoryEventType::RunLifecycle,
            parent_step_id: None,
            payload: serde_json::json!({ "sequence": sequence }),
            sensitivity: Sensitivity::Internal,
        }
    }

    #[async_trait]
    impl TrajectoryService for StubTrajectory {
        async fn append(
            &self,
            _trajectory_id: Option<&TrajectoryId>,
            _run_id: Option<&RunId>,
            _event: RuntimeEvent,
        ) -> Result<(), KernelError> {
            Ok(())
        }

        async fn query(
            &self,
            trajectory_id: &TrajectoryId,
            from_sequence: u64,
        ) -> Result<Vec<TrajectoryEvent>, KernelError> {
            Ok(self
                .facts
                .lock()
                .expect("stub trajectory lock")
                .iter()
                .filter(|fact| {
                    &fact.trajectory_id == trajectory_id && fact.sequence >= from_sequence
                })
                .cloned()
                .collect())
        }
    }

    #[tokio::test]
    async fn replay_from_reads_the_installed_trajectory_source() {
        let trajectory_id = TrajectoryId::new("traj-1").expect("trajectory id");
        let source = Arc::new(StubTrajectory::default());
        source.facts.lock().expect("stub trajectory lock").extend([
            fact(&trajectory_id, 0),
            fact(&trajectory_id, 1),
            fact(&trajectory_id, 2),
        ]);

        let transport = EventTransport::new(4).with_replay_source(source);
        let replayed = transport
            .replay_from(&trajectory_id, 1)
            .await
            .expect("replay from sequence 1");
        assert_eq!(
            replayed
                .iter()
                .map(|fact| fact.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[tokio::test]
    async fn replay_without_a_source_is_an_explicit_failure() {
        let transport = EventTransport::new(4);
        let trajectory_id = TrajectoryId::new("traj-1").expect("trajectory id");
        let error = transport
            .replay_from(&trajectory_id, 0)
            .await
            .expect_err("no trajectory source installed");
        assert_eq!(error.category(), ErrorCategory::CapabilityNotFound);
    }
}
