//! Shared request/reply registry for UI prompts (`RequestSelect` /
//! `RequestMultiSelect`).
//!
//! The agent runtime no longer embeds a `tokio::sync::oneshot::Sender` inside
//! [`AgentUpdate`] (that made the protocol enum impossible to serialize and
//! coupled the transport to a single in-process responder). Instead a tool or
//! the permission gate asks [`UiResponder`] for a selection, which:
//!
//! 1. allocates a globally-unique `request_id`,
//! 2. records a pending oneshot for that id,
//! 3. emits a pure-data `RequestSelect` / `RequestMultiSelect` carrying the id,
//! 4. awaits the answer, which arrives via [`UiResponder::handle_response`] from
//!    the command driver (after the TUI sends `UserCommand::UiResponse`).
//!
//! The id allocator and pending map live behind an `Arc`, so the parent agent
//! and every subagent (which clone the same [`ToolContext`]) share one
//! namespace: a response can always be routed to the exact waiter, even when a
//! subagent forwarded its request through the parent's UI channel.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use tact_protocol::{AgentUpdate, UiResponse};
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Shared registry routing UI responses back to the waiting caller.
///
/// Cheap to clone: every clone shares the same inner state.
#[derive(Clone, Default)]
pub struct UiResponder {
    inner: Arc<UiResponderInner>,
}

#[derive(Default)]
struct UiResponderInner {
    pending: Mutex<HashMap<u64, oneshot::Sender<UiResponse>>>,
    next_id: AtomicU64,
}

impl UiResponder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Ask the user to pick one option.
    ///
    /// Returns `Ok(Some(index))` on a selection, `Ok(None)` when the user
    /// cancelled, or `Err` when the UI closed before answering (the shared
    /// responder was shut down).
    pub async fn request_select(
        &self,
        ui_tx: &UnboundedSender<AgentUpdate>,
        prompt: String,
        options: Vec<String>,
        log_confirm: bool,
    ) -> Result<Option<usize>, oneshot::error::RecvError> {
        let (request_id, rx) = self.register();
        let _ = ui_tx.send(AgentUpdate::RequestSelect {
            request_id,
            prompt,
            options,
            log_confirm,
        });
        match rx.await {
            Ok(UiResponse::Select { choice, .. }) => Ok(choice),
            Err(err) => Err(err),
            Ok(_) => Ok(None),
        }
    }

    /// Ask the user to pick zero or more options.
    ///
    /// Returns `Ok(Some(indices))` on confirm (possibly empty), `Ok(None)` when
    /// cancelled, or `Err` when the UI closed before answering.
    pub async fn request_multi(
        &self,
        ui_tx: &UnboundedSender<AgentUpdate>,
        prompt: String,
        options: Vec<String>,
    ) -> Result<Option<Vec<usize>>, oneshot::error::RecvError> {
        let (request_id, rx) = self.register();
        let _ = ui_tx.send(AgentUpdate::RequestMultiSelect {
            request_id,
            prompt,
            options,
        });
        match rx.await {
            Ok(UiResponse::MultiSelect { choices, .. }) => Ok(choices),
            Err(err) => Err(err),
            Ok(_) => Ok(None),
        }
    }

    /// Route a TUI response to the waiting caller. Called by the command driver
    /// when it receives [`UserCommand::UiResponse`]; no-op if the request id is
    /// unknown (e.g. already answered or cancelled).
    pub fn handle_response(&self, response: UiResponse) {
        let request_id = response.request_id();
        if let Some(tx) = self
            .inner
            .pending
            .lock()
            .expect("ui responder lock poisoned")
            .remove(&request_id)
        {
            let _ = tx.send(response);
        }
    }

    /// Drop every pending waiter, unblocking any in-flight `request_*` call
    /// with `Err(RecvError)`. Invoked by the driver when the UI is gone so the
    /// agent task can finish instead of deadlocking on an answer that never
    /// arrives.
    pub fn shutdown(&self) {
        self.inner
            .pending
            .lock()
            .expect("ui responder lock poisoned")
            .clear();
    }

    fn register(&self) -> (u64, oneshot::Receiver<UiResponse>) {
        let request_id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.inner
            .pending
            .lock()
            .expect("ui responder lock poisoned")
            .insert(request_id, tx);
        (request_id, rx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::UserCommand;
    use tokio::sync::mpsc::unbounded_channel;

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
                    .request_select(&tx, "Allow?".into(), vec!["Yes".into()], false)
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
                    .request_select(&tx, "stale".into(), vec!["x".into()], false)
                    .await
            }
        });

        // No response is ever sent; shutdown must still unblock the waiter.
        tokio::task::yield_now().await;
        responder.shutdown();
        assert!(handle.await.unwrap().is_err());
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
}
