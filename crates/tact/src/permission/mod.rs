//! Permission system for tool invocation.
//!
//! Every tool call is classified by [`CapabilityRisk`] (Read / Write / High)
//! from its typed metadata. The [`PermissionManager`] decides whether to allow,
//! deny, or ask the user, depending on:
//!
//! - The active [`PermissionMode`] (Default, Plan, Auto).
//! - The risk level of the tool.
//! - A per-user allow-list (`always_allowed_tools`).
//! - Consecutive denials (which may trigger a suggestion to switch to Plan mode).

use std::fmt;

use anyhow::Result;
use serde_json::Value;
use strum_macros::{Display, EnumString};

use crate::tool::PermissionPromptPolicy;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[strum(serialize_all = "snake_case")]
pub enum CapabilityRisk {
    Read,
    Write,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, EnumString)]
#[strum(serialize_all = "snake_case")]
pub enum PermissionMode {
    Default,
    Plan,
    Auto,
}

impl fmt::Display for PermissionMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            PermissionMode::Default => "default - ask for writes",
            PermissionMode::Plan => "plan - read only",
            PermissionMode::Auto => "auto - allow non-high operations",
        };

        write!(f, "{label}")
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

    #[allow(dead_code)]
    fn deny(reason: impl Into<String>) -> Self {
        Self {
            behavior: PermissionBehavior::Deny,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Display)]
#[allow(dead_code)]
enum UserPermissionChoice {
    #[strum(serialize = "allow once")]
    AllowOnce,
    #[strum(serialize = "deny")]
    Deny,
    #[strum(serialize = "always allow this tool")]
    AlwaysAllow,
}

#[derive(Debug)]
pub struct PermissionManager {
    mode: PermissionMode,
    always_allowed_tools: Vec<String>,
    consecutive_denials: usize,
    max_consecutive_denials: usize,
}

impl PermissionManager {
    pub fn try_new(mode: PermissionMode) -> Result<Self> {
        Ok(Self {
            mode,
            always_allowed_tools: vec!["read_file".to_string()],
            consecutive_denials: 0,
            max_consecutive_denials: 3,
        })
    }

    pub fn mode(&self) -> PermissionMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    pub fn rules(&self) -> &[String] {
        &self.always_allowed_tools
    }

    /// Check permission for a tool given its stable name and resolved risk.
    pub fn check(&mut self, tool_name: &str, risk: CapabilityRisk) -> PermissionDecision {
        if risk == CapabilityRisk::Read {
            self.consecutive_denials = 0;
            return PermissionDecision::allow("Read-only capability allowed");
        }

        if self.mode == PermissionMode::Plan {
            return PermissionDecision::deny("Plan mode: write operations are blocked");
        }

        if risk == CapabilityRisk::High {
            return PermissionDecision::ask(format!(
                "High-risk capability requires approval: {}",
                tool_name
            ));
        }

        if self.is_always_allowed(tool_name) {
            self.consecutive_denials = 0;
            return PermissionDecision::allow(format!("Always allowed tool: {tool_name}"));
        }

        match self.mode {
            PermissionMode::Auto => {
                self.consecutive_denials = 0;
                PermissionDecision::allow("Auto mode: non-high capability auto-approved")
            }
            PermissionMode::Default | PermissionMode::Plan => {
                PermissionDecision::ask(format!("Default mode: asking user for {tool_name}"))
            }
        }
    }

    pub fn ask_user(&mut self, tool_name: &str) -> Result<bool> {
        eprintln!("[permission] non-interactive: denying {}", tool_name);
        let approved = self.apply_user_choice(UserPermissionChoice::Deny, tool_name);
        if !approved && self.should_suggest_plan_mode() {
            eprintln!(
                "[{} consecutive denials -- consider switching to plan mode]",
                self.consecutive_denials
            );
        }
        Ok(approved)
    }

    fn apply_user_choice(&mut self, choice: UserPermissionChoice, tool_name: &str) -> bool {
        match choice {
            UserPermissionChoice::AllowOnce => {
                self.consecutive_denials = 0;
                true
            }
            UserPermissionChoice::Deny => {
                self.consecutive_denials += 1;
                false
            }
            UserPermissionChoice::AlwaysAllow => {
                self.allow_tool(tool_name);
                self.consecutive_denials = 0;
                true
            }
        }
    }

