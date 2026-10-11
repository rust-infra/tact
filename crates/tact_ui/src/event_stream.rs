//! The headless host's reconnecting Runtime-event client.
//!
//! The interactive TUI consumes the session's event transport as a *sink*: it
//! renders whatever the broadcast delivers and has no notion of a sequence, so
//! a dropped window is invisible. This client is the other half the protocol
//! always described — a **client that watches the stream by sequence** and,
//! when its subscription falls behind, reconnects from the durable trajectory
//! instead of silently losing the facts it missed.
//!
//! The live broadcast carries no history, so it cannot answer "what did I
//! miss". [`EventTransport::replay_from`] answers it from the trajectory the
//! session already persists (`start_trajectory_recorder` installs the durable
//! source). A [`RuntimeCommand::Subscribe`] attaches to whatever run this
//! session starts; a [`RuntimeCommand::Resume`] attaches to a known trajectory
//! from a known sequence — the two reconnect commands the protocol defines and
//! nothing consumed until now.
//!
//! The headless host is the consumer and not the interactive driver because a
//! View that is *already* rendering the stream has no use for a second
//! subscription that re-delivers what it just drew (see `ARCHITECTURE.md` §15):
//! the driver owes the reconnect protocol nothing, while a headless pipeline
//! that keys on a gap-free sequence owes it everything.

use std::collections::{HashMap, VecDeque};

use tact::{EventSubscription, EventTransport};
use tact_protocol::{RuntimeCommand, RuntimeEvent, TrajectoryId};
use tact_trajectory::trajectory_for_event;
use tokio::sync::broadcast;

/// The trajectory `replay_from` is polled at when it has nothing new, while the
/// client waits for a just-closed writer to flush. Bounded so a stuck recorder
/// cannot hang a headless exit.
const FLUSH_POLL: std::time::Duration = std::time::Duration::from_millis(10);
/// How many empty passes in a row end the flush.
const FLUSH_IDLE_PASSES: u32 = 2;
/// Total flush passes before giving up, i.e. the flush is bounded at
/// `MAX_FLUSH_PASSES * FLUSH_POLL` even if the writer never catches up.
const MAX_FLUSH_PASSES: u32 = 100;

/// One line of the client's output: the fact's durable sequence, then the
/// event it records.
#[derive(Debug, Clone)]
pub struct EventStreamLine {
    /// The sequence the durable trajectory gave this fact. A consumer that
    /// reads the stream in order can spot a gap — a jump in this number — and
    /// know the client failed to recover it.
    pub sequence: u64,
    pub event: RuntimeEvent,
}

impl EventStreamLine {
    /// The NDJSON form: one JSON object per line, `seq` first so a reader sees
    /// the number without parsing the (possibly large) event.
    #[must_use]
    pub fn to_ndjson(&self) -> String {
        serde_json::json!({ "seq": self.sequence, "event": self.event }).to_string()
    }
}

/// How the client attaches, named by the protocol command that asks for it.
#[derive(Debug, Clone, PartialEq)]
pub enum EventStreamMode {
    /// Run from `from_sequence` on; the trajectory is discovered from the
    /// first fact that names one (the run `RunStarted` starts).
    Subscribe { from_sequence: Option<u64> },
    /// Attach to a known trajectory from a known sequence — a client that
    /// disconnected and came back.
    Resume {
        trajectory_id: TrajectoryId,
        from_sequence: u64,
    },
}

impl EventStreamMode {
    /// The mode a protocol command asks for, or `None` when the command is not
    /// a subscription (`StartRun`, `CancelRun`, `Invoke`, `Shutdown`, …).
    ///
    /// This is the seam that gives `RuntimeCommand::{Subscribe, Resume}` a
    /// consumer: a host that receives one of them routes it here instead of
    /// ignoring it.
    #[must_use]
    pub fn from_command(command: &RuntimeCommand) -> Option<Self> {
        match command {
            RuntimeCommand::Subscribe { from_sequence } => Some(Self::Subscribe {
                from_sequence: *from_sequence,
            }),
            RuntimeCommand::Resume {
                trajectory_id,
                from_sequence,
            } => Some(Self::Resume {
                trajectory_id: trajectory_id.clone(),
                from_sequence: *from_sequence,
            }),
            _ => None,
        }
    }
}

