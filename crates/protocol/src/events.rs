//! The fan-out sink every [`AgentUpdate`] travels through.
//!
//! Two kinds of frontend consume the same event stream, and they differ in
//! exactly one respect: whether they can answer a prompt. A TUI can, so it
//! takes the *ui* channel — that channel is the ask path, and holding it is a
//! claim that prompts will be answered. A headless run cannot, so it only
//! *observes*: it gets a copy of every update and answers nothing.
//!
//! Splitting the two into separate fields is what makes the distinction
//! enforceable rather than conventional: [`Self::emit`] fans out to both, while
//! [`Self::ui`] is the only handle a prompt may use, and it is `None` for an
//! observer-only sink.

use tokio::sync::mpsc::UnboundedSender;

use crate::AgentUpdate;

/// Where one agent's updates go: at most one answering frontend, plus any
/// number of read-only observers.
///
/// Cloneable and cheap — the senders are handles, and the observer list is
/// shared. `Default` is the honest value for "nobody is listening": every
/// [`Self::emit`] is a no-op, and [`Self::is_some`] is `false`.
#[derive(Clone, Debug, Default)]
pub struct UpdateSink {
    /// The frontend that can answer prompts. `None` means no one can.
    ui: Option<UnboundedSender<AgentUpdate>>,
    /// Read-only mirrors. Each update is cloned into every one of them.
    observers: Vec<UnboundedSender<AgentUpdate>>,
}

impl UpdateSink {
    /// A sink for a frontend that can answer prompts (the TUI).
    pub fn for_ui(tx: UnboundedSender<AgentUpdate>) -> Self {
        Self {
            ui: Some(tx),
            observers: Vec::new(),
        }
    }

    /// A sink built from an optional answering channel.
    ///
    /// Lets call sites that still carry `Option<UnboundedSender>` (the TUI's
    /// command driver) reach the sink-taking helpers without an `if let` at
    /// every one of them.
    pub fn from_ui(tx: Option<UnboundedSender<AgentUpdate>>) -> Self {
        Self {
            ui: tx,
            observers: Vec::new(),
        }
    }

    /// A sink for a frontend that only watches (headless).
    pub fn observing(tx: UnboundedSender<AgentUpdate>) -> Self {
        Self {
            ui: None,
            observers: vec![tx],
        }
    }

    /// Add a read-only mirror, keeping the answering frontend (if any).
    #[must_use]
    pub fn with_observer(mut self, tx: UnboundedSender<AgentUpdate>) -> Self {
        self.observers.push(tx);
        self
    }

    /// A copy of this sink with the answering channel removed.
    ///
    /// Used when handing a child the parent's destinations: a subagent's
    /// events should reach the same log, but a child must not acquire the
    /// parent's claim that prompts get answered.
    pub fn observers_only(&self) -> Self {
        Self {
            ui: None,
            observers: self.observers.clone(),
        }
    }

    /// The channel a prompt may be sent on.
    ///
    /// `None` for an observer-only sink, which is what makes a permission
    /// check take the non-interactive path instead of waiting on an answer
    /// that cannot come.
    pub fn ui(&self) -> Option<&UnboundedSender<AgentUpdate>> {
        self.ui.as_ref()
    }

    /// Whether a frontend that can answer prompts is attached.
    pub fn is_some(&self) -> bool {
        self.ui.is_some()
    }

    /// Whether nothing is attached at all.
    pub fn is_none(&self) -> bool {
        self.ui.is_none() && self.observers.is_empty()
    }

    /// Send one update to every destination.
    ///
    /// Observers get a clone so they can never consume the copy the answering
    /// frontend needs; the ui channel takes the original. A closed receiver is
    /// ignored: the run must not fail because a log reader went away.
    pub fn send(&self, update: AgentUpdate) {
        for observer in &self.observers {
            let _ = observer.send(update.clone());
        }
        if let Some(tx) = &self.ui {
            let _ = tx.send(update);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AgentUpdate;

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
        assert!(!sink.is_some());
        assert!(!sink.is_none(), "an observer is still attached");
    }

    #[test]
    fn an_empty_sink_drops_updates_without_panicking() {
        let sink = UpdateSink::default();
        assert!(sink.is_none());
        sink.send(AgentUpdate::Info("nobody home".into()));
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
}
