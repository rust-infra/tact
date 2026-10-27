//! Ordered host update delivery.
//!
//! Kernel events have two different delivery guarantees. Progress is advisory:
//! a slow frontend may cause it to be coalesced or dropped. Terminal updates
//! are lifecycle, result, usage, and failure events: they must remain reliable
//! and ordered. [`UpdateSink`] fans both kinds out to the answering frontend
//! and any read-only observers.
//!
//! The production channel from [`update_channel`] keeps progress bounded and
//! sends terminal updates through an unbounded ordered lane. A receiver merges
//! the two lanes without letting a later terminal update overtake progress that
//! preceded it. The legacy [`UpdateSink::for_ui`] constructor accepts an
//! existing unbounded sender so small embedders and tests keep the simple
//! all-events path.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender, error::TryRecvError};

use crate::AgentUpdate;

/// Failure to enqueue a terminal update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdateSendError;

/// Common send surface for policy-aware and legacy update senders.
pub trait UpdateDispatch {
    fn send_update(&self, update: AgentUpdate) -> Result<(), UpdateSendError>;
}

/// A sender for one host update destination.
///
/// The raw form preserves the historical unbounded `AgentUpdate` channel. The
/// split form is created by [`update_channel`] and provides bounded progress
/// plus reliable terminal delivery.
#[derive(Clone, Debug)]
pub struct UpdateSender {
    mode: UpdateSenderMode,
}

#[derive(Clone, Debug)]
enum UpdateSenderMode {
    Raw(UnboundedSender<AgentUpdate>),
    Split {
        terminal: UnboundedSender<UpdateEnvelope>,
        progress: mpsc::Sender<UpdateEnvelope>,
        next_progress: Arc<AtomicU64>,
        next_terminal: Arc<AtomicU64>,
        dropped: Arc<Mutex<VecDeque<(u64, u64)>>>,
        dropped_notify: mpsc::Sender<()>,
    },
}

#[derive(Debug, Clone)]
enum UpdateEnvelope {
    Progress {
        seq: u64,
        after_terminal: u64,
        update: AgentUpdate,
    },
    Terminal {
        seq: u64,
        progress_through: u64,
        update: AgentUpdate,
    },
}

/// The receiving half of [`update_channel`].
///
/// It supports the same `try_recv` and `recv` surface as the old unbounded
/// receiver, so TUI consumers do not need to know which delivery policy a
/// producer selected.
#[derive(Debug)]
pub struct UpdateReceiver {
    mode: UpdateReceiverMode,
}

#[derive(Debug)]
enum UpdateReceiverMode {
    Raw(UnboundedReceiver<AgentUpdate>),
    Split(SplitUpdateReceiver),
}

#[derive(Debug)]
struct SplitUpdateReceiver {
    terminal: UnboundedReceiver<UpdateEnvelope>,
    progress: mpsc::Receiver<UpdateEnvelope>,
    dropped_notify: mpsc::Receiver<()>,
    progress_assigned: Arc<AtomicU64>,
    dropped: Arc<Mutex<VecDeque<(u64, u64)>>>,
    terminal_closed: bool,
    progress_closed: bool,
    dropped_closed: bool,
    next_terminal: u64,
    next_progress: u64,
    terminal_buffer: BTreeMap<u64, (u64, AgentUpdate)>,
    progress_buffer: BTreeMap<u64, (u64, AgentUpdate)>,
    dropped_ranges: VecDeque<(u64, u64)>,
}

impl UpdateSender {
    /// Send one reliable, ordered terminal update.
    pub fn send(&self, update: AgentUpdate) -> Result<(), UpdateSendError> {
        match &self.mode {
            UpdateSenderMode::Raw(tx) => tx.send(update).map_err(|_| UpdateSendError),
            UpdateSenderMode::Split {
                terminal,
                next_progress,
                next_terminal,
                ..
            } => {
                // Read the progress barrier before assigning the terminal
                // sequence. A progress send after this point therefore waits
                // for this terminal update instead of forming a two-way wait.
                let progress_through = next_progress.load(Ordering::SeqCst);
                let seq = next_terminal.fetch_add(1, Ordering::SeqCst);
                terminal
                    .send(UpdateEnvelope::Terminal {
                        seq,
                        progress_through,
                        update,
                    })
                    .map_err(|_| UpdateSendError)
            }
        }
    }

