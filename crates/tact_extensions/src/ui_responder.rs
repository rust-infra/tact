//! Shared request/reply registry for UI prompts (`RequestSelect` /
//! `RequestMultiSelect`).
//!
//! The agent runtime no longer embeds a `tokio::sync::oneshot::Sender` inside
//! [`AgentUpdate`] (that made the protocol enum impossible to serialize and
//! coupled the transport to a single in-process responder). Instead a tool or
//! the permission gate asks [`UiResponder`] for a selection, which:
//!
//! 1. allocates a globally-unique `request_id`,
//! 2. records the request metadata plus a pending oneshot for that id,
//! 3. emits a pure-data `RequestSelect` / `RequestMultiSelect` carrying the id,
//! 4. awaits the answer, which arrives via [`UiResponder::respond`] (or the
//!    protocol `InteractionResponse`) from the
//!    TUI or the command driver.
//!
//! The pending map is also exposed as an ordered [`UiResponder::snapshot`]. In
//! interactive mode the TUI reconciles its select popup from that snapshot and
//! treats `RequestSelect` as a wake-up hint only; a lost or duplicated hint can
//! no longer leave a waiter hanging. The legacy event path remains for
//! headless/tests when no broker snapshot is attached.
//!
//! The id allocator and pending map live behind an `Arc`, so the parent agent
//! and every subagent (which clone the same [`ToolContext`]) share one
//! namespace: a response can always be routed to the exact waiter, even when a
//! subagent forwarded its request through the parent's UI channel.

use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::utils::LockExt;
use tact_view::AgentUpdate;
#[cfg(any(test, feature = "test-support"))]
use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::oneshot;

/// Why a UI select request failed to complete.
///
/// Distinct from the payload types (`Option<usize>` / `Option<Vec<usize>>`),
/// which represent a completed request that was answered or cancelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiRequestError {
    /// The UI channel closed before answering (the responder was shut down or
    /// the request could not be delivered).
    Closed,
    /// The response variant did not match the request kind — a protocol bug.
    Mismatched,
}

impl fmt::Display for UiRequestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            UiRequestError::Closed => f.write_str("UI closed before answering"),
            UiRequestError::Mismatched => f.write_str("UI response variant mismatch"),
        }
    }
}

impl std::error::Error for UiRequestError {}

/// A pending UI select request as seen by an in-process UI.
///
/// This is intentionally not a `tact_protocol` type yet: Step 2 keeps the
/// authoritative pending state in the shared [`UiResponder`] while the TUI
/// reconciles from [`UiResponder::snapshot`]. A future server transport can
/// expose the same fields through a versioned protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingUiRequest {
    pub request_id: u64,
    pub prompt: String,
    pub options: Vec<String>,
    pub kind: PendingUiRequestKind,
    pub log_confirm: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingUiRequestKind {
    Select,
    MultiSelect,
}

struct PendingEntry {
    request: PendingUiRequest,
    tx: oneshot::Sender<tact_protocol::InteractionResponse>,
}

/// Removes the pending entry if the waiting future is dropped/aborted before
/// an answer arrives. Without this, the TUI snapshot could keep rendering a
/// ghost prompt for a waiter that no longer exists.
struct PendingRequestGuard {
    responder: UiResponder,
    request_id: u64,
}

impl Drop for PendingRequestGuard {
    fn drop(&mut self) {
        self.responder.withdraw(self.request_id);
    }
}

/// Shared registry routing UI responses back to the waiting caller.
///
/// Cheap to clone: every clone shares the same inner state.
#[derive(Clone, Default)]
pub struct UiResponder {
    inner: Arc<UiResponderInner>,
}

#[derive(Default)]
struct UiResponderInner {
    pending: Mutex<HashMap<u64, PendingEntry>>,
    next_id: AtomicU64,
    runtime_event_sink: Mutex<Option<Arc<dyn tact::RuntimeEventSink>>>,
    #[cfg(any(test, feature = "test-support"))]
    legacy_tx: Mutex<Option<UnboundedSender<AgentUpdate>>>,
}

