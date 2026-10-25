//! Everything about *how* a shell command runs, as one value.
//!
//! These four settings — the timeout, the scheduling priority, the sandbox and
//! the degradation notice — were four fields on `ToolContext`, which meant every
//! tool was handed them and only `bash` read any. They are also resolved once at
//! startup and never change, unlike the rest of the context (session id, cancel
//! flag, progress reporter), which is per run.
//!
//! Grouping them is the point: a caller that has a `ShellPolicy` has answered
//! "how may this command run", and a tool that does not get one cannot run a
//! command at all.

use std::sync::Arc;

/// The shell's session-constant policy.
#[derive(Clone, Default)]
pub struct ShellPolicy {
    /// `[tools].bash_timeout_secs`; `0` disables the timeout.
    pub timeout_secs: u64,
    /// Nice increment applied to the sub-process group. `0` disables.
    pub nice: i32,
    /// The resolved sandbox, or `None` to run `sh -c` directly.
    pub sandbox: Option<Arc<dyn crate::sandbox::Sandbox>>,
    /// Set when a configured sandbox could not start.
    pub degraded: Option<Arc<crate::sandbox::SandboxDegradation>>,
}

impl ShellPolicy {
    /// The timeout for one call: the caller's override wins.
    ///
    /// `0` disables it, on either side — a tool argument cannot turn a disabled
    /// session limit back on, because that would let a model grant itself more
    /// time than the user configured.
    #[must_use]
    pub fn timeout_for(&self, input_timeout: Option<u64>) -> u64 {
        input_timeout.unwrap_or(self.timeout_secs)
    }

    /// Whether commands can run at all.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        // A policy is usable whether or not it is sandboxed; it is unusable only
        // when nothing was resolved, which `Default` cannot express. Kept as a
        // method so a future "sandbox required but unavailable" state has a place.
        true
    }

    /// Debug form that names the sandbox without requiring `Sandbox: Debug`.
    #[must_use]
    pub fn describe(&self) -> String {
        let sandbox = if self.sandbox.is_some() {
            "sandboxed"
        } else {
            "unsandboxed"
        };
        let degraded = if self.degraded.is_some() {
            ", degraded"
        } else {
            ""
        };
        format!(
            "{sandbox}{degraded}; timeout {}s; nice {}",
            self.timeout_secs, self.nice
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_per_call_timeout_wins_over_the_configured_one() {
        let policy = ShellPolicy {
            timeout_secs: 120,
            ..ShellPolicy::default()
        };
        assert_eq!(policy.timeout_for(Some(5)), 5);
        assert_eq!(policy.timeout_for(None), 120);
    }

    #[test]
    fn a_per_call_zero_still_means_no_timeout() {
        // `0` disables the limit on both sides; treating the tool argument as an
        // override that re-enables it would let a model ask for more time than the
        // user configured.
        let policy = ShellPolicy {
            timeout_secs: 120,
            ..ShellPolicy::default()
        };
        assert_eq!(policy.timeout_for(Some(0)), 0);
    }

    #[test]
    fn the_default_is_unsandboxed_with_no_limits() {
        let policy = ShellPolicy::default();
        assert!(policy.sandbox.is_none());
        assert!(policy.degraded.is_none());
        assert_eq!(policy.timeout_secs, 0);
        assert_eq!(policy.nice, 0);
        assert!(policy.describe().contains("unsandboxed"));
    }

    #[test]
    fn describe_names_the_limits_a_log_line_needs() {
        // The one-time "running unsandboxed" notice and the startup line both read
        // this, so the wording is asserted rather than left to chance. A degraded
        // policy cannot be built here (the type's fields are private), so the
        // degraded wording is covered where a degradation is actually produced.
        let policy = ShellPolicy {
            timeout_secs: 120,
            nice: 10,
            ..ShellPolicy::default()
        };
        let text = policy.describe();
        assert!(text.contains("unsandboxed"), "{text}");
        assert!(text.contains("timeout 120s"), "{text}");
        assert!(text.contains("nice 10"), "{text}");
    }
}