    /// Send one advisory progress update.
    ///
    /// In split mode this never waits for a consumer. A full progress lane
    /// drops the update and records the gap so a later terminal update remains
    /// deliverable.
    pub fn send_progress(&self, update: AgentUpdate) -> Result<(), UpdateSendError> {
        match &self.mode {
            UpdateSenderMode::Raw(tx) => tx.send(update).map_err(|_| UpdateSendError),
            UpdateSenderMode::Split {
                progress,
                next_progress,
                next_terminal,
                dropped,
                dropped_notify,
                ..
            } => {
                // Read the terminal barrier before assigning the progress
                // sequence. See the matching comment in `send`.
                let after_terminal = next_terminal.load(Ordering::SeqCst);
                let seq = next_progress.fetch_add(1, Ordering::SeqCst);
                let envelope = UpdateEnvelope::Progress {
                    seq,
                    after_terminal,
                    update,
                };
                match progress.try_send(envelope) {
                    Ok(()) => Ok(()),
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        record_dropped(seq, dropped, dropped_notify);
                        Ok(())
                    }
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        record_dropped(seq, dropped, dropped_notify);
                        Err(UpdateSendError)
                    }
                }
            }
        }
    }
}

impl From<UnboundedSender<AgentUpdate>> for UpdateSender {
    fn from(tx: UnboundedSender<AgentUpdate>) -> Self {
        Self {
            mode: UpdateSenderMode::Raw(tx),
        }
    }
}

impl UpdateDispatch for UpdateSender {
    fn send_update(&self, update: AgentUpdate) -> Result<(), UpdateSendError> {
        self.send(update)
    }
}

impl UpdateDispatch for UnboundedSender<AgentUpdate> {
    fn send_update(&self, update: AgentUpdate) -> Result<(), UpdateSendError> {
        self.send(update).map_err(|_| UpdateSendError)
    }
}

impl UpdateReceiver {
    /// Try to receive the next update without waiting.
    pub fn try_recv(&mut self) -> Result<AgentUpdate, TryRecvError> {
        match &mut self.mode {
            UpdateReceiverMode::Raw(rx) => rx.try_recv(),
            UpdateReceiverMode::Split(rx) => rx.try_recv(),
        }
    }

    /// Receive the next update in delivery order.
    pub async fn recv(&mut self) -> Option<AgentUpdate> {
        match &mut self.mode {
            UpdateReceiverMode::Raw(rx) => rx.recv().await,
            UpdateReceiverMode::Split(rx) => rx.recv().await,
        }
    }

    /// Build a receiver over the legacy unbounded channel shape.
    pub fn from_unbounded(rx: UnboundedReceiver<AgentUpdate>) -> Self {
        Self {
            mode: UpdateReceiverMode::Raw(rx),
        }
    }
}

impl From<UnboundedReceiver<AgentUpdate>> for UpdateReceiver {
    fn from(rx: UnboundedReceiver<AgentUpdate>) -> Self {
        Self::from_unbounded(rx)
    }
}

/// Create a production update channel.
///
/// Progress has at most `progress_capacity` queued updates. Terminal updates
/// use a separate unbounded lane and are never dropped. The two lanes are
/// merged by [`UpdateReceiver`] in send order.
///
/// The terminal lane is deliberately unbounded: terminal traffic is low-volume
/// and correctness requires that lifecycle and result updates survive a slow
/// consumer. A host that stops draining the receiver can therefore accumulate
/// terminal updates; that is a stalled-host failure, not a reason to silently
/// drop state transitions.
pub fn update_channel(progress_capacity: usize) -> (UpdateSender, UpdateReceiver) {
    let (terminal_tx, terminal_rx) = mpsc::unbounded_channel();
    let (progress_tx, progress_rx) = mpsc::channel(progress_capacity.max(1));
    let (dropped_tx, dropped_rx) = mpsc::channel(1);
    let next_progress = Arc::new(AtomicU64::new(0));
    let next_terminal = Arc::new(AtomicU64::new(0));
    let dropped = Arc::new(Mutex::new(VecDeque::new()));

    let sender = UpdateSender {
        mode: UpdateSenderMode::Split {
            terminal: terminal_tx,
            progress: progress_tx,
            next_progress: next_progress.clone(),
            next_terminal,
            dropped: dropped.clone(),
            dropped_notify: dropped_tx,
        },
    };
    let receiver = UpdateReceiver {
        mode: UpdateReceiverMode::Split(SplitUpdateReceiver {
            terminal: terminal_rx,
            progress: progress_rx,
            dropped_notify: dropped_rx,
            progress_assigned: next_progress,
            dropped,
            terminal_closed: false,
            progress_closed: false,
            dropped_closed: false,
            next_terminal: 0,
            next_progress: 0,
            terminal_buffer: BTreeMap::new(),
            progress_buffer: BTreeMap::new(),
            dropped_ranges: VecDeque::new(),
        }),
    };
    (sender, receiver)
}

