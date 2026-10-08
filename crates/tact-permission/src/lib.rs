//! Pure permission value types shared by the runtime and capability owners.
//!
//! This crate deliberately excludes policy storage, tool metadata, UI and
//! `Agent` state. Those concerns remain in `tact` until their adapters have
//! explicit owner boundaries.

use std::fmt;

use strum_macros::EnumString;

pub use tact_contracts::capability::CapabilityRisk;

/// Permission mode presented to tools and compatibility hooks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
}

impl PermissionMode {
    /// Claude/Codex-compatible spelling used in hook payloads.
    #[must_use]
    pub fn hook_name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::Auto => "acceptEdits",
        }
    }
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            Self::Default => "default - ask for writes",
            Self::Plan => "plan - read only",
            Self::Auto => "auto - allow non-high operations",
        };
        f.write_str(label)
    }
}

/// Result of evaluating a capability against the current policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

/// Result of recording an "always allow" choice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllowOutcome {
    Recorded,
    NotNarrowable,
}

impl AllowOutcome {
    #[must_use]
    pub fn is_recorded(self) -> bool {
        matches!(self, Self::Recorded)
    }
}

/// Decision returned by a permission evaluator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub reason: String,
}

impl PermissionDecision {
    #[must_use]
    pub fn allow(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn ask(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Ask,
            reason: reason.into(),
        }
    }

    #[must_use]
    pub fn deny(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            reason: reason.into(),
        }
    }
}

/// Default risk for an MCP capability without an explicit declaration.
#[must_use]
pub fn normalize_mcp_capability(_server: &str, _tool: &str) -> CapabilityRisk {
    CapabilityRisk::High
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_and_risk_spellings_are_stable() {
        assert_eq!(PermissionMode::Auto.hook_name(), "acceptEdits");
        assert_eq!(PermissionMode::Plan.to_string(), "plan - read only");
        assert_eq!(CapabilityRisk::High.to_string(), "high");
    }

    #[test]
    fn decisions_and_allow_outcomes_are_pure_values() {
        assert_eq!(
            PermissionDecision::allow("ok").behavior,
            PermissionBehavior::Allow
        );
        assert_eq!(
            PermissionDecision::ask("confirm").behavior,
            PermissionBehavior::Ask
        );
        assert_eq!(
            PermissionDecision::deny("blocked").behavior,
            PermissionBehavior::Deny
        );
        assert!(AllowOutcome::Recorded.is_recorded());
        assert!(!AllowOutcome::NotNarrowable.is_recorded());
    }
}
