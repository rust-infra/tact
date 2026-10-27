//! Interior-synchronized permission ownership for one logical agent.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use serde_json::Value;

use crate::{
    permission::{
        AllowOutcome, CapabilityRisk, PermissionDecision, PermissionManager, PermissionMode,
        PermissionSnapshot,
    },
    utils::LockExt,
};

/// The single permission authority shared by an agent's facade, gate, and mode
/// controls.
///
/// Every method takes the lock only for the duration of one synchronous
/// manager operation. Permission popups must be awaited before calling the
/// mutation that records their answer, never while this lock is held.
#[derive(Clone)]
pub struct PermissionAuthority {
    manager: Arc<Mutex<PermissionManager>>,
}

impl PermissionAuthority {
    pub(crate) fn new(manager: PermissionManager) -> Self {
        Self {
            manager: Arc::new(Mutex::new(manager)),
        }
    }

    /// Build a child authority from an inherited snapshot.
    ///
    /// This creates a fresh live manager: later changes in the child do not
    /// mutate the parent, and denial counters start clean.
    pub(crate) fn from_snapshot(snapshot: PermissionSnapshot) -> Self {
        Self::new(PermissionManager::from_snapshot(snapshot))
    }

    pub(crate) fn mode(&self) -> PermissionMode {
        self.manager.lock_recover().mode()
    }

    pub fn set_mode(&self, mode: PermissionMode) {
        self.manager.lock_recover().set_mode(mode);
    }

    pub(crate) fn rules(&self) -> Vec<String> {
        self.manager.lock_recover().rules().to_vec()
    }

    pub(crate) fn snapshot(&self) -> PermissionSnapshot {
        self.manager.lock_recover().snapshot()
    }

    pub(crate) fn scanner(&self) -> crate::security::sensitive::Scanner {
        self.manager.lock_recover().scanner()
    }

    pub(crate) fn security_config(&self) -> crate::security::SecurityConfig {
        self.manager.lock_recover().security_config()
    }

    pub(crate) fn check_with_auto(
        &self,
        tool_name: &str,
        risk: CapabilityRisk,
        input: &Value,
        auto_approved: bool,
    ) -> PermissionDecision {
        self.manager
            .lock_recover()
            .check_with_auto(tool_name, risk, input, auto_approved)
    }

    pub(crate) fn ask_user(&self, tool_name: &str, risk: CapabilityRisk) -> Result<bool> {
        self.manager.lock_recover().ask_user(tool_name, risk)
    }

    pub(crate) fn allow_tool_with_input(
        &self,
        tool_name: &str,
        policy: crate::tool::PermissionPromptPolicy,
        input: &Value,
    ) -> AllowOutcome {
        self.manager
            .lock_recover()
            .allow_tool_with_input(tool_name, policy, input)
    }

    pub(crate) fn allow_tool_as_prefix(
        &self,
        tool_name: &str,
        policy: crate::tool::PermissionPromptPolicy,
        input: &Value,
    ) -> AllowOutcome {
        self.manager
            .lock_recover()
            .allow_tool_as_prefix(tool_name, policy, input)
    }

