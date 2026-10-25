//! The host state a hook payload needs, without an `Agent`.
//!
//! Every hook callback in this crate took `&Agent`, which is why the hook engine
//! could not live behind a kernel port. What it actually reads is small and
//! neutral: a session id, the model name, the permission mode, the turn count,
//! and the session-start source — plus a way to emit a notice. This trait is
//! exactly that, so a payload can be built (and a hook run) without the agent
//! loop being in scope.

use std::time::Instant;

use tact_protocol::{AgentUpdate, UpdateSink};

use crate::utils::LockExt;

use crate::permission::PermissionMode;

/// The live facts a hook is given about its run.
///
/// A snapshot, not a handle: a hook that wants to *change* something says so
/// through its return value, which is why nothing here is mutable. The one
/// exception is [`Self::emit`], because a status line or a failure notice is an
/// observation, not a decision.
pub trait HookHost: Send + Sync {
    fn session_id(&self) -> Option<String>;
    /// The model name the session is running, for `payload.model`.
    fn model(&self) -> String;
    fn permission_mode(&self) -> PermissionMode;
    fn turns_taken(&self) -> u32;
    /// The `SessionStart` source matcher value (`startup`, `resume`, …).
    fn session_start_source(&self) -> &'static str;
    /// Where a hook's notice goes.
    fn emit(&self, update: AgentUpdate);
    /// Allocate an id for a hook status row.
    fn next_hook_status_id(&self) -> u64;

    /// The update sink, for the few call sites that need a channel rather than
    /// an emit.
    fn events(&self) -> UpdateSink;

    /// Call an MCP tool, for an `mcp_tool` hook.
    ///
    /// A hook kind rather than a general capability: an `mcp_tool` hook *is* a
    /// call to a server, so the port has to offer one. `None` means this host
    /// has no MCP router attached, which the caller reports instead of failing
    /// silently.
    fn mcp_call(
        &self,
        full_name: String,
        arguments: serde_json::Value,
    ) -> Option<std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send>>>;

    /// Record context a hook wants in the next request.
    ///
    /// `PreToolUse` and `PostToolUse` collect `additionalContext` here; it is
    /// framed and injected before the following request, never into the tool
    /// input.
    fn queue_context(&self, chunk: crate::hook::HookContextChunk);
}

/// A host with nothing attached, for a hook run with no live session.
///
/// Used by the tests that exercise `run_hook` against a fixture, and by any
/// caller that needs a payload but has no agent.
#[derive(Clone)]
pub struct DetachedHookHost {
    events: UpdateSink,
    session_id: Option<String>,
    model: String,
}

impl DetachedHookHost {
    pub fn new(events: UpdateSink, session_id: Option<String>, model: impl Into<String>) -> Self {
        Self {
            events,
            session_id,
            model: model.into(),
        }
    }

    /// The smallest host: no session, no destinations.
    pub fn headless() -> Self {
        Self::new(UpdateSink::default(), None, "unknown")
    }
}

impl HookHost for DetachedHookHost {
    fn session_id(&self) -> Option<String> {
        self.session_id.clone()
    }

    fn model(&self) -> String {
        self.model.clone()
    }

    fn permission_mode(&self) -> PermissionMode {
        // A detached host has no policy: a hook that branches on the mode sees
        // the most restrictive one rather than a permissive default.
        PermissionMode::Plan
    }

    fn turns_taken(&self) -> u32 {
        0
    }

    fn session_start_source(&self) -> &'static str {
        "startup"
    }

    fn emit(&self, update: AgentUpdate) {
        self.events.send(update);
    }

    fn next_hook_status_id(&self) -> u64 {
        // Detached hosts have no shared counter, so ids are only unique per
        // host. Nothing renders them without a session.
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }

    fn events(&self) -> UpdateSink {
        self.events.clone()
    }

    fn mcp_call(
        &self,
        _full_name: String,
        _arguments: serde_json::Value,
    ) -> Option<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'static>,
        >,
    > {
        // No router attached: the caller reports "needs a live agent".
        None
    }

    fn queue_context(&self, _chunk: crate::hook::HookContextChunk) {
        // Nowhere to inject into: a detached run has no conversation.
    }
}

/// A status row a hook opened, so the caller can close the same id.
pub struct OpenStatus {
    pub id: u64,
    pub started: Instant,
    pub source: String,
    pub message: String,
}

/// Open a hook status row, if the hook declared one.
pub fn open_status(
    host: Option<&dyn HookHost>,
    source: &str,
    message: Option<&str>,
) -> Option<OpenStatus> {
    let (host, message) = (host?, message?);
    let id = host.next_hook_status_id();
    host.emit(AgentUpdate::HookStatus {
        id,
        source: Some(source.to_string()),
        message: message.to_string(),
        elapsed_ms: None,
    });
    Some(OpenStatus {
        id,
        started: Instant::now(),
        source: source.to_string(),
        message: message.to_string(),
    })
}

