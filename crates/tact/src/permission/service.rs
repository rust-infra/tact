//! The kernel `PermissionService` port, backed by [`PermissionManager`].
//!
//! The mapping is deliberately thin: `PermissionManager` stays the single
//! source of truth for modes, rules, always-allow persistence and the
//! plan-mode nudge. This adapter only translates its verdict into the kernel's
//! option-shaped prompt, and routes the chosen option back to the manager's
//! own recording APIs.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use tact_kernel::tool::descriptor::ToolDescriptor;
use tact_kernel::tool::executor::{
    PermissionAnswer, PermissionDecision, PermissionOption, PermissionOptionKind, PermissionPrompt,
    PermissionRequest, PermissionService,
};

use super::policy_ext::PermissionPolicyExt;
use super::{PermissionBehavior, PermissionManager, PermissionMode};
use crate::permission::format_permission_prompt;

/// The popup's options, in the order the popup shows them.
///
/// Order is load-bearing: `Deny` stays second and "always allow this tool"
/// third, because those two positions are what the muscle memory of existing
/// users expects. The pattern option is appended.
const OPTIONS: [&str; 4] = [
    "Allow once",
    "Deny",
    "Always allow this tool",
    "Always allow this pattern",
];

/// Whether this call can offer the program-level option at all.
fn pattern_rule_available(descriptor: &ToolDescriptor, input: &serde_json::Value) -> bool {
    PermissionManager::prefix_rule_for(
        descriptor.name.as_str(),
        descriptor.policy.permission_prompt,
        input,
    )
    .is_some()
}

/// Build the kernel prompt for a call the manager decided to ask about.
fn build_prompt(
    descriptor: &ToolDescriptor,
    input: &serde_json::Value,
    reason: &str,
) -> PermissionPrompt {
    let with_pattern = pattern_rule_available(descriptor, input);
    let mut body =
        format_permission_prompt(&descriptor.name, descriptor.policy.permission_prompt, input);
    if let Some(rule) = PermissionManager::prefix_rule_for(
        &descriptor.name,
        descriptor.policy.permission_prompt,
        input,
    ) {
        body.push_str(&format!(
            "\n\n\"Always allow this pattern\" would record: {}",
            rule.to_rule_string()
        ));
    }
    if !reason.is_empty() {
        body.push_str(&format!("\n\n{reason}"));
    }

    let mut options = Vec::new();
    for (index, label) in OPTIONS.iter().enumerate() {
        let kind = match index {
            0 => PermissionOptionKind::AllowOnce,
            1 => PermissionOptionKind::Deny,
            2 => PermissionOptionKind::AlwaysAllow,
            _ => PermissionOptionKind::AlwaysAllowWide,
        };
        if kind == PermissionOptionKind::AlwaysAllowWide && !with_pattern {
            continue;
        }
        options.push(PermissionOption {
            id: match kind {
                PermissionOptionKind::AllowOnce => "allow_once",
                PermissionOptionKind::Deny => "deny",
                PermissionOptionKind::AlwaysAllow => "always_allow",
                PermissionOptionKind::AlwaysAllowWide => "always_allow_pattern",
            }
            .to_string(),
            label: (*label).to_string(),
            detail: None,
            kind,
        });
    }

    PermissionPrompt {
        message: body,
        options,
    }
}