impl SplitUpdateReceiver {
    fn try_recv(&mut self) -> Result<AgentUpdate, TryRecvError> {
        loop {
            if let Some(update) = self.next_ready() {
                return Ok(update);
            }
            let mut advanced = false;
            advanced |= self.drain_terminal();
            advanced |= self.drain_progress();
            advanced |= self.drain_dropped();
            if let Some(update) = self.next_ready() {
                return Ok(update);
            }
            if self.done() {
                return Err(TryRecvError::Disconnected);
            }
            if !advanced {
                return Err(TryRecvError::Empty);
            }
        }
    }

    async fn recv(&mut self) -> Option<AgentUpdate> {
        loop {
            if let Some(update) = self.next_ready() {
                return Some(update);
            }
            if self.done() {
                return None;
            }

            tokio::select! {
                biased;
                envelope = self.terminal.recv(), if !self.terminal_closed => {
                    match envelope {
                        Some(envelope) => self.accept_envelope(envelope),
                        None => self.terminal_closed = true,
                    }
                }
                envelope = self.progress.recv(), if !self.progress_closed => {
                    match envelope {
                        Some(envelope) => self.accept_envelope(envelope),
                        None => {
                            self.progress_closed = true;
                            self.mark_remaining_progress_dropped();
                        }
                    }
                }
                marker = self.dropped_notify.recv(), if !self.dropped_closed => {
                    if marker.is_none() {
                        self.dropped_closed = true;
                    }
                    self.drain_dropped();
                }
            }
        }
    }

    fn next_ready(&mut self) -> Option<AgentUpdate> {
        self.skip_dropped();

        if let Some((seq, (progress_through, update))) = self.terminal_buffer.first_key_value()
            && *seq == self.next_terminal
            && self.next_progress >= *progress_through
        {
            let update = update.clone();
            self.terminal_buffer.remove(&self.next_terminal);
            self.next_terminal = self.next_terminal.saturating_add(1);
            return Some(update);
        }

        if let Some((seq, (after_terminal, update))) = self.progress_buffer.first_key_value()
            && *seq == self.next_progress
            && self.next_terminal >= *after_terminal
        {
            let update = update.clone();
            self.progress_buffer.remove(&self.next_progress);
            self.next_progress = self.next_progress.saturating_add(1);
            return Some(update);
        }

        None
    }

    fn accept_envelope(&mut self, envelope: UpdateEnvelope) {
        match envelope {
            UpdateEnvelope::Terminal {
                seq,
                progress_through,
                update,
            } => {
                if seq >= self.next_terminal {
                    self.terminal_buffer
                        .entry(seq)
                        .or_insert((progress_through, update));
                }
            }
            UpdateEnvelope::Progress {
                seq,
                after_terminal,
                update,
            } => {
                if seq >= self.next_progress {
                    self.progress_buffer
                        .entry(seq)
                        .or_insert((after_terminal, update));
                }
            }
        }
    }

    fn drain_terminal(&mut self) -> bool {
        let mut advanced = false;
        loop {
            match self.terminal.try_recv() {
                Ok(envelope) => {
                    advanced = true;
                    self.accept_envelope(envelope);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.terminal_closed = true;
                    break;
                }
            }
        }
        advanced
    }

    fn drain_progress(&mut self) -> bool {
        let mut advanced = false;
        loop {
            match self.progress.try_recv() {
                Ok(envelope) => {
                    advanced = true;
                    self.accept_envelope(envelope);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.progress_closed = true;
                    self.mark_remaining_progress_dropped();
                    break;
                }
            }
        }
        advanced
    }

    fn drain_dropped(&mut self) -> bool {
        let mut advanced = false;
        loop {
            match self.dropped_notify.try_recv() {
                Ok(()) => {
                    advanced = true;
                    self.take_shared_dropped();
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.dropped_closed = true;
                    self.take_shared_dropped();
                    break;
                }
            }
        }
        advanced
    }