/// A sequence-aware subscriber to a session's event transport.
///
/// Build it with [`EventStreamClient::connect`] **before** the turn starts, so
/// the subscription is in place when the first fact is published. Then call
/// [`EventStreamClient::recv`] until it yields `None`.
pub struct EventStreamClient {
    transport: EventTransport,
    subscription: EventSubscription,
    /// The next sequence each trajectory is expected to deliver. A live fact is
    /// stamped with this value; a fact recovered from the trajectory advances it
    /// to one past the recovered head.
    next: HashMap<TrajectoryId, u64>,
    /// Recovered facts waiting to be handed out, ahead of the live stream.
    pending: VecDeque<EventStreamLine>,
    /// The sequence a trajectory starts at the first time it is seen (the
    /// `Subscribe` command's `from_sequence`).
    default_from: u64,
    /// Set when the broadcast reports a dropped window; the next loop recovers
    /// it from the trajectory instead of emitting a gapped sequence.
    recovering: bool,
    closed: bool,
}

impl EventStreamClient {
    /// Subscribes now — the caller must do this before the turn it wants to
    /// observe, or the facts already in flight are simply never seen.
    #[must_use]
    pub fn connect(transport: &EventTransport, mode: EventStreamMode) -> Self {
        let mut next = HashMap::new();
        let mut default_from = 0;
        match mode {
            EventStreamMode::Subscribe { from_sequence } => {
                default_from = from_sequence.unwrap_or(0);
            }
            EventStreamMode::Resume {
                trajectory_id,
                from_sequence,
            } => {
                next.insert(trajectory_id, from_sequence);
            }
        }
        Self {
            transport: transport.clone(),
            subscription: transport.subscribe(),
            next,
            pending: VecDeque::new(),
            default_from,
            recovering: false,
            closed: false,
        }
    }

    /// Builds the client a protocol command asks for, or `None` when the
    /// command does not subscribe. The subscription is created here, so a host
    /// routes the command before the turn, not after.
    #[must_use]
    pub fn from_command(transport: &EventTransport, command: &RuntimeCommand) -> Option<Self> {
        Some(Self::connect(
            transport,
            EventStreamMode::from_command(command)?,
        ))
    }

