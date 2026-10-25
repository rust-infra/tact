//! Neutral tool-call context and execution ports.
//!
//! No domain manager, UI responder, provider client or SQLite handle belongs
//! here: a tool that needs one holds an `Arc` injected by its plugin.

use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;

/// Who issued a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolOrigin {
    Main,
    Subagent,
}

/// The workspace a tool call is scoped to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    root: PathBuf,
}

impl Workspace {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Cooperative cancellation for one run.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// The shared flag, for a legacy adapter that still exposes an
    /// `Arc<AtomicBool>` to old tools.
    pub fn shared_flag(&self) -> Arc<AtomicBool> {
        self.0.clone()
    }

    /// A token over a flag someone else already owns.
    ///
    /// A host that keeps its own cancel flag uses this so `is_cancelled()` sees
    /// a cancellation that happens *during* the call, not just one that was
    /// already set when the context was built.
    pub fn from_shared(flag: Arc<AtomicBool>) -> Self {
        Self(flag)
    }
}

/// How much of a tool's live/final output to redact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedactionPolicy {
    Off,
    Basic,
    Credential,
}

/// A fact a tool emits about its own progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEvent {
    Started { call_id: String, tool_name: String },
    Finished { call_id: String, content: String },
    Failed { call_id: String, error: String },
}

/// Where a tool's events go. The kernel owns fan-out; tools only emit facts.
pub trait EventSink: Send + Sync {
    fn emit(&self, event: ToolEvent);
}

/// A request for a human answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionRequest {
    Confirm {
        prompt: String,
    },
    Select {
        prompt: String,
        options: Vec<String>,
    },
    Input {
        prompt: String,
    },
}

/// A human's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionResponse {
    Approved,
    Rejected,
    Cancelled,
    Text(String),
    Selected(Vec<String>),
}

/// A request that is waiting for a human, as a view sees it.
///
/// Views reconcile from a snapshot rather than trusting one event: an event can
/// be missed (a view attaching late), and a waiter that was dropped must not
/// leave a ghost prompt on screen. The ID is what pairs an answer with its
/// waiter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingInteraction {
    pub request_id: u64,
    pub request: InteractionRequest,
    /// The request came from a `spawn_subagent` child rather than the main run.
    ///
    /// A view titles the popup from this; the prompt itself stays clean, because
    /// the model wrote it and the user should read what the model said.
    pub from_subagent: bool,
}

/// The client-neutral interaction port.
///
/// `request` is the only required method: a broker with no view attached simply
/// answers (or refuses) without ever showing anything. Snapshot and withdraw are
/// defaulted because a broker that only answers inline — a headless run, a test
/// fake — has no pending set to expose, and forcing it to invent one would make
/// every fake carry state it never reads.
#[async_trait]
pub trait InteractionBroker: Send + Sync {
    async fn request(&self, request: InteractionRequest) -> anyhow::Result<InteractionResponse>;

    /// The requests waiting for an answer, in registration order.
    ///
    /// A view calls this on every event and on a timer: the set is the truth,
    /// and a prompt that is no longer in it must be closed.
    fn pending(&self) -> Vec<PendingInteraction> {
        Vec::new()
    }

    /// Abandon one request, if it is still waiting.
    ///
    /// Returns whether anything was withdrawn. The waiter sees a cancellation
    /// rather than waiting forever, which is what makes a closed popup safe.
    fn withdraw(&self, _request_id: u64) -> bool {
        false
    }
}

/// Everything a tool call is allowed to know about its run.
#[derive(Clone)]
pub struct ToolCallContext {
    pub run_id: String,
    pub tool_use_id: String,
    pub session_id: Option<String>,
    /// Opaque binding for a bounded run; only Goal/Task extensions interpret it.
    pub active_goal_id: Option<String>,
    pub origin: ToolOrigin,
    pub workspace: Workspace,
    pub events: Arc<dyn EventSink>,
    pub interaction: Arc<dyn InteractionBroker>,
    pub cancel: CancelToken,
    pub redaction: RedactionPolicy,
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NullEvents;
    impl EventSink for NullEvents {
        fn emit(&self, _event: ToolEvent) {}
    }

