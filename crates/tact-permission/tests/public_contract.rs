use std::str::FromStr;

use tact_permission::{
    AllowOutcome, CapabilityRisk, PermissionBehavior, PermissionDecision, PermissionMode,
    normalize_mcp_capability,
};

#[test]
fn permission_contract_is_independent_of_agent_and_tools() {
    assert_eq!(PermissionMode::from_str("plan"), Ok(PermissionMode::Plan));
    assert_eq!(PermissionMode::Auto.hook_name(), "acceptEdits");
    assert_eq!(
        normalize_mcp_capability("server", "tool"),
        CapabilityRisk::High
    );

    let decision = PermissionDecision::ask("needs approval");
    assert_eq!(decision.behavior, PermissionBehavior::Ask);
    assert!(!decision.reason.is_empty());
    assert!(AllowOutcome::Recorded.is_recorded());
}