    fn take_shared_dropped(&mut self) {
        let ranges = {
            let mut shared = self
                .dropped
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            std::mem::take(&mut *shared)
        };
        for range in ranges {
            insert_range(&mut self.dropped_ranges, range);
        }
    }

    fn mark_remaining_progress_dropped(&mut self) {
        self.take_shared_dropped();
        let assigned = self.progress_assigned.load(Ordering::SeqCst);
        if assigned > self.next_progress {
            insert_range(
                &mut self.dropped_ranges,
                (self.next_progress, assigned.saturating_sub(1)),
            );
        }
    }

    fn skip_dropped(&mut self) {
        while let Some((start, end)) = self.dropped_ranges.front().copied() {
            if end < self.next_progress {
                self.dropped_ranges.pop_front();
                continue;
            }
            if start <= self.next_progress && self.next_progress <= end {
                self.next_progress = end.saturating_add(1);
                self.dropped_ranges.pop_front();
                continue;
            }
            break;
        }
    }

    fn done(&mut self) -> bool {
        self.take_shared_dropped();
        self.skip_dropped();
        self.terminal_closed
            && self.progress_closed
            && self.dropped_closed
            && self.terminal_buffer.is_empty()
            && self.progress_buffer.is_empty()
    }
}

fn record_dropped(
    seq: u64,
    dropped: &Arc<Mutex<VecDeque<(u64, u64)>>>,
    dropped_notify: &mpsc::Sender<()>,
) {
    let mut ranges = dropped
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    insert_range(&mut ranges, (seq, seq));
    drop(ranges);
    // A single pending notification is enough: the receiver reads the shared
    // range set when it wakes, so coalescing drops cannot lose a gap.
    let _ = dropped_notify.try_send(());
}

fn insert_range(ranges: &mut VecDeque<(u64, u64)>, range: (u64, u64)) {
    ranges.push_back(range);
    let mut all: Vec<_> = ranges.drain(..).collect();
    all.sort_unstable();
    let mut merged: VecDeque<(u64, u64)> = VecDeque::with_capacity(all.len());
    for (start, end) in all {
        if let Some((_, last_end)) = merged.back_mut()
            && start <= last_end.saturating_add(1)
        {
            *last_end = (*last_end).max(end);
        } else {
            merged.push_back((start, end));
        }
    }
    *ranges = merged;
}

/// Where one agent's updates go: at most one answering frontend, plus any
/// number of read-only observers.
#[derive(Clone, Debug, Default)]
pub struct UpdateSink {
    ui: Option<UpdateSender>,
    observers: Vec<UpdateSender>,
}

impl UpdateSink {
    /// A sink for a frontend that can answer prompts (the TUI).
    pub fn for_ui(tx: impl Into<UpdateSender>) -> Self {
        Self {
            ui: Some(tx.into()),
            observers: Vec::new(),
        }
    }

    /// A sink built from an optional legacy answering channel.
    pub fn from_ui(tx: Option<UnboundedSender<AgentUpdate>>) -> Self {
        Self::from_ui_sender(tx.map(Into::into))
    }

    /// A sink built from an optional already-policy-aware answering channel.
    pub fn from_ui_sender(tx: Option<UpdateSender>) -> Self {
        Self {
            ui: tx,
            observers: Vec::new(),
        }
    }

    /// A sink for a frontend that only watches (headless).
    pub fn observing(tx: impl Into<UpdateSender>) -> Self {
        Self {
            ui: None,
            observers: vec![tx.into()],
        }
    }

    /// Add a read-only mirror, keeping the answering frontend (if any).
    #[must_use]
    pub fn with_observer(mut self, tx: impl Into<UpdateSender>) -> Self {
        self.observers.push(tx.into());
        self
    }

    /// A copy of this sink with the answering channel removed.
    #[must_use]
    pub fn observers_only(&self) -> Self {
        Self {
            ui: None,
            observers: self.observers.clone(),
        }
    }

    /// The reliable channel a prompt or control update may be sent on.
    pub fn ui(&self) -> Option<&UpdateSender> {
        self.ui.as_ref()
    }

    /// Whether a frontend that can answer prompts is attached.
    pub fn has_ui(&self) -> bool {
        self.ui.is_some()
    }

