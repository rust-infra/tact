//! Centralized permission boundary.
//!
//! Every capability invocation — native, MCP, or plugin — is authorized through
//! [`PermissionService`] before its handler runs. The Kernel owns the *decision
//! boundary*; a host supplies the policy implementation. `Ok(())` is "allow";
//! an `Err(KernelError)` is a denial whose category and reason survive the
//! protocol boundary.

use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{CapabilityDeclaration, InteractionRequest, InteractionResponse};

use crate::{InvocationContext, KernelError};

#[async_trait]
pub trait PermissionService: Send + Sync {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        context: &InvocationContext,
        input: &Value,
    ) -> Result<(), KernelError>;

    async fn request(
        &self,
        _request: InteractionRequest,
        _context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError> {
        Err(KernelError::permission_denied(
            "permission interaction is not available",
        ))
    }
}

// ── Centralized permission decision ──────────────────────────────────────
//
// The Kernel owns the decision itself, not just the boundary: every host —
// in-process tools, the Node host, the WASM host — resolves a capability
// through [`decide`]. A host supplies the loaded rules ([`PermissionRules`]);
// the decision order (read → plan → auto → rules → risk → allow-list) is the
// Kernel's.

/// Risk classification of a capability under the permission ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityRisk {
    Read,
    Write,
    High,
}

impl std::fmt::Display for CapabilityRisk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::High => "high",
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
}

impl std::fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Default => "default - ask for writes",
            Self::Plan => "plan - read only",
            Self::Auto => "auto - allow non-high operations",
        })
    }
}

impl PermissionMode {
    /// The mode's name in the Claude Code / plugin-hook vocabulary.
    #[must_use]
    pub fn hook_name(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Plan => "plan",
            Self::Auto => "acceptEdits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionBehavior {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionDecision {
    pub behavior: PermissionBehavior,
    pub reason: String,
}

impl PermissionDecision {
    fn allow(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Allow,
            reason: reason.into(),
        }
    }

    fn ask(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Ask,
            reason: reason.into(),
        }
    }

    fn deny(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            reason: reason.into(),
        }
    }
}

/// What a loaded permission rule says about one capability call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleAction {
    Allow,
    Ask,
    Deny,
}

/// The loaded permission rules a host supplies to [`decide`].
pub trait PermissionRules {
    /// `Some` when a rule matches, `None` when none does.
    fn action(&self, capability: &str, input: &Value) -> Option<RuleAction>;
}

/// The inputs to one decision.
pub struct DecisionInput<'a> {
    pub capability: &'a str,
    pub risk: CapabilityRisk,
    pub input: &'a Value,
    /// Loaded rules, when a settings store is present.
    pub rules: Option<&'a dyn PermissionRules>,
    /// An MCP server entry opted this capability into `approval_mode: "auto"`.
    pub auto_approved: bool,
    /// The in-session always-allowed list matched this exact capability+input.
    pub always_allowed: bool,
}

/// The central permission decision.
///
/// Ordering is the safety contract, and it is the same for every host:
///
/// 1. Read capabilities are always allowed.
/// 2. Plan mode blocks every write/high capability.
/// 3. Auto mode trusts the agent.
/// 4. A loaded `deny` rule blocks before any prompt.
/// 5. A loaded `allow` rule permits (including high risk — explicit trust).
/// 6. A loaded `ask` rule prompts (high risk still prompts unless the server
///    opted into auto).
/// 7. A server's `auto` entry can skip the default prompt, never a local rule.
/// 8. High risk prompts unless the user allowed this exact capability+input.
/// 9. An in-session always-allowed entry permits.
/// 10. Otherwise: ask.
#[must_use]
pub fn decide(mode: PermissionMode, input: &DecisionInput<'_>) -> PermissionDecision {
    // 1. Read capabilities are always allowed.
    if input.risk == CapabilityRisk::Read {
        return PermissionDecision::allow("Read-only capability allowed");
    }
    // 2. Plan mode blocks all write/High operations.
    if mode == PermissionMode::Plan {
        return PermissionDecision::deny("Plan mode: write operations are blocked");
    }
    // 3. Auto mode trusts the agent — skip all risk checks.
    if mode == PermissionMode::Auto {
        return PermissionDecision::allow("Auto mode: all capabilities auto-approved");
    }

    // At this point we are in Default mode.
    let capability = input.capability;
    let settings_action = input
        .rules
        .and_then(|rules| rules.action(capability, input.input));

    match settings_action {
        // 4. Deny outranks everything below it.
        Some(RuleAction::Deny) => {
            return PermissionDecision::deny(format!(
                "Blocked by project permission rule: {capability}"
            ));
        }
        // 5. An explicit allow rule is the user trusting the pattern.
        Some(RuleAction::Allow) => {
            return PermissionDecision::allow(format!(
                "Allowed by project permission rule: {capability}"
            ));
        }
        // 6. An explicit local `ask` rule outranks the server's own entry.
        Some(RuleAction::Ask) if input.risk != CapabilityRisk::High || input.auto_approved => {
            return PermissionDecision::ask(format!(
                "Project permission rule requires confirmation: {capability}"
            ));
        }
        // 7. The server entry opted this capability into `approval_mode: "auto"`.
        _ if input.auto_approved => {
            return PermissionDecision::allow(format!(
                "Auto-approved by the MCP server entry: {capability}"
            ));
        }
        _ => {}
    }

    // 8. High risk asks first, then honours an explicit in-session allow.
    if input.risk == CapabilityRisk::High {
        if input.always_allowed {
            return PermissionDecision::allow(format!(
                "Always-allowed high-risk capability: {capability}"
            ));
        }
        return PermissionDecision::ask(format!(
            "High-risk capability requires approval: {capability}"
        ));
    }

    // 9. In-session always-allowed list.
    if input.always_allowed {
        return PermissionDecision::allow(format!("Always allowed tool: {capability}"));
    }

    // 10. Default: ask.
    PermissionDecision::ask(format!("Default mode: asking user for {capability}"))
}