    pub fn allow_tool(&mut self, tool_name: &str) {
        if !self.is_always_allowed(tool_name) {
            self.always_allowed_tools.push(tool_name.to_string());
        }
    }

    fn is_always_allowed(&self, tool_name: &str) -> bool {
        self.always_allowed_tools
            .iter()
            .any(|allowed| allowed == tool_name)
    }

    fn should_suggest_plan_mode(&self) -> bool {
        self.consecutive_denials >= self.max_consecutive_denials
    }

    #[allow(dead_code)]
    pub fn set_max_consecutive_denials(&mut self, max: usize) {
        self.max_consecutive_denials = max;
    }
}

/// Format a user-facing permission prompt using typed policy.
pub fn format_permission_prompt(
    name: &str,
    policy: PermissionPromptPolicy,
    input: &Value,
) -> String {
    let field_str = |field: &str| input.get(field).and_then(|v| v.as_str()).unwrap_or("");
    match policy {
        PermissionPromptPolicy::Command { field } => format!("Run command: {}", field_str(field)),
        PermissionPromptPolicy::Question { field } => format!("Ask user: {}", field_str(field)),
        PermissionPromptPolicy::Path { field } => format!("Allow {name} on {}?", field_str(field)),
        PermissionPromptPolicy::Json => format!("Allow {name}?"),
    }
}

/// MCP tools always start as High risk.
pub fn normalize_mcp_capability(_server: &str, _tool: &str) -> CapabilityRisk {
    CapabilityRisk::High
}

#[allow(dead_code)]
fn truncate_for_prompt(input: &Value, _max_chars: usize) -> String {
    input.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_list_matches_exact_name() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool("read_file");
        assert!(mgr.is_always_allowed("read_file"));
    }

    #[test]
    fn deny_increments_consecutive_count() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        assert_eq!(mgr.consecutive_denials, 1);
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        assert_eq!(mgr.consecutive_denials, 2);
    }

    #[test]
    fn allow_resets_consecutive_count() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.apply_user_choice(UserPermissionChoice::Deny, "bash");
        mgr.apply_user_choice(UserPermissionChoice::AllowOnce, "bash");
        assert_eq!(mgr.consecutive_denials, 0);
    }

    #[test]
    fn plan_mode_denies_write_including_mcp() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Plan).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::Write);
        assert_eq!(decision.behavior, PermissionBehavior::Deny);
        let mcp_decision = mgr.check("mcp__srv__tool", CapabilityRisk::Write);
        assert_eq!(mcp_decision.behavior, PermissionBehavior::Deny);
    }

    #[test]
    fn auto_mode_allows_non_high_capabilities() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Auto).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::Write);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn default_mode_asks_for_write() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let decision = mgr.check("bash", CapabilityRisk::Write);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }

    #[test]
    fn default_mode_allows_resolved_read_capability() {
        let mut manager = PermissionManager::try_new(PermissionMode::Default).unwrap();
        let decision = manager.check("read_file", CapabilityRisk::Read);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn path_prompt_policy_preserves_file_prompt() {
        let prompt = format_permission_prompt(
            "edit_file",
            PermissionPromptPolicy::Path { field: "path" },
            &serde_json::json!({"path": "src/lib.rs"}),
        );
        assert_eq!(prompt, "Allow edit_file on src/lib.rs?");
    }

    #[test]
    fn always_allow_and_check_skips_prompt() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool("bash");
        let decision = mgr.check("bash", CapabilityRisk::Write);
        assert_eq!(decision.behavior, PermissionBehavior::Allow);
    }

    #[test]
    fn high_risk_requires_approval_even_for_allowed_tool() {
        let mut mgr = PermissionManager::try_new(PermissionMode::Default).unwrap();
        mgr.allow_tool("bash");
        let decision = mgr.check("bash", CapabilityRisk::High);
        assert_eq!(decision.behavior, PermissionBehavior::Ask);
    }
}
