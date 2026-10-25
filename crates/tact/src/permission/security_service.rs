//! The kernel `SecurityService` port, backed by the sensitive-path scanner.
//!
//! This is the one production port that needs nothing from an `Agent`: the
//! scanner and the classification live in this crate and are already shared
//! with the legacy dispatch path.

use tact_kernel::tool::descriptor::ToolDescriptor;
use tact_kernel::tool::executor::SecurityService;

use super::policy_ext::PermissionPolicyExt;
use crate::security::sensitive::{Scanner, Tier, refusal_text};

/// Refuses a call whose target is credential material.
///
/// Deliberately a *guard*, not a decision: it runs before the PreToolUse hook
/// and before permission, so no mode, rule, hook or in-session grant can talk
/// it out of a refusal. `Secret` (non-credential) hits are not refused here —
/// [`PermissionService`](tact_kernel::tool::executor::PermissionService) is
/// expected to escalate them.
pub struct SensitivePathGuard {
    scanner: Scanner,
}

impl SensitivePathGuard {
    pub fn new(scanner: Scanner) -> Self {
        Self { scanner }
    }

    /// A guard with the built-in rules and the current user's home.
    pub fn builtin() -> Self {
        Self::new(Scanner::builtin())
    }
}

impl SecurityService for SensitivePathGuard {
    fn guard(&self, descriptor: &ToolDescriptor, input: &serde_json::Value) -> anyhow::Result<()> {
        let Some(hit) = descriptor.policy.permission.sensitive(input, &self.scanner) else {
            return Ok(());
        };
        if hit.tier == Tier::Credential {
            anyhow::bail!("{}", refusal_text(&hit));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tact_kernel::tool::metadata::{PermissionPolicy, ToolMetadata};

    fn descriptor_for(policy: PermissionPolicy) -> ToolDescriptor {
        let mut metadata = ToolMetadata::read_json("read_file", "Read", "Read");
        metadata.permission = policy;
        metadata.permission_prompt =
            tact_kernel::tool::PermissionPromptPolicy::Path { field: "path" };
        ToolDescriptor::from_native("read_file", "Read", json!({"type": "object"}), &metadata)
    }

    #[test]
    fn credential_path_is_refused() {
        let guard = SensitivePathGuard::builtin();
        let descriptor = descriptor_for(PermissionPolicy::ReadPath { path_field: "path" });
        let Err(error) = guard.guard(&descriptor, &json!({"path": ".ssh/id_ed25519"})) else {
            panic!("private keys are refused");
        };
        let text = error.to_string();
        assert!(text.contains("Refused"), "{text}");
        assert!(text.contains("sensitive_paths.allow"), "{text}");
    }

    #[test]
    fn absolute_targets_are_left_to_the_path_guard() {
        // `~`-rooted and `/`-rooted targets never reach this classifier: the
        // workspace-relative rule is what keeps `safe_path` handling them, and
        // duplicating that decision here would make a home-directory read look
        // like a credential refusal.
        let guard = SensitivePathGuard::builtin();
        let descriptor = descriptor_for(PermissionPolicy::ReadPath { path_field: "path" });
        guard
            .guard(&descriptor, &json!({"path": "~/.ssh/id_ed25519"}))
            .expect("absolute targets are not classified here");
    }

    #[test]
    fn ordinary_path_passes() {
        let guard = SensitivePathGuard::builtin();
        let descriptor = descriptor_for(PermissionPolicy::ReadPath { path_field: "path" });
        guard
            .guard(&descriptor, &json!({"path": "src/main.rs"}))
            .expect("source files are not credential material");
    }

    #[test]
    fn a_policy_without_a_target_never_refuses() {
        let guard = SensitivePathGuard::builtin();
        let descriptor = descriptor_for(PermissionPolicy::Read);
        guard
            .guard(&descriptor, &json!({"anything": 1}))
            .expect("no target means nothing to classify");
    }

    #[test]
    fn secret_tier_is_left_to_permission() {
        // `.env` is `Secret`, not `Credential`: refusing it here would make the
        // "asks in Default, denied in Plan" escalation unreachable.
        let guard = SensitivePathGuard::builtin();
        let descriptor = descriptor_for(PermissionPolicy::ReadPath { path_field: "path" });
        guard
            .guard(&descriptor, &json!({"path": ".env"}))
            .expect("Secret is escalated by permission, not refused by security");
    }
}
