//! Opt-in OS-level sandbox for shell execution.
//!
//! v1 wraps the `bash` tool's `sh -c` process in Linux `bubblewrap`. It is
//! selected by `[tools] sandbox` (default `"none"`), so a default install runs
//! exactly as before.
//!
//! ## Fail-open, never silent
//!
//! The backend is resolved **once at startup**. When the requested backend
//! cannot start — `bwrap` missing from `PATH`, the construction probe failing,
//! a kernel that forbids unprivileged user namespaces, a non-Linux host — the
//! session degrades to unsandboxed execution instead of failing the `bash`
//! tool. Every degradation carries a reason ([`SandboxDegradation::reason`])
//! that the caller announces at startup, so the downgrade is loud even though
//! it is not fatal.
//!
//! ## What this does and does not bound
//!
//! The sandbox constrains *the processes a shell command spawns*. It is not a
//! boundary around the agent: `background_run` and `worktree_run` spawn their
//! own shells and are not sandboxed in v1, and the in-process file tools keep
//! full host access. What it does bound is third-party code an approved command
//! pulls in (`cargo` build scripts, `npm` lifecycle scripts, test binaries).

use std::path::Path;
use std::sync::{Arc, OnceLock};

use crate::config::SandboxBackend;

#[cfg(target_os = "linux")]
mod bwrap;

#[cfg(target_os = "linux")]
pub use bwrap::BwrapSandbox;

/// Builds the process invocation for a sandboxed shell command.
///
/// The trait is deliberately command-oriented: a sandbox constructs the
/// isolated invocation and nothing else. Spawning, stdout/stderr handling,
/// timeouts, cancellation and process-group management stay in the `bash`
/// tool, so the existing execution lifecycle is unchanged by design.
pub trait Sandbox: Send + Sync {
    /// Build the command that runs `program` with `args` inside the sandbox,
    /// with `work_dir` mounted as the sandbox workspace.
    ///
    /// # Errors
    ///
    /// Returns an error if the workspace is refused by the sandbox's boundary
    /// guard or a required host mount is missing.
    fn command(
        &self,
        program: &str,
        args: &[String],
        work_dir: &Path,
    ) -> anyhow::Result<tokio::process::Command>;

    /// Short backend name for diagnostics ("bwrap").
    fn describe(&self) -> &'static str;
}

/// Why the requested sandbox backend is not active for this session.
///
/// Shared through an `Arc` on [`ToolContext`](crate::tool::ToolContext), which
/// is cloned for every tool invocation — so the one-time notice fires once per
/// session, not once per `bash` call.
#[derive(Debug)]
pub struct SandboxDegradation {
    /// Human-readable sentence naming the backend, the cause, and the effect.
    pub reason: String,
    noticed: OnceLock<()>,
}

impl SandboxDegradation {
    fn new(detail: &str) -> Arc<Self> {
        Arc::new(Self {
            reason: format!("sandbox: {detail}; bash will run unsandboxed for this session"),
            noticed: OnceLock::new(),
        })
    }

    /// Returns `true` exactly once per session — the caller then emits the
    /// visible "this command is not sandboxed" notice.
    pub fn first_command_notice(&self) -> bool {
        self.noticed.set(()).is_ok()
    }
}

/// Resolve the configured backend for a session.
///
/// Returns the active sandbox (if any) and, when the requested backend could
/// not start, the degradation to announce. `SandboxBackend::None` yields
/// `(None, None)`: unsandboxed by configuration, which is not a degradation.
pub fn resolve(
    backend: SandboxBackend,
    work_dir: &Path,
) -> (Option<Arc<dyn Sandbox>>, Option<Arc<SandboxDegradation>>) {
    match backend {
        SandboxBackend::None => (None, None),
        SandboxBackend::Bwrap => resolve_bwrap(work_dir),
    }
}

#[cfg(target_os = "linux")]
fn resolve_bwrap(work_dir: &Path) -> (Option<Arc<dyn Sandbox>>, Option<Arc<SandboxDegradation>>) {
    match BwrapSandbox::probe(work_dir) {
        Ok(sandbox) => (Some(Arc::new(sandbox)), None),
        Err(detail) => (None, Some(SandboxDegradation::new(&detail))),
    }
}

#[cfg(not(target_os = "linux"))]
fn resolve_bwrap(_work_dir: &Path) -> (Option<Arc<dyn Sandbox>>, Option<Arc<SandboxDegradation>>) {
    (
        None,
        Some(SandboxDegradation::new(
            "backend 'bwrap' is Linux-only and unavailable on this host",
        )),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn none_backend_is_not_a_degradation() {
        let (sandbox, degraded) = resolve(SandboxBackend::None, &PathBuf::from("/tmp/ws"));
        assert!(sandbox.is_none());
        assert!(degraded.is_none());
    }

    #[test]
    fn degradation_notice_fires_only_once() {
        let degradation = SandboxDegradation::new("bwrap not found on PATH");
        assert!(degradation.first_command_notice());
        assert!(!degradation.first_command_notice());
    }

    #[test]
    fn degradation_reason_names_the_cause_and_the_effect() {
        let degradation = SandboxDegradation::new("bwrap not found on PATH");
        assert!(degradation.reason.contains("bwrap not found on PATH"));
        assert!(degradation.reason.contains("unsandboxed"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn bwrap_backend_resolves_to_a_handle_or_a_reasoned_degradation() {
        // Either outcome is valid here (the host may not have bubblewrap); what
        // must hold is that a failure is never silent.
        let (sandbox, degraded) = resolve(SandboxBackend::Bwrap, &PathBuf::from("/tmp/ws"));
        match (sandbox, degraded) {
            (Some(sandbox), None) => assert_eq!(sandbox.describe(), "bwrap"),
            (None, Some(degraded)) => assert!(!degraded.reason.is_empty()),
            (Some(_), Some(_)) => panic!("a sandbox and a degradation are mutually exclusive"),
            (None, None) => panic!("an unusable backend must report why"),
        }
    }
}
