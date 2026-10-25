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

/// The client-neutral interaction port.
#[async_trait]
pub trait InteractionBroker: Send + Sync {
    async fn request(&self, request: InteractionRequest) -> anyhow::Result<InteractionResponse>;
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

    #[test]
    fn redaction_variants_are_distinct() {
        assert_ne!(RedactionPolicy::Off, RedactionPolicy::Basic);
        assert_ne!(RedactionPolicy::Basic, RedactionPolicy::Credential);
    }
}