impl UiResponder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Routes prompt wake-ups through the protocol event stream when a Runtime
    /// host is attached. The snapshot remains the authoritative UI state.
    pub fn set_runtime_event_sink(&self, sink: Arc<dyn tact::RuntimeEventSink>) {
        *self.inner.runtime_event_sink.lock_recover() = Some(sink);
    }

    #[cfg(any(test, feature = "test-support"))]
    pub fn set_legacy_sender(&self, tx: UnboundedSender<AgentUpdate>) {
        *self.inner.legacy_tx.lock_recover() = Some(tx);
    }

    /// Register a single-select request and return its id plus the waiter.
    ///
    /// The request is visible through [`Self::snapshot`] immediately, before
    /// any `AgentUpdate` hint is sent. This ordering is what lets the TUI
    /// recover from a lost hint: it can pull the authoritative snapshot on a
    /// later tick.
    pub fn register_select(
        &self,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
    ) -> (u64, oneshot::Receiver<tact_protocol::InteractionResponse>) {
        self.register(PendingUiRequestKind::Select, prompt, options, log_confirm)
    }

    /// Register a multi-select request and return its id plus the waiter.
    pub fn register_multi(
        &self,
        prompt: String,
        options: Vec<String>,
    ) -> (u64, oneshot::Receiver<tact_protocol::InteractionResponse>) {
        self.register(PendingUiRequestKind::MultiSelect, prompt, options, false)
    }

    /// Snapshot of pending requests, ordered by request id (oldest first).
    pub fn snapshot(&self) -> Vec<PendingUiRequest> {
        let mut requests: Vec<_> = self
            .inner
            .pending
            .lock_recover()
            .values()
            .map(|entry| entry.request.clone())
            .collect();
        requests.sort_by_key(|request| request.request_id);
        requests
    }

    /// Deliver a response to the waiter for `response.request_id()`.
    ///
    /// Returns `false` when the id is unknown (already answered, withdrawn, or
    /// stale). Callers should treat that as a no-op and log it; it must not
    /// affect any other pending request.
    pub fn respond(&self, response: tact_protocol::InteractionResponse) -> bool {
        let request_id = interaction_request_id(&response);
        let Ok(request_id) = request_id.parse::<u64>() else {
            return false;
        };
        let entry = self.inner.pending.lock_recover().remove(&request_id);
        match entry {
            Some(entry) => {
                let _ = entry.tx.send(response);
                true
            }
            None => false,
        }
    }

    /// Remove a pending request without answering it. The waiter observes a
    /// closed channel, which callers map to cancellation/denial.
    pub fn withdraw(&self, request_id: u64) -> bool {
        self.inner
            .pending
            .lock_recover()
            .remove(&request_id)
            .is_some()
    }

    /// Ask the user to pick one option.
    ///
    /// Returns `Ok(Some(index))` on a selection, `Ok(None)` when the user
    /// cancelled, or `Err` when the UI closed before answering or answered with
    /// the wrong response kind.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn request_select(
        &self,
        ui_tx: &UnboundedSender<AgentUpdate>,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
    ) -> Result<Option<usize>, UiRequestError> {
        self.set_legacy_sender(ui_tx.clone());
        self.request_select_inner(prompt, options, log_confirm)
            .await
    }

    /// Ask through the Runtime interaction event stream, without requiring a
    /// View channel owned by the Agent.
    pub async fn request_select_runtime(
        &self,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
    ) -> Result<Option<usize>, UiRequestError> {
        self.request_select_inner(prompt, options, log_confirm)
            .await
    }

    async fn request_select_inner(
        &self,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
    ) -> Result<Option<usize>, UiRequestError> {
        let (request_id, rx) = self.register_select(prompt.clone(), options.clone(), log_confirm);
        let _guard = PendingRequestGuard {
            responder: self.clone(),
            request_id,
        };
        if !self.send_request_hint(AgentUpdate::RequestSelect {
            request_id,
            prompt,
            options: options.clone(),
            log_confirm,
        }) {
            // UI already gone: drop the pending waiter so the receiver resolves
            // immediately instead of hanging until `shutdown`.
            self.withdraw(request_id);
            return Err(UiRequestError::Closed);
        }
        match rx.await {
            Ok(response) => select_index(response, &options),
            Err(_) => Err(UiRequestError::Closed),
        }
    }

    /// Ask the user to pick zero or more options.
    ///
    /// Returns `Ok(Some(indices))` on confirm (possibly empty), `Ok(None)` when
    /// cancelled, or `Err` when the UI closed before answering or answered with
    /// the wrong response kind.
    #[cfg(any(test, feature = "test-support"))]
    pub async fn request_multi(
        &self,
        ui_tx: &UnboundedSender<AgentUpdate>,
        prompt: String,
        options: Vec<String>,
    ) -> Result<Option<Vec<usize>>, UiRequestError> {
        self.set_legacy_sender(ui_tx.clone());
        self.request_multi_inner(prompt, options).await
    }

    pub async fn request_multi_runtime(
        &self,
        prompt: String,
        options: Vec<String>,
    ) -> Result<Option<Vec<usize>>, UiRequestError> {
        self.request_multi_inner(prompt, options).await
    }

    async fn request_multi_inner(
        &self,
        prompt: String,
        options: Vec<String>,
    ) -> Result<Option<Vec<usize>>, UiRequestError> {
        let (request_id, rx) = self.register_multi(prompt.clone(), options.clone());
        let _guard = PendingRequestGuard {
            responder: self.clone(),
            request_id,
        };
        if !self.send_request_hint(AgentUpdate::RequestMultiSelect {
            request_id,
            prompt,
            options: options.clone(),
        }) {
            self.withdraw(request_id);
            return Err(UiRequestError::Closed);
        }
        match rx.await {
            Ok(response) => multi_index(response, &options),
            Err(_) => Err(UiRequestError::Closed),
        }
    }

    /// Answer a pending select by option index.
    ///
    /// Test-support helper: production Views produce a protocol
    /// [`tact_protocol::InteractionResponse`] directly, so this exists only so
    /// integration tests can keep expressing a choice as an index.
    #[cfg(any(test, feature = "test-support"))]
    pub fn respond_by_index(&self, request_id: u64, choice: Option<usize>) -> bool {
        let Some(options) = self.pending_options(request_id) else {
            return false;
        };
        let request_id = tact_protocol::RequestId::from(request_id.to_string());
        let response = match choice.and_then(|index| options.get(index).cloned()) {
            Some(value) => tact_protocol::InteractionResponse::Selected {
                request_id,
                values: vec![value],
            },
            None => tact_protocol::InteractionResponse::Cancelled { request_id },
        };
        self.respond(response)
    }

    /// Answer a pending multi-select by option indices.
    #[cfg(any(test, feature = "test-support"))]
    pub fn respond_multi_by_index(&self, request_id: u64, choices: Option<Vec<usize>>) -> bool {
        let Some(options) = self.pending_options(request_id) else {
            return false;
        };
        let request_id = tact_protocol::RequestId::from(request_id.to_string());
        let response = match choices {
            Some(indices) => tact_protocol::InteractionResponse::Selected {
                request_id,
                values: indices
                    .into_iter()
                    .filter_map(|index| options.get(index).cloned())
                    .collect(),
            },
            None => tact_protocol::InteractionResponse::Cancelled { request_id },
        };
        self.respond(response)
    }

    #[cfg(any(test, feature = "test-support"))]
    fn pending_options(&self, request_id: u64) -> Option<Vec<String>> {
        self.inner
            .pending
            .lock_recover()
            .get(&request_id)
            .map(|entry| entry.request.options.clone())
    }

    fn send_request_hint(&self, update: AgentUpdate) -> bool {
        let Some(sink) = self.inner.runtime_event_sink.lock_recover().clone() else {
            #[cfg(any(test, feature = "test-support"))]
            return self
                .inner
                .legacy_tx
                .lock_recover()
                .clone()
                .is_some_and(|tx| tx.send(update).is_ok());
            #[cfg(not(any(test, feature = "test-support")))]
            return false;
        };
        let request = match update {
            AgentUpdate::RequestSelect {
                request_id,
                prompt,
                options,
                log_confirm,
            } => tact_protocol::InteractionRequest::Select {
                request_id: tact_protocol::RequestId::from(request_id.to_string()),
                prompt,
                options,
                log_confirm,
            },
            AgentUpdate::RequestMultiSelect {
                request_id,
                prompt,
                options,
            } => tact_protocol::InteractionRequest::MultiSelect {
                request_id: tact_protocol::RequestId::from(request_id.to_string()),
                prompt,
                options,
            },
            _ => return false,
        };
        sink.emit(tact_protocol::RuntimeEvent::InteractionRequested { request })
            .is_ok()
    }

    /// Drop every pending waiter, unblocking any in-flight `request_*` call
    /// with `Err(UiRequestError::Closed)`. Invoked by the driver when the UI is
    /// gone so the agent task can finish instead of deadlocking on an answer
    /// that never arrives.
    pub fn shutdown(&self) {
        self.inner.pending.lock_recover().clear();
    }

    fn register(
        &self,
        kind: PendingUiRequestKind,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
    ) -> (u64, oneshot::Receiver<tact_protocol::InteractionResponse>) {
        let request_id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let request = PendingUiRequest {
            request_id,
            prompt,
            options,
            kind,
            log_confirm,
        };
        self.inner
            .pending
            .lock_recover()
            .insert(request_id, PendingEntry { request, tx });
        (request_id, rx)
    }
}

