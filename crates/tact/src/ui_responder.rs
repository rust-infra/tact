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
//!    backwards-compatible [`UiResponder::handle_response`] wrapper) from the
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
use tact_protocol::{
    AgentUpdate, UiAgentRef, UiRequestOrigin, UiRequestPurpose, UiResponse, UpdateDispatch,
};
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
    /// The request came from a `spawn_subagent` child, not the main agent. The
    /// popup titles itself accordingly; the prompt itself stays clean.
    pub from_subagent: bool,
    pub purpose: UiRequestPurpose,
    pub origin: UiRequestOrigin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingUiRequestKind {
    Select,
    MultiSelect,
}

struct PendingEntry {
    request: PendingUiRequest,
    tx: oneshot::Sender<UiResponse>,
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
}

impl UiResponder {
    pub fn new() -> Self {
        Self::default()
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
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> (u64, oneshot::Receiver<UiResponse>) {
        self.register(
            PendingUiRequestKind::Select,
            prompt,
            options,
            log_confirm,
            purpose,
            origin,
        )
    }

    /// Register a multi-select request and return its id plus the waiter.
    pub fn register_multi(
        &self,
        prompt: String,
        options: Vec<String>,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> (u64, oneshot::Receiver<UiResponse>) {
        self.register(
            PendingUiRequestKind::MultiSelect,
            prompt,
            options,
            false,
            purpose,
            origin,
        )
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
    pub fn respond(&self, response: UiResponse) -> bool {
        let request_id = response.request_id();
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
    pub async fn request_select(
        &self,
        ui_tx: &impl UpdateDispatch,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> Result<Option<usize>, UiRequestError> {
        let from_subagent = matches!(origin.agent, UiAgentRef::Subagent { .. });
        let (request_id, rx) = self.register_select(
            prompt.clone(),
            options.clone(),
            log_confirm,
            purpose,
            origin.clone(),
        );
        let _guard = PendingRequestGuard {
            responder: self.clone(),
            request_id,
        };
        if ui_tx
            .send_update(AgentUpdate::RequestSelect {
                request_id,
                prompt,
                options,
                log_confirm,
                from_subagent,
                purpose,
                origin,
            })
            .is_err()
        {
            // UI already gone: drop the pending waiter so the receiver resolves
            // immediately instead of hanging until `shutdown`.
            self.withdraw(request_id);
            return Err(UiRequestError::Closed);
        }
        match rx.await {
            Ok(UiResponse::Select { choice, .. }) => Ok(choice),
            Ok(_) => Err(UiRequestError::Mismatched),
            Err(_) => Err(UiRequestError::Closed),
        }
    }

    /// Ask the user to pick zero or more options.
    ///
    /// Returns `Ok(Some(indices))` on confirm (possibly empty), `Ok(None)` when
    /// cancelled, or `Err` when the UI closed before answering or answered with
    /// the wrong response kind.
    pub async fn request_multi(
        &self,
        ui_tx: &impl UpdateDispatch,
        prompt: String,
        options: Vec<String>,
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> Result<Option<Vec<usize>>, UiRequestError> {
        let from_subagent = matches!(origin.agent, UiAgentRef::Subagent { .. });
        let (request_id, rx) =
            self.register_multi(prompt.clone(), options.clone(), purpose, origin.clone());
        let _guard = PendingRequestGuard {
            responder: self.clone(),
            request_id,
        };
        if ui_tx
            .send_update(AgentUpdate::RequestMultiSelect {
                request_id,
                prompt,
                options,
                from_subagent,
                purpose,
                origin,
            })
            .is_err()
        {
            self.withdraw(request_id);
            return Err(UiRequestError::Closed);
        }
        match rx.await {
            Ok(UiResponse::MultiSelect { choices, .. }) => Ok(choices),
            Ok(_) => Err(UiRequestError::Mismatched),
            Err(_) => Err(UiRequestError::Closed),
        }
    }

    /// Backwards-compatible wrapper used by tests and other in-process callers.
    pub fn handle_response(&self, response: UiResponse) {
        let _ = self.respond(response);
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
        purpose: UiRequestPurpose,
        origin: UiRequestOrigin,
    ) -> (u64, oneshot::Receiver<UiResponse>) {
        let request_id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let from_subagent = matches!(origin.agent, UiAgentRef::Subagent { .. });
        let request = PendingUiRequest {
            request_id,
            prompt,
            options,
            kind,
            log_confirm,
            from_subagent,
            purpose,
            origin,
        };
        self.inner
            .pending
            .lock_recover()
            .insert(request_id, PendingEntry { request, tx });
        (request_id, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::UserCommand;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn pending_gate_preserves_purpose_and_origin() {
        let responder = UiResponder::new();
        let (request_id, _rx) = responder.register_select(
            "Pick one".into(),
            vec!["a".into()],
            false,
            UiRequestPurpose::Question,
            UiRequestOrigin {
                agent: UiAgentRef::Main,
                tool_id: Some("call-1".into()),
            },
        );

        let pending = responder.snapshot();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].request_id, request_id);
        assert_eq!(pending[0].purpose, UiRequestPurpose::Question);
        assert!(matches!(pending[0].origin.agent, UiAgentRef::Main));
        assert_eq!(pending[0].origin.tool_id.as_deref(), Some("call-1"));
    }

    #[test]
    fn subagent_gate_names_the_child_and_tool() {
        let responder = UiResponder::new();
        responder.register_select(
            "Allow write?".into(),
            vec!["allow".into()],
            false,
            UiRequestPurpose::Permission,
            UiRequestOrigin::subagent("child-7", Some("call-9".into())),
        );

        let pending = responder.snapshot();
        assert!(pending[0].from_subagent);
        assert_eq!(pending[0].purpose, UiRequestPurpose::Permission);
        assert_eq!(
            pending[0].origin.agent,
            UiAgentRef::Subagent {
                child_id: "child-7".into()
            }
        );
        assert_eq!(pending[0].origin.tool_id.as_deref(), Some("call-9"));
    }

    #[test]
    fn stale_gate_response_is_a_noop() {
        let responder = UiResponder::new();
        responder.register_select(
            "still pending".into(),
            vec!["a".into()],
            false,
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
        );

        assert!(!responder.respond(UiResponse::Select {
            request_id: 999,
            choice: Some(0),
        }));
        assert_eq!(responder.snapshot().len(), 1);
        assert_eq!(responder.snapshot()[0].prompt, "still pending");
    }

    #[tokio::test]
    async fn multi_select_routes_response_by_request_id() {
        let responder = UiResponder::new();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();

        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_multi(
                        &tx,
                        "Pick toppings".into(),
                        vec!["a".into(), "b".into()],
                        UiRequestPurpose::Question,
                        UiRequestOrigin::main(),
                    )
                    .await
            }
        });

        // The request carries a request_id, not a sender.
        let AgentUpdate::RequestMultiSelect {
            request_id,
            prompt,
            options,
            from_subagent,
            purpose,
            ..
        } = rx.recv().await.unwrap()
        else {
            panic!("expected RequestMultiSelect");
        };
        assert_eq!(prompt, "Pick toppings");
        assert_eq!(options, vec!["a", "b"]);
        assert!(!from_subagent, "a main-agent ask_user is not a child's");
        assert_eq!(purpose, UiRequestPurpose::Question);

        responder.handle_response(UiResponse::MultiSelect {
            request_id,
            choices: Some(vec![0]),
        });
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
                    .request_select(
                        &tx,
                        "Allow?".into(),
                        vec!["Yes".into()],
                        false,
                        UiRequestPurpose::Permission,
                        UiRequestOrigin::main(),
                    )
                    .await
            }
        });

        let AgentUpdate::RequestSelect { request_id, .. } = rx.recv().await.unwrap() else {
            panic!("expected RequestSelect");
        };
        responder.handle_response(UiResponse::Select {
            request_id,
            choice: None,
        });
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
                    .request_select(
                        &tx,
                        "stale".into(),
                        vec!["x".into()],
                        false,
                        UiRequestPurpose::Question,
                        UiRequestOrigin::main(),
                    )
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
            responder.request_select(
                &tx,
                "q".into(),
                vec!["a".into()],
                false,
                UiRequestPurpose::Question,
                UiRequestOrigin::main(),
            ),
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
                    .request_select(
                        &tx,
                        "Allow?".into(),
                        vec!["Yes".into()],
                        false,
                        UiRequestPurpose::Permission,
                        UiRequestOrigin::main(),
                    )
                    .await
            }
        });

        let AgentUpdate::RequestSelect { request_id, .. } = rx.recv().await.unwrap() else {
            panic!("expected RequestSelect");
        };
        // Wrong variant for a single-select request.
        responder.handle_response(UiResponse::MultiSelect {
            request_id,
            choices: Some(vec![0]),
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
                a.request_select(
                    &tx,
                    "one".into(),
                    vec!["x".into()],
                    false,
                    UiRequestPurpose::Question,
                    UiRequestOrigin::main(),
                )
                .await
            }
        });
        let h2 = tokio::spawn({
            let b = b.clone();
            let tx = tx.clone();
            async move {
                b.request_select(
                    &tx,
                    "two".into(),
                    vec!["y".into()],
                    false,
                    UiRequestPurpose::Question,
                    UiRequestOrigin::main(),
                )
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
        a.handle_response(UiResponse::Select {
            request_id: id1,
            choice: None,
        });
        a.handle_response(UiResponse::Select {
            request_id: id2,
            choice: None,
        });
        h1.await.unwrap().unwrap();
        h2.await.unwrap().unwrap();
    }

    #[test]
    fn ui_response_request_id_accessor() {
        let resp = UserCommand::UiResponse(UiResponse::Select {
            request_id: 7,
            choice: Some(1),
        });
        let UserCommand::UiResponse(resp) = resp else {
            panic!("expected UiResponse");
        };
        assert_eq!(resp.request_id(), 7);
    }
    #[test]
    fn snapshot_orders_pending_requests_and_clears_on_respond() {
        let responder = UiResponder::new();
        let (id1, _rx1) = responder.register_select(
            "first".into(),
            vec!["a".into(), "b".into()],
            false,
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
        );
        let (id2, _rx2) = responder.register_multi(
            "second".into(),
            vec!["x".into()],
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
        );

        let snapshot = responder.snapshot();
        assert_eq!(
            snapshot.iter().map(|r| r.request_id).collect::<Vec<_>>(),
            vec![id1, id2]
        );
        assert_eq!(snapshot[0].prompt, "first");
        assert_eq!(snapshot[0].kind, PendingUiRequestKind::Select);
        assert_eq!(snapshot[1].kind, PendingUiRequestKind::MultiSelect);

        assert!(responder.respond(UiResponse::Select {
            request_id: id1,
            choice: Some(1),
        }));
        assert_eq!(responder.snapshot().len(), 1);

        // A second response for the same id is stale and must not affect id2.
        assert!(!responder.respond(UiResponse::Select {
            request_id: id1,
            choice: Some(0),
        }));
        assert_eq!(responder.snapshot()[0].request_id, id2);
    }

    #[tokio::test]
    async fn withdraw_removes_snapshot_and_unblocks_waiter() {
        let responder = UiResponder::new();
        let (request_id, rx) = responder.register_select(
            "gone".into(),
            vec!["x".into()],
            false,
            UiRequestPurpose::Question,
            UiRequestOrigin::main(),
        );
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
                    .request_select(
                        &tx,
                        "Allow?".into(),
                        vec!["Yes".into()],
                        false,
                        UiRequestPurpose::Permission,
                        UiRequestOrigin::main(),
                    )
                    .await
            }
        });

        let AgentUpdate::RequestSelect { request_id, .. } = rx.recv().await.unwrap() else {
            panic!("expected RequestSelect");
        };
        // The hint and the snapshot are both live; reconcile can use either.
        assert_eq!(responder.snapshot()[0].request_id, request_id);
        responder.respond(UiResponse::Select {
            request_id,
            choice: Some(0),
        });
        assert_eq!(handle.await.unwrap().unwrap(), Some(0));
    }
    /// The popup titles itself from the *snapshot*, so a subagent's origin has
    /// to be recorded there too — the wire hint is not the only reader.
    #[tokio::test]
    async fn a_subagent_request_carries_its_origin_into_the_snapshot() {
        let responder = UiResponder::new();
        let (tx, mut rx) = unbounded_channel::<AgentUpdate>();
        let handle = tokio::spawn({
            let responder = responder.clone();
            async move {
                responder
                    .request_select(
                        &tx,
                        "Allow write_file on a.rs?".into(),
                        vec!["Allow once".into()],
                        false,
                        UiRequestPurpose::Permission,
                        UiRequestOrigin::subagent("child-1", Some("tool-1".into())),
                    )
                    .await
            }
        });

        let AgentUpdate::RequestSelect {
            request_id,
            from_subagent,
            ..
        } = rx.recv().await.unwrap()
        else {
            panic!("expected RequestSelect");
        };
        assert!(from_subagent, "the hint has to say who is asking");
        assert!(
            responder.snapshot()[0].from_subagent,
            "the snapshot is what the popup is reconciled from"
        );
        responder.respond(UiResponse::Select {
            request_id,
            choice: Some(0),
        });
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
                    .request_select(
                        &tx,
                        "gone".into(),
                        vec!["x".into()],
                        false,
                        UiRequestPurpose::Question,
                        UiRequestOrigin::main(),
                    )
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
}