    /// The next line to emit, or `None` once the transport has closed and the
    /// trajectory has been drained.
    pub async fn recv(&mut self) -> Option<EventStreamLine> {
        loop {
            if let Some(line) = self.pending.pop_front() {
                return Some(line);
            }
            if self.closed {
                return None;
            }

            if self.recovering {
                if self.next.is_empty() {
                    // The gap swallowed the *first* window, before any fact
                    // named a trajectory — so there is nothing to replay yet.
                    // The next live fact is the only thing that can say which
                    // trajectory to reconnect to.
                    match self.subscription.recv().await {
                        Ok(event) => {
                            self.next
                                .insert(trajectory_for_event(&event), self.default_from);
                        }
                        Err(broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(
                                skipped,
                                "runtime event client still behind, and no trajectory to replay yet"
                            );
                            continue;
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            self.closed = true;
                            self.flush().await;
                            return self.pending.pop_front();
                        }
                    }
                }
                tracing::warn!("runtime event client recovering from the trajectory");
                self.recover().await;
                self.recovering = false;
                continue;
            }

            match self.subscription.recv().await {
                Ok(event) => return Some(self.stamp(event)),
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // The bus dropped `skipped` facts before this receiver's
                    // cursor. They are gone from the channel; the durable
                    // trajectory is where they are recovered.
                    tracing::warn!(
                        skipped,
                        "runtime event client fell behind; recovering from the trajectory"
                    );
                    self.recovering = true;
                }
                Err(broadcast::error::RecvError::Closed) => {
                    self.closed = true;
                    self.flush().await;
                    return self.pending.pop_front();
                }
            }
        }
    }

    /// Stamps a live fact with its trajectory sequence.
    ///
    /// The broadcast preserves per-sender order and the recorder appends in that
    /// same order, so the client's counter matches the trajectory's sequence for
    /// as long as the client has not skipped a window — which is exactly what a
    /// [`Self::recover`] restores when it has.
    fn stamp(&mut self, event: RuntimeEvent) -> EventStreamLine {
        let trajectory = trajectory_for_event(&event);
        let sequence = self.next.entry(trajectory).or_insert(self.default_from);
        let line = EventStreamLine {
            sequence: *sequence,
            event,
        };
        *sequence += 1;
        line
    }

    /// Drains the live channel, then re-reads every tracked trajectory from the
    /// sequence the client stopped at, queueing the facts it missed.
    ///
    /// The drain comes first: the channel still holds the newest facts it kept,
    /// and the replay returns those same facts, so delivering both would emit a
    /// sequence twice.
    async fn recover(&mut self) {
        while self.subscription.try_recv().is_ok() {}
        let wanted: Vec<(TrajectoryId, u64)> = self
            .next
            .iter()
            .map(|(trajectory, sequence)| (trajectory.clone(), *sequence))
            .collect();
        for (trajectory_id, from_sequence) in wanted {
            self.replay_into(trajectory_id, from_sequence).await;
        }
    }

    /// Queues every fact of one trajectory from `from_sequence` on.
    ///
    /// A session with no durable source (the recorder failed to open) cannot
    /// recover; keeping the live numbering is the best it can do.
    async fn replay_into(&mut self, trajectory_id: TrajectoryId, from_sequence: u64) {
        let Ok(facts) = self
            .transport
            .replay_from(&trajectory_id, from_sequence)
            .await
        else {
            return;
        };
        for fact in facts {
            let Ok(event) = serde_json::from_value::<RuntimeEvent>(fact.payload.clone()) else {
                continue;
            };
            self.pending.push_back(EventStreamLine {
                sequence: fact.sequence,
                event,
            });
            self.next.insert(trajectory_id.clone(), fact.sequence + 1);
        }
    }

    /// After the transport closes, the recorder is still draining its own
    /// subscription, so the final facts can reach SQLite a moment later. Poll
    /// the trajectory until two passes add nothing, bounded so a stuck writer
    /// cannot hold the process open.
    async fn flush(&mut self) {
        let mut idle = 0;
        for _ in 0..MAX_FLUSH_PASSES {
            let before = self.pending.len();
            self.recover().await;
            if self.pending.len() == before {
                idle += 1;
                if idle >= FLUSH_IDLE_PASSES {
                    break;
                }
                tokio::time::sleep(FLUSH_POLL).await;
            } else {
                idle = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use tact::{KernelError, TrajectoryService};
    use tact_protocol::{RunId, Sensitivity, TrajectoryEvent, TrajectoryEventType};

    /// A durable source that already holds every fact, so a client that lags on
    /// the *live* channel can still recover the whole sequence. The recorder
    /// would answer the same way; a stub keeps the test deterministic and
    /// socket-free.
    #[derive(Default)]
    struct StubTrajectory {
        facts: Mutex<Vec<TrajectoryEvent>>,
    }

    fn text(run_id: &RunId, body: &str) -> RuntimeEvent {
        RuntimeEvent::Text {
            run_id: Some(run_id.clone()),
            role: "assistant".into(),
            content: body.into(),
        }
    }

    fn fact(
        trajectory_id: &TrajectoryId,
        run_id: &RunId,
        sequence: u64,
        body: &str,
    ) -> TrajectoryEvent {
        TrajectoryEvent {
            trajectory_id: trajectory_id.clone(),
            run_id: Some(run_id.clone()),
            sequence,
            timestamp: chrono::Utc::now(),
            actor: "runtime".into(),
            event_type: TrajectoryEventType::Message,
            parent_step_id: None,
            payload: serde_json::to_value(text(run_id, body)).expect("serialize fact"),
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
                .expect("stub lock")
                .iter()
                .filter(|fact| {
                    &fact.trajectory_id == trajectory_id && fact.sequence >= from_sequence
                })
                .cloned()
                .collect())
        }
    }

    /// The point of the feature: a capacity-1 transport drops live events, and
    /// the client recovers them from the trajectory so the emitted sequence has
    /// no gaps.
    #[tokio::test]
    async fn a_lagging_subscription_recovers_a_gap_free_sequence() {
        let run_id = RunId::from("recover-run");
        let trajectory_id = TrajectoryId::from("recover-run");

        // The durable source already holds the whole run; the live channel below
        // will only manage to keep the tail of it.
        let source = Arc::new(StubTrajectory::default());
        {
            let mut facts = source.facts.lock().expect("stub lock");
            for sequence in 0..8 {
                facts.push(fact(
                    &trajectory_id,
                    &run_id,
                    sequence,
                    &format!("event-{sequence}"),
                ));
            }
        }

        // Capacity 1: publishing eight facts before the client polls drops all
        // but the last from the client's window (a forced lag).
        let transport = EventTransport::new(1).with_replay_source(source);
        let mut client = EventStreamClient::connect(
            &transport,
            EventStreamMode::Subscribe {
                from_sequence: None,
            },
        );
        for sequence in 0..8 {
            transport
                .publish(text(&run_id, &format!("event-{sequence}")))
                .expect("publish");
        }

        // The client recovers every dropped fact from the trajectory: it emits
        // all eight, in order, from the one live window it still had.
        let mut lines = Vec::new();
        for _ in 0..8 {
            let line = tokio::time::timeout(std::time::Duration::from_secs(1), client.recv())
                .await
                .expect("recovery must not block")
                .expect("each recovered fact is emitted");
            lines.push(line);
        }
        // End the stream so the client terminates instead of waiting forever.
        transport.close();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(1), client.recv())
                .await
                .expect("close must end the client")
                .is_none()
        );

        // Every fact, once, in order — the exact trajectory sequence, no gaps.
        let sequences: Vec<u64> = lines.iter().map(|line| line.sequence).collect();
        assert_eq!(sequences, (0..8).collect::<Vec<_>>());
        for (expected, line) in lines.iter().enumerate() {
            match &line.event {
                RuntimeEvent::Text { content, .. } => {
                    assert_eq!(content, &format!("event-{expected}"));
                }
                other => panic!("expected Text, got {other:?}"),
            }
        }
    }

    /// A client that starts after facts exist resyncs to the trajectory head
    /// rather than emitting the facts it never saw.
    #[tokio::test]
    async fn resume_starts_at_the_requested_sequence() {
        let run_id = RunId::from("resume-run");
        let trajectory_id = TrajectoryId::from("resume-run");

        let source = Arc::new(StubTrajectory::default());
        {
            let mut facts = source.facts.lock().expect("stub lock");
            for sequence in 0..4 {
                facts.push(fact(
                    &trajectory_id,
                    &run_id,
                    sequence,
                    &format!("event-{sequence}"),
                ));
            }
        }
        let transport = EventTransport::new(8).with_replay_source(source);

        // Resume from sequence 2 for that trajectory: the recovered facts are
        // 2 and 3, not 0 and 1. Closing ends the stream, so the client drains
        // the trajectory and stops.
        let mut client = EventStreamClient::connect(
            &transport,
            EventStreamMode::Resume {
                trajectory_id: trajectory_id.clone(),
                from_sequence: 2,
            },
        );
        transport.close();

        let mut sequences = Vec::new();
        while let Some(line) =
            tokio::time::timeout(std::time::Duration::from_secs(1), client.recv())
                .await
                .expect("recovery must not block")
        {
            sequences.push(line.sequence);
        }
        assert_eq!(sequences, vec![2, 3]);
    }

    /// `RuntimeCommand::{Subscribe, Resume}` route to the client instead of
    /// being ignored: each names the attachment the client will make.
    #[test]
    fn subscribe_and_resume_commands_reach_the_client() {
        let subscribe = RuntimeCommand::Subscribe {
            from_sequence: Some(5),
        };
        assert_eq!(
            EventStreamMode::from_command(&subscribe),
            Some(EventStreamMode::Subscribe {
                from_sequence: Some(5)
            })
        );

        let trajectory_id = TrajectoryId::from("resume-command");
        let resume = RuntimeCommand::Resume {
            trajectory_id: trajectory_id.clone(),
            from_sequence: 9,
        };
        assert_eq!(
            EventStreamMode::from_command(&resume),
            Some(EventStreamMode::Resume {
                trajectory_id,
                from_sequence: 9,
            })
        );

        // A command that is not a subscription does not build a client.
        assert_eq!(
            EventStreamMode::from_command(&RuntimeCommand::Shutdown),
            None
        );
        assert_eq!(
            EventStreamMode::from_command(&RuntimeCommand::Subscribe {
                from_sequence: None
            }),
            Some(EventStreamMode::Subscribe {
                from_sequence: None
            })
        );
    }

    /// A `Subscribe { from_sequence: Some(n) }` stamps the first live fact of a
    /// freshly seen trajectory with `n`.
    #[tokio::test]
    async fn subscribe_from_sequence_stamps_the_first_live_fact() {
        let run_id = RunId::from("subscribe-from");
        let transport = EventTransport::new(8);
        let mut client = EventStreamClient::connect(
            &transport,
            EventStreamMode::Subscribe {
                from_sequence: Some(3),
            },
        );
        transport.publish(text(&run_id, "live")).expect("publish");
        transport.close();

        let line = client.recv().await.expect("one live fact");
        assert_eq!(line.sequence, 3);
    }
}