fn interaction_request_id(response: &tact_protocol::InteractionResponse) -> &str {
    use tact_protocol::InteractionResponse;
    match response {
        InteractionResponse::Approved { request_id }
        | InteractionResponse::Rejected { request_id }
        | InteractionResponse::Selected { request_id, .. }
        | InteractionResponse::Text { request_id, .. }
        | InteractionResponse::Cancelled { request_id } => request_id.as_str(),
    }
}

/// Maps a client-neutral single-select response back to an option index.
fn select_index(
    response: tact_protocol::InteractionResponse,
    options: &[String],
) -> Result<Option<usize>, UiRequestError> {
    use tact_protocol::InteractionResponse;
    match response {
        InteractionResponse::Selected { values, .. } => Ok(values
            .first()
            .and_then(|value| options.iter().position(|option| option == value))),
        InteractionResponse::Approved { .. } => Ok(options
            .iter()
            .position(|option| option.eq_ignore_ascii_case("allow"))),
        InteractionResponse::Rejected { .. } => Ok(options
            .iter()
            .position(|option| option.eq_ignore_ascii_case("deny"))),
        InteractionResponse::Cancelled { .. } => Ok(None),
        InteractionResponse::Text { .. } => Err(UiRequestError::Mismatched),
    }
}

/// Maps a client-neutral multi-select response back to option indices.
fn multi_index(
    response: tact_protocol::InteractionResponse,
    options: &[String],
) -> Result<Option<Vec<usize>>, UiRequestError> {
    use tact_protocol::InteractionResponse;
    match response {
        InteractionResponse::Selected { values, .. } => {
            let indices = values
                .iter()
                .map(|value| options.iter().position(|option| option == value))
                .collect::<Option<Vec<_>>>();
            indices.map(Some).ok_or(UiRequestError::Mismatched)
        }
        InteractionResponse::Cancelled { .. } => Ok(None),
        _ => Err(UiRequestError::Mismatched),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::RuntimeEvent;
    use tokio::sync::mpsc::unbounded_channel;

    #[derive(Default)]
    struct CapturedEvents(std::sync::Mutex<Vec<RuntimeEvent>>);

    impl tact::RuntimeEventSink for CapturedEvents {
        fn emit(&self, event: RuntimeEvent) -> Result<(), tact::KernelError> {
            self.0.lock().unwrap().push(event);
            Ok(())
        }
    }

    #[tokio::test]
    async fn multi_select_routes_response_by_request_id() {
        let responder = UiResponder::new();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();

        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_multi(&tx, "Pick toppings".into(), vec!["a".into(), "b".into()])
                    .await
            }
        });

        // The request carries a request_id, not a sender.
        let AgentUpdate::RequestMultiSelect {
            request_id,
            prompt,
            options,
        } = rx.recv().await.unwrap()
        else {
            panic!("expected RequestMultiSelect");
        };
        assert_eq!(prompt, "Pick toppings");
        assert_eq!(options, vec!["a", "b"]);

        responder.respond_multi_by_index(request_id, Some(vec![0]));
        assert_eq!(handle.await.unwrap(), Ok(Some(vec![0])));
    }

    #[tokio::test]
    async fn select_returns_none_on_cancel() {
        let responder = UiResponder::new();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();

        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(&tx, "Allow?".into(), vec!["Yes".into()], false)
                    .await
            }
        });

        let AgentUpdate::RequestSelect { request_id, .. } = rx.recv().await.unwrap() else {
            panic!("expected RequestSelect");
        };
        responder.respond_by_index(request_id, None);
        assert_eq!(handle.await.unwrap(), Ok(None));
    }

    #[tokio::test]
    async fn shutdown_unblocks_waiters_with_error() {
        let responder = UiResponder::new();
        let (tx, _rx) = unbounded_channel::<AgentUpdate>();

        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(&tx, "stale".into(), vec!["x".into()], false)
                    .await
            }
        });

        // No response is ever sent; shutdown must still unblock the waiter.
        tokio::task::yield_now().await;
        responder.shutdown();
        assert_eq!(handle.await.unwrap(), Err(UiRequestError::Closed));
    }

    #[tokio::test]
    async fn closed_channel_fails_fast_without_hanging() {
        let responder = UiResponder::new();
        let (tx, rx) = unbounded_channel::<AgentUpdate>();
        drop(rx); // close the UI channel

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(100),
            responder.request_select(&tx, "q".into(), vec!["a".into()], false),
        )
        .await;

        assert_eq!(result, Ok(Err(UiRequestError::Closed)));
    }

    #[tokio::test]
    async fn mismatched_response_is_an_error_not_cancel() {
        let responder = UiResponder::new();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();

        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(&tx, "Allow?".into(), vec!["Yes".into()], false)
                    .await
            }
        });

        let AgentUpdate::RequestSelect { request_id, .. } = rx.recv().await.unwrap() else {
            panic!("expected RequestSelect");
        };
        // Wrong variant for a single-select request, at the protocol level.
        responder.respond(tact_protocol::InteractionResponse::Text {
            request_id: tact_protocol::RequestId::from(request_id.to_string()),
            value: "unexpected".into(),
        });
        assert_eq!(handle.await.unwrap(), Err(UiRequestError::Mismatched));
    }

    #[tokio::test]
    async fn request_ids_are_globally_unique_across_clones() {
        let a = UiResponder::new();
        let b = a.clone();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();

        let h1 = tokio::spawn({
            let a = a.clone();
            let tx = tx.clone();
            async move {
                a.request_select(&tx, "one".into(), vec!["x".into()], false)
                    .await
            }
        });
        let h2 = tokio::spawn({
            let b = b.clone();
            let tx = tx.clone();
            async move {
                b.request_select(&tx, "two".into(), vec!["y".into()], false)
                    .await
            }
        });

        let id1 = match rx.recv().await.unwrap() {
            AgentUpdate::RequestSelect { request_id, .. } => request_id,
            other => panic!("unexpected {other:?}"),
        };
        let id2 = match rx.recv().await.unwrap() {
            AgentUpdate::RequestSelect { request_id, .. } => request_id,
            other => panic!("unexpected {other:?}"),
        };
        assert_ne!(id1, id2);

        // Answer both so the spawned tasks finish.
        a.respond_by_index(id1, None);
        a.respond_by_index(id2, None);
        h1.await.unwrap().unwrap();
        h2.await.unwrap().unwrap();
    }

    #[test]
    fn snapshot_orders_pending_requests_and_clears_on_respond() {
        let responder = UiResponder::new();
        let (id1, _rx1) =
            responder.register_select("first".into(), vec!["a".into(), "b".into()], false);
        let (id2, _rx2) = responder.register_multi("second".into(), vec!["x".into()]);

        let snapshot = responder.snapshot();
        assert_eq!(
            snapshot.iter().map(|r| r.request_id).collect::<Vec<_>>(),
            vec![id1, id2]
        );
        assert_eq!(snapshot[0].prompt, "first");
        assert_eq!(snapshot[0].kind, PendingUiRequestKind::Select);
        assert_eq!(snapshot[1].kind, PendingUiRequestKind::MultiSelect);

        assert!(
            responder.respond(tact_protocol::InteractionResponse::Selected {
                request_id: tact_protocol::RequestId::from(id1.to_string()),
                values: vec!["b".into()],
            })
        );
        assert_eq!(responder.snapshot().len(), 1);

        // A second response for the same id is stale and must not affect id2.
        assert!(
            !responder.respond(tact_protocol::InteractionResponse::Selected {
                request_id: tact_protocol::RequestId::from(id1.to_string()),
                values: vec!["a".into()],
            })
        );
        assert_eq!(responder.snapshot()[0].request_id, id2);
    }

    #[test]
    fn protocol_selection_response_routes_by_request_id_and_value() {
        let responder = UiResponder::new();
        let (request_id, mut receiver) = responder.register_select(
            "Permission".into(),
            vec!["Allow".into(), "Deny".into()],
            false,
        );

        assert!(
            responder.respond(tact_protocol::InteractionResponse::Selected {
                request_id: tact_protocol::RequestId::from(request_id.to_string()),
                values: vec!["Deny".into()],
            })
        );
        assert!(matches!(
            receiver.try_recv(),
            Ok(tact_protocol::InteractionResponse::Selected { request_id: id, values })
                if id.as_str() == request_id.to_string() && values == vec!["Deny".to_string()]
        ));
    }

    #[tokio::test]
    async fn withdraw_removes_snapshot_and_unblocks_waiter() {
        let responder = UiResponder::new();
        let (request_id, rx) = responder.register_select("gone".into(), vec!["x".into()], false);
        assert_eq!(responder.snapshot().len(), 1);

        assert!(responder.withdraw(request_id));
        assert!(responder.snapshot().is_empty());
        assert!(!responder.withdraw(request_id));
        assert!(rx.await.is_err(), "withdraw drops the sender -> Closed");
    }

    #[tokio::test]
    async fn request_is_visible_in_snapshot_before_hint_is_received() {
        let responder = UiResponder::new();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();
        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(&tx, "Allow?".into(), vec!["Yes".into()], false)
                    .await
            }
        });

        let AgentUpdate::RequestSelect { request_id, .. } = rx.recv().await.unwrap() else {
            panic!("expected RequestSelect");
        };
        // The hint and the snapshot are both live; reconcile can use either.
        assert_eq!(responder.snapshot()[0].request_id, request_id);
        responder.respond_by_index(request_id, Some(0));
        assert_eq!(handle.await.unwrap().unwrap(), Some(0));
    }
    #[tokio::test]
    async fn aborting_request_task_withdraws_pending_entry() {
        let responder = UiResponder::new();
        let (tx, _rx) = unbounded_channel::<AgentUpdate>();
        let task = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(&tx, "gone".into(), vec!["x".into()], false)
                    .await
            }
        });

        // Let the task register and publish the request.
        for _ in 0..10 {
            if !responder.snapshot().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
        assert_eq!(responder.snapshot().len(), 1);

        task.abort();
        let _ = task.await;
        assert!(
            responder.snapshot().is_empty(),
            "aborting the waiter must withdraw its pending entry"
        );
    }

    #[tokio::test]
    async fn runtime_event_sink_replaces_legacy_select_hint() {
        let responder = UiResponder::new();
        let events = Arc::new(CapturedEvents::default());
        responder.set_runtime_event_sink(events.clone());
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();
        let task = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(&tx, "Choose".into(), vec!["a".into(), "b".into()], false)
                    .await
            }
        });

        tokio::time::timeout(std::time::Duration::from_millis(200), async {
            while responder.snapshot().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("select request did not register");
        let request_id = responder.snapshot()[0].request_id;
        assert!(matches!(
            events.0.lock().unwrap().as_slice(),
            [RuntimeEvent::InteractionRequested { request: tact_protocol::InteractionRequest::Select { prompt, .. } }]
                if prompt == "Choose"
        ));
        assert!(rx.try_recv().is_err());
        responder.respond_by_index(request_id, Some(1));
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_millis(200), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Some(1)
        );
    }

    #[tokio::test]
    async fn runtime_select_request_needs_no_view_channel() {
        let responder = UiResponder::new();
        let events = Arc::new(CapturedEvents::default());
        responder.set_runtime_event_sink(events.clone());
        let task = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select_runtime(
                        "Select through Runtime".into(),
                        vec!["yes".into(), "no".into()],
                        false,
                    )
                    .await
            }
        });

        tokio::time::timeout(std::time::Duration::from_millis(200), async {
            while responder.snapshot().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request did not register");
        let pending = responder.snapshot()[0].clone();
        assert!(events.0.lock().unwrap().iter().any(|event| matches!(
            event,
            RuntimeEvent::InteractionRequested {
                request: tact_protocol::InteractionRequest::Select { prompt, .. }
            } if prompt == "Select through Runtime"
        )));
        responder.respond_by_index(pending.request_id, Some(0));
        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_millis(200), task)
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Some(0)
        );
    }
}