    /// Whether nothing is attached at all.
    pub fn is_empty(&self) -> bool {
        self.ui.is_none() && self.observers.is_empty()
    }

    /// Send one reliable, ordered update to every destination.
    pub fn send(&self, update: AgentUpdate) {
        for observer in &self.observers {
            let _ = observer.send(update.clone());
        }
        if let Some(tx) = &self.ui {
            let _ = tx.send(update);
        }
    }

    /// Send one advisory progress update to every destination.
    ///
    /// A slow split-mode destination drops progress instead of applying
    /// backpressure to the agent.
    pub fn send_progress(&self, update: AgentUpdate) {
        for observer in &self.observers {
            let _ = observer.send_progress(update.clone());
        }
        if let Some(tx) = &self.ui {
            let _ = tx.send_progress(update);
        }
    }
}

impl From<Option<UnboundedSender<AgentUpdate>>> for UpdateSink {
    fn from(tx: Option<UnboundedSender<AgentUpdate>>) -> Self {
        Self::from_ui(tx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AgentUpdate, ThinkingChunk};

    #[test]
    fn an_observer_sees_a_copy_and_the_ui_keeps_the_original() {
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let (obs_tx, mut obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = UpdateSink::for_ui(ui_tx).with_observer(obs_tx);

        sink.send(AgentUpdate::Info("both".into()));

        let ui = ui_rx.try_recv().expect("ui got the update");
        let obs = obs_rx.try_recv().expect("observer got a copy");
        assert!(matches!(ui, AgentUpdate::Info(ref t) if t == "both"));
        assert!(matches!(obs, AgentUpdate::Info(ref t) if t == "both"));
    }

    #[test]
    fn an_observer_only_sink_cannot_answer_prompts() {
        let (obs_tx, _obs_rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = UpdateSink::observing(obs_tx);

        assert!(sink.ui().is_none(), "no frontend can answer");
        assert!(!sink.has_ui(), "an observer is not an answering frontend");
        assert!(!sink.is_empty(), "an observer is still attached");
    }

    #[test]
    fn an_empty_sink_drops_updates_without_panicking() {
        let sink = UpdateSink::default();
        assert!(sink.is_empty());
        sink.send(AgentUpdate::Info("nobody home".into()));
        sink.send_progress(AgentUpdate::StreamChunk("nobody home".into()));
    }

    #[test]
    fn an_optional_ui_channel_becomes_a_sink() {
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let sink = UpdateSink::from(Some(ui_tx));
        assert!(sink.has_ui());

        sink.send(AgentUpdate::Info("wired".into()));
        assert!(ui_rx.try_recv().is_ok());

        let empty = UpdateSink::from(None);
        assert!(empty.is_empty());
    }

    #[test]
    fn a_closed_observer_does_not_stop_the_ui() {
        let (ui_tx, mut ui_rx) = tokio::sync::mpsc::unbounded_channel();
        let (obs_tx, obs_rx) = tokio::sync::mpsc::unbounded_channel();
        drop(obs_rx);
        let sink = UpdateSink::for_ui(ui_tx).with_observer(obs_tx);

        sink.send(AgentUpdate::Info("still delivered".into()));

        assert!(ui_rx.try_recv().is_ok());
    }

    #[test]
    fn bounded_progress_drops_and_terminal_updates_survive() {
        let (tx, mut rx) = update_channel(1);
        tx.send_progress(AgentUpdate::StreamChunk("first".into()))
            .unwrap();
        for index in 0..100 {
            tx.send_progress(AgentUpdate::StreamChunk(format!("drop {index}")))
                .unwrap();
        }
        tx.send(AgentUpdate::Info("terminal".into())).unwrap();

        let mut received = Vec::new();
        while let Ok(update) = rx.try_recv() {
            received.push(update);
        }

        assert!(
            received
                .iter()
                .any(|update| matches!(update, AgentUpdate::Info(text) if text == "terminal")),
            "terminal update must survive progress pressure: {received:?}"
        );
        assert!(
            received.len() <= 3,
            "progress should be bounded, got {} updates",
            received.len()
        );
    }

    #[test]
    fn terminal_and_progress_keep_send_order() {
        let (tx, mut rx) = update_channel(8);
        tx.send_progress(AgentUpdate::StreamChunk("a".into()))
            .unwrap();
        tx.send(AgentUpdate::Info("middle".into())).unwrap();
        tx.send_progress(AgentUpdate::StreamChunk("b".into()))
            .unwrap();
        tx.send(AgentUpdate::Info("last".into())).unwrap();

        assert!(matches!(rx.try_recv(), Ok(AgentUpdate::StreamChunk(text)) if text == "a"));
        assert!(matches!(rx.try_recv(), Ok(AgentUpdate::Info(text)) if text == "middle"));
        assert!(matches!(rx.try_recv(), Ok(AgentUpdate::StreamChunk(text)) if text == "b"));
        assert!(matches!(rx.try_recv(), Ok(AgentUpdate::Info(text)) if text == "last"));
    }

    #[test]
    fn progress_lifecycle_values_are_not_terminal() {
        let (tx, mut rx) = update_channel(2);
        tx.send_progress(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(
            "thinking".into(),
        )))
        .unwrap();
        tx.send(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished))
            .unwrap();

        assert!(matches!(
            rx.try_recv(),
            Ok(AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(text))) if text == "thinking"
        ));
        assert!(matches!(
            rx.try_recv(),
            Ok(AgentUpdate::ThinkingChunk(ThinkingChunk::Finished))
        ));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_producers_keep_every_terminal_update() {
        use std::{collections::BTreeMap, time::Duration};

        const PRODUCERS: usize = 8;
        const STEPS: usize = 100;
        const TERMINAL_EVERY: usize = 10;

        let (tx, mut rx) = update_channel(2);
        let mut tasks = Vec::new();
        for producer in 0..PRODUCERS {
            let tx = tx.clone();
            tasks.push(tokio::spawn(async move {
                for step in 0..STEPS {
                    tx.send_progress(AgentUpdate::StreamChunk(format!(
                        "progress-{producer}-{step}"
                    )))
                    .expect("progress sender stays connected");
                    if step % TERMINAL_EVERY == 0 {
                        tx.send(AgentUpdate::Info(format!("terminal-{producer}-{step}")))
                            .expect("terminal sender stays connected");
                    }
                }
            }));
        }
        drop(tx);

        for task in tasks {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("producer timed out")
                .expect("producer panicked");
        }

        let terminals = tokio::time::timeout(Duration::from_secs(5), async {
            let mut terminals = Vec::new();
            while let Some(update) = rx.recv().await {
                if let AgentUpdate::Info(text) = update {
                    terminals.push(text);
                }
            }
            terminals
        })
        .await
        .expect("receiver did not close after every sender dropped");

        let expected = PRODUCERS * STEPS.div_ceil(TERMINAL_EVERY);
        assert_eq!(terminals.len(), expected, "{terminals:?}");

        let mut by_producer: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for terminal in terminals {
            let mut fields = terminal.split('-');
            assert_eq!(fields.next(), Some("terminal"), "{terminal}");
            let producer = fields
                .next()
                .and_then(|value| value.parse().ok())
                .expect("producer id");
            let step = fields
                .next()
                .and_then(|value| value.parse().ok())
                .expect("step id");
            assert_eq!(fields.next(), None, "{terminal}");
            by_producer.entry(producer).or_default().push(step);
        }

        assert_eq!(by_producer.len(), PRODUCERS);
        for (producer, steps) in by_producer {
            assert_eq!(steps.len(), STEPS.div_ceil(TERMINAL_EVERY));
            assert!(
                steps.windows(2).all(|pair| pair[0] < pair[1]),
                "producer {producer} lost terminal order: {steps:?}"
            );
        }
    }

    #[tokio::test]
    async fn closing_drains_queued_updates_before_disconnect() {
        use std::time::Duration;

        let (tx, mut rx) = update_channel(2);
        tx.send_progress(AgentUpdate::StreamChunk("queued".into()))
            .expect("progress sender stays connected");
        tx.send(AgentUpdate::Info("terminal".into()))
            .expect("terminal sender stays connected");
        drop(tx);

        let received = tokio::time::timeout(Duration::from_secs(5), async {
            let mut received = Vec::new();
            while let Some(update) = rx.recv().await {
                received.push(update);
            }
            received
        })
        .await
        .expect("receiver did not observe channel close");

        assert!(matches!(
            received.as_slice(),
            [
                AgentUpdate::StreamChunk(progress),
                AgentUpdate::Info(terminal)
            ] if progress == "queued" && terminal == "terminal"
        ));
    }
}