    #[cfg(test)]
    pub(crate) fn shares_manager_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.manager, &other.manager)
    }

    #[cfg(test)]
    pub(crate) fn consecutive_denials(&self) -> usize {
        self.manager.lock_recover().consecutive_denials()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::{
        permission::{AllowOutcome, PermissionBehavior, PermissionMode},
        tool::PermissionPromptPolicy,
    };

    #[test]
    fn cloned_handles_share_one_live_authority() {
        let authority =
            PermissionAuthority::new(PermissionManager::try_new(PermissionMode::Default).unwrap());
        let gate = authority.clone();

        authority.set_mode(PermissionMode::Plan);

        assert!(authority.shares_manager_with(&gate));
        assert_eq!(gate.mode(), PermissionMode::Plan);
        assert_eq!(
            gate.check_with_auto("bash", CapabilityRisk::Write, &json!({}), false)
                .behavior,
            PermissionBehavior::Deny
        );
    }

    #[test]
    fn child_authority_starts_from_a_snapshot_and_diverges() {
        let parent =
            PermissionAuthority::new(PermissionManager::try_new(PermissionMode::Default).unwrap());
        parent.manager.lock_recover().allow_tool_with_input(
            "bash",
            PermissionPromptPolicy::Command { field: "command" },
            &json!({"command": "cargo test"}),
        );
        let child = PermissionAuthority::from_snapshot(parent.snapshot());

        assert!(!parent.shares_manager_with(&child));
        assert_eq!(child.mode(), PermissionMode::Default);
        assert_eq!(child.rules(), parent.rules());

        child.set_mode(PermissionMode::Auto);
        assert_eq!(parent.mode(), PermissionMode::Default);
        assert_eq!(child.mode(), PermissionMode::Auto);
    }

    #[test]
    fn permission_authority_matrix_covers_grants_denials_modes_and_inheritance() {
        let authority =
            PermissionAuthority::new(PermissionManager::try_new(PermissionMode::Default).unwrap());
        let input = json!({"path": "notes.txt"});

        assert_eq!(
            authority
                .check_with_auto("edit_file", CapabilityRisk::Write, &input, false)
                .behavior,
            PermissionBehavior::Ask
        );

        let rules_before_allow_once = authority.rules().len();
        assert!(
            authority
                .ask_user("edit_file", CapabilityRisk::Write)
                .unwrap(),
            "non-interactive write approval is an allow-once"
        );
        assert_eq!(
            authority.rules().len(),
            rules_before_allow_once,
            "allow-once must not leave a durable grant"
        );

        assert_eq!(
            authority.allow_tool_with_input(
                "edit_file",
                PermissionPromptPolicy::Path { field: "path" },
                &input,
            ),
            AllowOutcome::Recorded
        );
        assert_eq!(
            authority
                .check_with_auto("edit_file", CapabilityRisk::Write, &input, false)
                .behavior,
            PermissionBehavior::Allow
        );

        for expected in 1..=3 {
            assert!(
                !authority
                    .ask_user("high_risk", CapabilityRisk::High)
                    .unwrap()
            );
            assert_eq!(authority.consecutive_denials(), expected);
        }
        assert!(
            authority
                .ask_user("edit_file", CapabilityRisk::Write)
                .unwrap()
        );
        assert_eq!(
            authority.consecutive_denials(),
            0,
            "a later allow resets the denial streak"
        );

        authority.set_mode(PermissionMode::Plan);
        assert_eq!(
            authority
                .check_with_auto("edit_file", CapabilityRisk::Write, &input, false)
                .behavior,
            PermissionBehavior::Deny
        );
        authority.set_mode(PermissionMode::Auto);
        assert_eq!(
            authority
                .check_with_auto("high_risk", CapabilityRisk::High, &json!({}), false)
                .behavior,
            PermissionBehavior::Allow
        );

        let parent =
            PermissionAuthority::new(PermissionManager::try_new(PermissionMode::Plan).unwrap());
        assert_eq!(
            parent.allow_tool_with_input(
                "edit_file",
                PermissionPromptPolicy::Path { field: "path" },
                &input,
            ),
            AllowOutcome::Recorded
        );
        assert!(!parent.ask_user("high_risk", CapabilityRisk::High).unwrap());
        let child = PermissionAuthority::from_snapshot(parent.snapshot());
        assert!(!parent.shares_manager_with(&child));
        assert_eq!(child.mode(), PermissionMode::Plan);
        assert_eq!(child.rules(), parent.rules());
        assert_eq!(
            child.consecutive_denials(),
            0,
            "a subagent inherits grants, not its parent's denial streak"
        );
        child.set_mode(PermissionMode::Auto);
        assert_eq!(parent.mode(), PermissionMode::Plan);
        assert_eq!(child.mode(), PermissionMode::Auto);
    }
}