/// A [`PermissionService`] over a shared [`PermissionManager`].
///
/// The manager is behind a mutex because it keeps denial counters and the
/// in-session always-allow list; the kernel may call the port from any task.
pub struct ManagerPermissionService {
    manager: Arc<Mutex<PermissionManager>>,
    auto_approved: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ui_available: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl ManagerPermissionService {
    pub fn new(manager: Arc<Mutex<PermissionManager>>) -> Self {
        Self {
            manager,
            auto_approved: Arc::new(|_| false),
            // Default to "a prompt can be shown", so a caller that wires no UI
            // fails at the prompt instead of silently auto-approving. A host
            // must supply the live answer through `with_ui_available`: the
            // frontend is usually attached *after* construction, so a value
            // captured here would report the wrong thing.
            ui_available: Arc::new(|| true),
        }
    }

    /// Also honour an MCP entry's `approval_mode: "auto"`.
    pub fn with_auto_approved(
        mut self,
        auto_approved: Arc<dyn Fn(&str) -> bool + Send + Sync>,
    ) -> Self {
        self.auto_approved = auto_approved;
        self
    }

    /// Declare whether a prompt can reach a human.
    ///
    /// When it cannot, an `Ask` is resolved by
    /// [`PermissionManager::ask_user`] — the same non-interactive ladder the
    /// agent used before the port existed (High denied, Write allowed once).
    pub fn with_ui_available(mut self, ui_available: Arc<dyn Fn() -> bool + Send + Sync>) -> Self {
        self.ui_available = ui_available;
        self
    }
}

#[async_trait]
impl PermissionService for ManagerPermissionService {
    async fn decide(&self, request: PermissionRequest) -> anyhow::Result<PermissionDecision> {
        let risk = request.descriptor.policy.permission.resolve(&request.input);
        // The descriptor's own bit first (an MCP entry's `approval_mode: auto`),
        // then the host's predicate, so a host can add policy without a source
        // having to declare it.
        let auto = request.descriptor.policy.auto_approved
            || (self.auto_approved)(&request.descriptor.name);
        let mut manager = self
            .manager
            .lock()
            .map_err(|_| anyhow::anyhow!("permission manager lock poisoned"))?;
        let decision =
            manager.check_with_auto(&request.descriptor.name, risk, &request.input, auto);
        Ok(match decision.behavior {
            PermissionBehavior::Allow => PermissionDecision::Allow,
            PermissionBehavior::Deny => PermissionDecision::Deny(decision.reason),
            PermissionBehavior::Ask if !(self.ui_available)() => {
                // No human can answer. Fall back to the documented headless
                // ladder rather than deciding for them: High is refused, and a
                // Write is allowed once (a Read never reaches `Ask`).
                if manager.ask_user(&request.descriptor.name, risk)? {
                    PermissionDecision::Allow
                } else {
                    PermissionDecision::Deny(format!(
                        "{} (no interactive UI available)",
                        decision.reason
                    ))
                }
            }
            PermissionBehavior::Ask => PermissionDecision::Ask(build_prompt(
                &request.descriptor,
                &request.input,
                &decision.reason,
            )),
        })
    }