/// Close the row [`open_status`] opened: same id, same wording, now timed.
pub fn close_status(host: Option<&dyn HookHost>, opened: Option<OpenStatus>) {
    let (Some(host), Some(opened)) = (host, opened) else {
        return;
    };
    host.emit(AgentUpdate::HookStatus {
        id: opened.id,
        source: Some(opened.source),
        message: opened.message,
        elapsed_ms: Some(opened.started.elapsed().as_millis() as u64),
    });
}

/// Report a hook failure as a notice, never as a block.
///
/// Every hook in this crate is fail-open: a broken policy script must not be
/// able to halt the loop by being broken.
pub fn report_failure(host: Option<&dyn HookHost>, event: &str, detail: &str) {
    tracing::warn!("plugin hook command failed (continuing): {detail}");
    if let Some(host) = host {
        host.emit(AgentUpdate::Info(format!(
            "[plugin hook {event} failed] {detail}"
        )));
    }
}

/// A borrow of an `Agent` as a hook host.
///
/// The hook engine's callbacks are stored for the life of the agent and receive
/// `&Agent`; this wrapper is what lets a `&Agent` become the neutral port
/// without the engine having to know about the agent loop. Borrowing (not
/// `Arc`) because every call site already holds the reference for exactly as
/// long as the run.
pub struct AgentHookHost<'a> {
    agent: &'a crate::Agent,
}

impl<'a> AgentHookHost<'a> {
    pub fn new(agent: &'a crate::Agent) -> Self {
        Self { agent }
    }
}

impl<'a> HookHost for AgentHookHost<'a> {
    fn session_id(&self) -> Option<String> {
        self.agent.runtime.session_id.clone()
    }

    fn model(&self) -> String {
        self.agent.model().to_string()
    }

    fn permission_mode(&self) -> PermissionMode {
        self.agent.runtime.permission_manager.mode()
    }

    fn turns_taken(&self) -> u32 {
        self.agent.turns_taken
    }

    fn session_start_source(&self) -> &'static str {
        self.agent.runtime.session_start_source.as_str()
    }

    fn emit(&self, update: AgentUpdate) {
        self.agent.emit_update(update);
    }

    fn next_hook_status_id(&self) -> u64 {
        self.agent.next_hook_status_id()
    }

    fn events(&self) -> UpdateSink {
        self.agent.runtime.events.clone()
    }

    fn mcp_call(
        &self,
        full_name: String,
        arguments: serde_json::Value,
    ) -> Option<
        std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<String>> + Send + 'static>,
        >,
    > {
        // `'static` because `AgentHookHost` only borrows the agent, so a future
        // capturing `self` could not outlive the caller's stack frame. The
        // router's clients are `Arc`ed, so a snapshot of the router carries the
        // same connections and can be moved into the future.
        let router = self.agent.mcp_router.snapshot();
        Some(Box::pin(
            async move { router.call(&full_name, arguments).await },
        ))
    }

    fn queue_context(&self, chunk: crate::hook::HookContextChunk) {
        self.agent
            .runtime
            .pending_hook_context
            .lock_recover()
            .push_back(chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    #[test]
    fn a_detached_host_defaults_to_the_restrictive_mode() {
        // A hook that branches on the mode must not read a permissive default
        // from a host that has no policy.
        let host = DetachedHookHost::headless();
        assert_eq!(host.permission_mode(), PermissionMode::Plan);
        assert!(host.session_id().is_none());
        assert_eq!(host.turns_taken(), 0);
    }

    #[test]
    fn emitting_reaches_the_sink() {
        let (tx, mut rx) = unbounded_channel();
        let host = DetachedHookHost::new(UpdateSink::observing(tx), Some("s".into()), "m");
        report_failure(Some(&host), "PreToolUse", "boom");
        assert_eq!(host.session_id().as_deref(), Some("s"));
        assert_eq!(host.model(), "m");
        match rx.try_recv() {
            Ok(AgentUpdate::Info(text)) => assert!(text.contains("boom"), "{text}"),
            other => panic!("expected an Info notice, got {other:?}"),
        }
    }

    #[test]
    fn a_status_row_is_opened_and_closed_with_one_id() {
        let (tx, mut rx) = unbounded_channel();
        let host = DetachedHookHost::new(UpdateSink::observing(tx), None, "m");
        let opened = open_status(Some(&host), "plugin test", Some("working"));
        assert!(opened.is_some());
        close_status(Some(&host), opened);

        let first = rx.try_recv().unwrap();
        let second = rx.try_recv().unwrap();
        match (first, second) {
            (
                AgentUpdate::HookStatus {
                    id: a,
                    elapsed_ms: None,
                    ..
                },
                AgentUpdate::HookStatus {
                    id: b,
                    elapsed_ms: Some(_),
                    ..
                },
            ) => assert_eq!(a, b, "the same row is closed"),
            other => panic!("expected an open then a closed row, got {other:?}"),
        }
    }

    #[test]
    fn a_hook_without_a_status_message_opens_nothing() {
        let host = DetachedHookHost::headless();
        assert!(open_status(Some(&host), "src", None).is_none());
        // And closing nothing is not an error.
        close_status(Some(&host), None);
        close_status(None, None);
    }
}
