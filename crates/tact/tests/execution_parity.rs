//! Parity between the legacy permission path and the kernel permission port.
//!
//! The Phase 1b adapters are only safe to cut over if they decide exactly what
//! `PermissionManager` decided. These tests compare the two directly across
//! every mode and risk, and check the sensitive-path guard's refusal boundary.

use std::sync::{Arc, Mutex};

use serde_json::json;
use tact::permission::{
    CapabilityRisk, ManagerPermissionService, PermissionBehavior, PermissionManager,
    PermissionMode, SensitivePathGuard,
};
use tact_kernel::tool::descriptor::ToolDescriptor;
use tact_kernel::tool::executor::{
    PermissionDecision, PermissionRequest, PermissionService, SecurityService,
};
use tact_kernel::tool::metadata::{PermissionPolicy, PermissionPromptPolicy, ToolMetadata};

const MODES: [PermissionMode; 3] = [
    PermissionMode::Default,
    PermissionMode::Plan,
    PermissionMode::Auto,
];

const RISKS: [CapabilityRisk; 3] = [
    CapabilityRisk::Read,
    CapabilityRisk::Write,
    CapabilityRisk::High,
];

fn descriptor(policy: PermissionPolicy) -> ToolDescriptor {
    let mut metadata = ToolMetadata::read_json("write_file", "Write", "Write File");
    metadata.permission = policy;
    metadata.permission_prompt = PermissionPromptPolicy::Path { field: "path" };
    ToolDescriptor::from_native("write_file", "Write", json!({"type": "object"}), &metadata)
}

fn policy_for(risk: CapabilityRisk) -> PermissionPolicy {
    match risk {
        CapabilityRisk::Read => PermissionPolicy::Read,
        CapabilityRisk::Write => PermissionPolicy::Write,
        CapabilityRisk::High => PermissionPolicy::High,
    }
}

#[tokio::test]
async fn every_mode_and_risk_decides_like_the_manager() {
    for mode in MODES {
        for risk in RISKS {
            let mut legacy = PermissionManager::try_new(mode).unwrap();
            let behavior = legacy
                .check_with_auto("write_file", risk, &json!({"path": "src/main.rs"}), false)
                .behavior;

            let service = ManagerPermissionService::new(Arc::new(Mutex::new(
                PermissionManager::try_new(mode).unwrap(),
            )));
            let decision = service
                .decide(PermissionRequest {
                    descriptor: descriptor(policy_for(risk)),
                    input: json!({"path": "src/main.rs"}),
                })
                .await
                .unwrap();

            let adapted = match decision {
                PermissionDecision::Allow => PermissionBehavior::Allow,
                PermissionDecision::Deny(_) => PermissionBehavior::Deny,
                PermissionDecision::Ask(_) => PermissionBehavior::Ask,
            };
            assert_eq!(
                adapted, behavior,
                "mode {mode:?} risk {risk:?} diverged between the manager and the port"
            );
        }
    }
}

#[tokio::test]
async fn auto_approved_matches_the_manager_bypass() {
    let mut legacy = PermissionManager::try_new(PermissionMode::Default).unwrap();
    let behavior = legacy
        .check_with_auto(
            "write_file",
            CapabilityRisk::Write,
            &json!({"path": "x"}),
            true,
        )
        .behavior;
    assert_eq!(behavior, PermissionBehavior::Allow);

    let service = ManagerPermissionService::new(Arc::new(Mutex::new(
        PermissionManager::try_new(PermissionMode::Default).unwrap(),
    )))
    .with_auto_approved(Arc::new(|_| true));
    let decision = service
        .decide(PermissionRequest {
            descriptor: descriptor(PermissionPolicy::Write),
            input: json!({"path": "x"}),
        })
        .await
        .unwrap();
    assert_eq!(decision, PermissionDecision::Allow);
}

#[test]
fn sensitive_guard_refuses_credentials_and_passes_secrets() {
    let guard = SensitivePathGuard::builtin();
    let read = descriptor(PermissionPolicy::ReadPath { path_field: "path" });

    let credential = guard.guard(&read, &json!({"path": ".ssh/id_ed25519"}));
    assert!(credential.is_err(), "a private key must be refused");

    let secret = guard.guard(&read, &json!({"path": ".env"}));
    assert!(secret.is_ok(), ".env escalates to High instead of refusing");

    let ordinary = guard.guard(&read, &json!({"path": "src/main.rs"}));
    assert!(ordinary.is_ok());
}

#[test]
fn sensitive_guard_matches_the_policy_classifier() {
    let guard = SensitivePathGuard::builtin();
    let policy = PermissionPolicy::ReadPath { path_field: "path" };
    let descriptor = descriptor(policy);

    for path in [".ssh/id_ed25519", ".env", "src/main.rs", ".aws/credentials"] {
        let input = json!({ "path": path });
        let refused = guard.guard(&descriptor, &input).is_err();
        let credential = tact::permission::is_credential_target(&policy, &input);
        assert_eq!(
            refused, credential,
            "guard and classifier disagree for {path}"
        );
    }
}