    async fn record(
        &self,
        request: &PermissionRequest,
        answer: &PermissionAnswer,
    ) -> anyhow::Result<()> {
        let mut manager = self
            .manager
            .lock()
            .map_err(|_| anyhow::anyhow!("permission manager lock poisoned"))?;
        match answer.kind {
            PermissionOptionKind::AllowOnce => {
                manager.record_allow_once();
            }
            PermissionOptionKind::Deny => {
                manager.record_denial(&request.descriptor.name);
            }
            PermissionOptionKind::AlwaysAllow => {
                manager.allow_tool_with_input(
                    &request.descriptor.name,
                    request.descriptor.policy.permission_prompt,
                    &request.input,
                );
            }
            PermissionOptionKind::AlwaysAllowWide => {
                manager.allow_tool_as_prefix(
                    &request.descriptor.name,
                    request.descriptor.policy.permission_prompt,
                    &request.input,
                );
            }
        }
        Ok(())
    }
}

/// Whether the manager is in plan mode.
pub fn is_plan_mode(manager: &PermissionManager) -> bool {
    manager.mode() == PermissionMode::Plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tact_kernel::tool::metadata::{PermissionPolicy, ToolMetadata};

    fn descriptor() -> ToolDescriptor {
        let mut metadata = ToolMetadata::read_json("write_file", "Write", "Write File");
        metadata.permission = PermissionPolicy::WritePath { path_field: "path" };
        metadata.permission_prompt =
            tact_kernel::tool::PermissionPromptPolicy::Path { field: "path" };
        ToolDescriptor::from_native("write_file", "Write", json!({"type": "object"}), &metadata)
    }

    fn service(mode: PermissionMode) -> ManagerPermissionService {
        ManagerPermissionService::new(Arc::new(Mutex::new(
            PermissionManager::try_new(mode).unwrap(),
        )))
    }

    #[tokio::test]
    async fn read_risk_is_allowed_without_a_prompt() {
        let service = service(PermissionMode::Default);
        let mut descriptor = descriptor();
        descriptor.policy.permission = PermissionPolicy::Read;
        let decision = service
            .decide(PermissionRequest {
                descriptor,
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn write_in_default_mode_asks_with_stable_option_ids() {
        let service = service(PermissionMode::Default);
        let decision = service
            .decide(PermissionRequest {
                descriptor: descriptor(),
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        let PermissionDecision::Ask(prompt) = decision else {
            panic!("Default mode must ask for a write");
        };
        let ids: Vec<_> = prompt
            .options
            .iter()
            .map(|option| option.id.as_str())
            .collect();
        // The three muscle-memory positions are stable; the pattern option is
        // appended only when a rule for it can be built.
        assert_eq!(&ids[..3], &["allow_once", "deny", "always_allow"]);
        assert!(ids.len() <= 4);
        assert!(prompt.message.contains("src/main.rs"), "{}", prompt.message);
    }

    #[tokio::test]
    async fn plan_mode_denies_writes() {
        let service = service(PermissionMode::Plan);
        let decision = service
            .decide(PermissionRequest {
                descriptor: descriptor(),
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        assert!(matches!(decision, PermissionDecision::Deny(_)));
    }

    /// A source that declares `approval_mode: auto` skips the prompt but does
    /// not lower the risk.
    ///
    /// The distinction matters: `Read` is allowed before plan mode is consulted,
    /// so mapping auto-approve onto `Read` would let an auto-approved MCP write
    /// run in plan mode.
    #[tokio::test]
    async fn a_descriptor_auto_approved_skips_the_prompt_but_not_plan_mode() {
        let mut declared = descriptor();
        declared.policy.auto_approved = true;

        let default_mode = service(PermissionMode::Default);
        assert_eq!(
            default_mode
                .decide(PermissionRequest {
                    descriptor: declared.clone(),
                    input: json!({"path": "src/main.rs"}),
                })
                .await
                .unwrap(),
            PermissionDecision::Allow,
            "auto-approved must not prompt"
        );

        let plan_mode = service(PermissionMode::Plan);
        let decided = plan_mode
            .decide(PermissionRequest {
                descriptor: declared,
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        assert!(
            matches!(decided, PermissionDecision::Deny(_)),
            "auto-approve must not unlock plan mode, got {decided:?}"
        );
    }

    #[tokio::test]
    async fn auto_approved_skips_the_prompt() {
        let service = service(PermissionMode::Default)
            .with_auto_approved(Arc::new(|name: &str| name == "write_file"));
        let decision = service
            .decide(PermissionRequest {
                descriptor: descriptor(),
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        assert_eq!(decision, PermissionDecision::Allow);
    }

    #[tokio::test]
    async fn always_allow_records_a_narrow_rule() {
        let manager = Arc::new(Mutex::new(
            PermissionManager::try_new(PermissionMode::Default).unwrap(),
        ));
        let service = ManagerPermissionService::new(manager.clone());
        let request = PermissionRequest {
            descriptor: descriptor(),
            input: json!({"path": "src/main.rs"}),
        };
        service
            .record(
                &request,
                &PermissionAnswer {
                    option_id: "always_allow".into(),
                    kind: PermissionOptionKind::AlwaysAllow,
                },
            )
            .await
            .unwrap();
        let decision = service.decide(request).await.unwrap();
        assert_eq!(decision, PermissionDecision::Allow);
    }

    /// The popup's indices are load-bearing — the UI answers with a position —
    /// so the list and the kinds it maps onto are pinned, not inferred.
    #[tokio::test]
    async fn popup_indices_map_to_their_option_kinds() {
        let service = service(PermissionMode::Default);
        let decision = service
            .decide(PermissionRequest {
                descriptor: descriptor(),
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        let PermissionDecision::Ask(prompt) = decision else {
            panic!("a write in Default mode must ask");
        };
        let kinds: Vec<_> = prompt.options.iter().map(|option| option.kind).collect();
        assert_eq!(kinds[0], PermissionOptionKind::AllowOnce);
        assert_eq!(kinds[1], PermissionOptionKind::Deny);
        assert_eq!(kinds[2], PermissionOptionKind::AlwaysAllow);
        // The wide option is appended, never inserted: gating it out cannot move
        // an index a user has already learned.
        assert!(kinds.len() == 3 || kinds[3] == PermissionOptionKind::AlwaysAllowWide);
        assert_eq!(
            prompt.options.last().map(|option| option.label.as_str()),
            if kinds.len() == 4 {
                Some("Always allow this pattern")
            } else {
                Some("Always allow this tool")
            }
        );
    }

    #[tokio::test]
    async fn headless_write_is_allowed_once_and_high_is_denied() {
        let service = service(PermissionMode::Default).with_ui_available(Arc::new(|| false));

        let write = service
            .decide(PermissionRequest {
                descriptor: descriptor(),
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        assert_eq!(
            write,
            PermissionDecision::Allow,
            "headless allows a write once"
        );

        let mut high = descriptor();
        high.policy.permission = PermissionPolicy::High;
        let denied = service
            .decide(PermissionRequest {
                descriptor: high,
                input: json!({"path": "src/main.rs"}),
            })
            .await
            .unwrap();
        let PermissionDecision::Deny(reason) = denied else {
            panic!("headless must refuse a high-risk call");
        };
        assert!(reason.contains("no interactive UI"), "{reason}");
    }

    #[tokio::test]
    async fn deny_records_a_denial_for_the_plan_mode_nudge() {
        let manager = Arc::new(Mutex::new(
            PermissionManager::try_new(PermissionMode::Default).unwrap(),
        ));
        let service = ManagerPermissionService::new(manager.clone());
        let request = PermissionRequest {
            descriptor: descriptor(),
            input: json!({"path": "src/main.rs"}),
        };
        for _ in 0..3 {
            service
                .record(
                    &request,
                    &PermissionAnswer {
                        option_id: "deny".into(),
                        kind: PermissionOptionKind::Deny,
                    },
                )
                .await
                .unwrap();
        }
        assert!(manager.lock().unwrap().should_suggest_plan_mode());
    }
}