    struct RejectAll;
    #[async_trait]
    impl InteractionBroker for RejectAll {
        async fn request(
            &self,
            _request: InteractionRequest,
        ) -> anyhow::Result<InteractionResponse> {
            Ok(InteractionResponse::Rejected)
        }
    }

    #[test]
    fn workspace_and_cancel_token_are_neutral() {
        let workspace = Workspace::new("/repo");
        assert_eq!(workspace.root(), Path::new("/repo"));
        let cancel = CancelToken::new();
        assert!(!cancel.is_cancelled());
        cancel.cancel();
        assert!(cancel.is_cancelled());
    }

    #[test]
    fn context_carries_only_run_scoped_handles() {
        let context = ToolCallContext {
            run_id: "run".into(),
            tool_use_id: "tool".into(),
            session_id: Some("session".into()),
            active_goal_id: None,
            origin: ToolOrigin::Subagent,
            workspace: Workspace::new("/repo"),
            events: Arc::new(NullEvents),
            interaction: Arc::new(RejectAll),
            cancel: CancelToken::new(),
            redaction: RedactionPolicy::Basic,
        };
        assert_eq!(context.origin, ToolOrigin::Subagent);
        assert_eq!(context.workspace.root(), Path::new("/repo"));
        assert_eq!(context.redaction, RedactionPolicy::Basic);
    }

    /// A broker with no view exposes no pending set, and that is not a failure.
    ///
    /// Headless runs and inline fakes answer without ever showing a prompt, so
    /// the defaults have to be the honest empty answer rather than an error the
    /// caller must special-case.
    #[test]
    fn a_broker_without_a_view_has_nothing_pending() {
        let broker = RejectAll;
        assert!(broker.pending().is_empty());
        assert!(
            !broker.withdraw(1),
            "there was nothing to withdraw, and saying so is the contract"
        );
    }

    /// A broker that does hold requests reports them, and withdrawing removes one.
    ///
    /// This is the shape a view depends on: the snapshot is the truth, so an
    /// entry that left it must not keep a popup on screen.
    #[tokio::test]
    async fn pending_requests_round_trip_through_the_snapshot() {
        struct Holding(std::sync::Mutex<Vec<PendingInteraction>>);

        #[async_trait]
        impl InteractionBroker for Holding {
            async fn request(
                &self,
                request: InteractionRequest,
            ) -> anyhow::Result<InteractionResponse> {
                self.0.lock().expect("lock").push(PendingInteraction {
                    request_id: 7,
                    request,
                    from_subagent: true,
                });
                Ok(InteractionResponse::Cancelled)
            }

            fn pending(&self) -> Vec<PendingInteraction> {
                self.0.lock().expect("lock").clone()
            }

            fn withdraw(&self, request_id: u64) -> bool {
                let mut pending = self.0.lock().expect("lock");
                let before = pending.len();
                pending.retain(|entry| entry.request_id != request_id);
                pending.len() != before
            }
        }

        let broker = Holding(std::sync::Mutex::new(Vec::new()));
        let pending = broker.pending();
        assert!(pending.is_empty());

        // Register through the same path a tool would take.
        broker
            .request(InteractionRequest::Confirm {
                prompt: "run?".into(),
            })
            .await
            .expect("request");

        let pending = broker.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].request_id, 7);
        assert!(
            pending[0].from_subagent,
            "a child's prompt is labelled so the popup can say so"
        );

        assert!(broker.withdraw(7), "withdrawing an existing request works");
        assert!(broker.pending().is_empty(), "and it leaves the snapshot");
        assert!(!broker.withdraw(7), "a second withdraw is a no-op");
    }

    #[test]
    fn redaction_variants_are_distinct() {
        assert_ne!(RedactionPolicy::Off, RedactionPolicy::Basic);
        assert_ne!(RedactionPolicy::Basic, RedactionPolicy::Credential);
    }
}
