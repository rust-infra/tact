//! Frontend permission policy: the mode a session runs in, and the policies its
//! capability invocations are decided by.
//!
//! Two policies live here because they answer two different questions about one
//! capability call. The agent loop's per-tool gate is the session's
//! [`PermissionManager`]; the serving context (the router a plugin's Kernel
//! service calls arrive on) needs a `PermissionService` that keeps the *host's*
//! control plane working while deciding everything else the same way the
//! session does.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;
use tact::{InvocationContext, KernelError, PermissionService};
use tact_extensions::permission::{
    PermissionManager, PermissionManagerService, PermissionMode, settings::PermissionSettings,
};
use tact_protocol::{CapabilityDeclaration, InteractionRequest, InteractionResponse};

pub(crate) fn permission_mode_from_config() -> PermissionMode {
    match tact_extensions::config::settings()
        .permission_mode
        .as_deref()
    {
        Some("plan") => PermissionMode::Plan,
        Some("default") => PermissionMode::Default,
        _ => PermissionMode::Auto,
    }
}

/// The plugin identities the host's own control plane invokes as.
///
/// These are the extensions that own the host-driven capabilities: Chat owns
/// `chat.submit` / `chat.compact`, the Agent owns `runs.start` / `runs.cancel`.
/// The host speaks as the owning extension of the capability it is invoking
/// (the same convention `runs.*` and `chat.compact` already followed), so a
/// host call is one of these two and nothing else. Membership is the whole
/// check — a caller that is not one of the host's identities is refused even
/// for a control capability name.
pub(crate) const HOST_CONTROL_PLANE_IDENTITIES: [&str; 2] = ["tact.chat", "tact.agent"];

/// The host's permission policy for its **own** control-plane calls.
///
/// `CapabilityRouter::invoke` always runs `PermissionService::check`, and the
/// host has no guarded path to its own turn: `chat.submit` submits the
/// conversational turn the user asked for, `chat.compact` compacts the
/// conversation the user asked to compact, and `runs.start` / `runs.cancel` are
/// the run that turn executes and the process stopping it. All are the host
/// acting on the user's own request, not a capability the model asked for.
/// The general policy, [`PermissionManagerService`], is fail-closed in `Ask`
/// mode: with no responder it denies. That would break a headless run for any
/// user on `mode = default`, because submitting the turn would be gated on a
/// prompt the process cannot answer.
///
/// So this policy allows exactly those names, and only when the caller is one
/// of the host's own identities ([`HOST_CONTROL_PLANE_IDENTITIES`]); every
/// other caller and every other capability is denied. It is **not** a per-tool
/// bypass: tool invocations never pass through it — they are authorized by the
/// Agent's own permission manager and the preflight gate in tool dispatch. The
/// narrow, deny-by-default shape keeps that honest: a plugin holding the
/// serving router cannot inject a turn, fire hooks/notifications, or rewrite
/// the session history by invoking a control capability under its own name.
pub(crate) struct HostControlPlanePermission;

#[async_trait]
impl PermissionService for HostControlPlanePermission {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        context: &InvocationContext,
        _input: &Value,
    ) -> Result<(), KernelError> {
        match declaration.name.as_str() {
            "chat.submit" | "chat.compact" | "runs.start" | "runs.cancel" => {
                if HOST_CONTROL_PLANE_IDENTITIES.contains(&context.plugin_id().as_str()) {
                    Ok(())
                } else {
                    Err(KernelError::permission_denied(format!(
                        "{} is a host control capability; only the host's own identities \
                         ({}) may invoke it, not {}",
                        declaration.name,
                        HOST_CONTROL_PLANE_IDENTITIES.join(", "),
                        context.plugin_id(),
                    )))
                }
            }
            other => Err(KernelError::permission_denied(format!(
                "the host only authorizes its own control capabilities; \
                 {other} is not one"
            ))),
        }
    }
}

/// The permission policy the session's serving context applies.
///
/// Two decisions meet here, and conflating them would undo one of the other:
///
/// - The host's **own control plane** (`chat.submit` / `chat.compact` /
///   `runs.start` / `runs.cancel`) is decided by [`HostControlPlanePermission`],
///   so the process can always submit, compact and stop the turn it is hosting
///   regardless of the configured mode.
/// - **Everything else** — the Kernel's `storage.*` / `events.*` /
///   `trajectory.*` / `permission.request` / `interaction.request`, the
///   `sessions.*` extension, and any plugin capability — is decided by the same
///   configured policy the session's tools use. In `ask` mode that check fails
///   closed, and that is the intended answer: there is no interactive responder
///   for a plugin-initiated capability, so an unattended "ask" is a denial.
///
/// This is not a tool bypass. Tool capabilities never reach the serving router:
/// the wave keeps its own router and its own preflight gate, so nothing here can
/// widen what a tool may do.
struct ServingPermission {
    policy: PermissionManagerService,
    control_plane: HostControlPlanePermission,
}

#[async_trait]
impl PermissionService for ServingPermission {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        context: &InvocationContext,
        input: &Value,
    ) -> Result<(), KernelError> {
        match declaration.name.as_str() {
            "chat.submit" | "chat.compact" | "runs.start" | "runs.cancel" => {
                self.control_plane.check(declaration, context, input).await
            }
            _ => self.policy.check(declaration, context, input).await,
        }
    }

    /// Answers a `permission.request` with the policy's own responder.
    ///
    /// The session's policy is built without a responder, so this fails closed:
    /// a capability that asks the *policy* for a decision gets an explicit
    /// denial rather than a hang.
    async fn request(
        &self,
        request: InteractionRequest,
        context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError> {
        self.policy.request(request, context).await
    }
}

/// Builds the serving context's policy from the session's mode and settings.
///
/// The settings are cloned rather than shared because the Agent's own
/// [`PermissionManager`] owns its copy: a policy decision made here must be the
/// same decision the session's tools would get, not a second, diverging one.
pub(crate) fn serving_permission(
    mode: PermissionMode,
    settings: PermissionSettings,
) -> anyhow::Result<Arc<dyn PermissionService>> {
    let manager = PermissionManager::try_new_with_settings(mode, settings)?;
    Ok(Arc::new(ServingPermission {
        policy: PermissionManagerService::new(manager),
        control_plane: HostControlPlanePermission,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{CapabilityKind, CapabilityRisk, PluginId, RequestId};

    fn declaration(name: &str, risk: CapabilityRisk) -> CapabilityDeclaration {
        CapabilityDeclaration {
            name: name.into(),
            kind: CapabilityKind::Service,
            version: "1".into(),
            description: None,
            input_schema: None,
            output_schema: None,
            risk,
        }
    }

    fn context() -> InvocationContext {
        InvocationContext::new(
            RequestId::from("permission-probe"),
            PluginId::from("tact.agent"),
            "headless",
        )
    }

    /// The host's control-plane policy allows its own capabilities and denies
    /// everything else — it is not a blanket allow.
    #[tokio::test]
    async fn the_host_control_plane_policy_allows_only_the_run_capabilities() {
        let check = |name: &str| {
            let declaration = declaration(name, CapabilityRisk::Medium);
            let context = context();
            async move {
                HostControlPlanePermission
                    .check(&declaration, &context, &serde_json::json!({}))
                    .await
            }
        };

        assert!(check("chat.submit").await.is_ok());
        assert!(check("chat.compact").await.is_ok());
        assert!(check("runs.start").await.is_ok());
        assert!(check("runs.cancel").await.is_ok());
        let denied = check("bash").await.expect_err("a tool is not allowed");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );
    }

    /// The allowance is scoped to the host: both of the host's own identities
    /// pass, for every control capability.
    #[tokio::test]
    async fn the_host_control_plane_policy_allows_the_host_identities() {
        for plugin in HOST_CONTROL_PLANE_IDENTITIES {
            for name in ["chat.submit", "chat.compact", "runs.start", "runs.cancel"] {
                let declaration = declaration(name, CapabilityRisk::Medium);
                let context = InvocationContext::new(
                    RequestId::from("permission-probe"),
                    PluginId::from(plugin),
                    "the host",
                );
                HostControlPlanePermission
                    .check(&declaration, &context, &serde_json::json!({}))
                    .await
                    .unwrap_or_else(|error| {
                        panic!(
                            "the host identity {plugin} may invoke {name}: {}",
                            error.message()
                        )
                    });
            }
        }
    }

    /// An unrelated plugin holding the router cannot invoke the host's control
    /// plane by name alone: without a permission check it could inject a turn,
    /// fire hooks and notifications, or rewrite the session history.
    #[tokio::test]
    async fn the_host_control_plane_policy_refuses_an_unrelated_plugin() {
        for name in ["chat.submit", "chat.compact", "runs.start", "runs.cancel"] {
            let declaration = declaration(name, CapabilityRisk::Medium);
            let context = InvocationContext::new(
                RequestId::from("permission-probe"),
                PluginId::from("demo.plugin"),
                "a plugin",
            );
            let denied = HostControlPlanePermission
                .check(&declaration, &context, &serde_json::json!({}))
                .await
                .unwrap_err();
            assert_eq!(
                denied.category(),
                tact_protocol::ErrorCategory::PermissionDenied,
                "{name} must be refused for an unrelated plugin"
            );
        }
    }

    /// A plugin id that only *looks* like a host id is not one: the check is
    /// exact membership, not a prefix.
    #[tokio::test]
    async fn the_host_control_plane_policy_is_not_fooled_by_a_lookalike_plugin() {
        let declaration = declaration("chat.submit", CapabilityRisk::Medium);
        let context = InvocationContext::new(
            RequestId::from("permission-probe"),
            PluginId::from("tact.chat.evil"),
            "a lookalike",
        );
        let denied = HostControlPlanePermission
            .check(&declaration, &context, &serde_json::json!({}))
            .await
            .expect_err("a lookalike plugin id is not the host");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );
    }

    /// The serving policy keeps the host's run capability available in `ask`
    /// mode while deciding everything else by the configured policy.
    ///
    /// This is the shape that lets the run and the Kernel services share one
    /// router: `runs.start` must not be gated on a prompt headless cannot
    /// answer, while `storage.set` — a capability something *else* asked for —
    /// is denied without a responder, exactly as `ask` mode intends.
    #[tokio::test]
    async fn the_serving_policy_keeps_the_run_available_and_fails_closed_otherwise() {
        // Loaded with no global layer so a developer's own `~/.tact` rules
        // cannot decide what this test asserts.
        let directory = tempfile::tempdir().expect("temp directory");
        let settings = PermissionSettings::load_from(&directory.path().join("settings.json"), None);
        let policy = serving_permission(PermissionMode::Default, settings).expect("policy");

        policy
            .check(
                &declaration("runs.start", CapabilityRisk::Medium),
                &context(),
                &serde_json::json!({}),
            )
            .await
            .expect("the host's own run must start in ask mode");

        policy
            .check(
                &declaration("chat.submit", CapabilityRisk::Medium),
                &context(),
                &serde_json::json!({}),
            )
            .await
            .expect("the host's own turn must be submittable in ask mode");

        let denied = policy
            .check(
                &declaration("storage.set", CapabilityRisk::Medium),
                &context(),
                &serde_json::json!({}),
            )
            .await
            .expect_err("a plugin capability must fail closed in ask mode");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );

        // In `auto` the same capability is allowed, so the failure above is the
        // mode's decision and not an unconditional refusal.
        let directory = tempfile::tempdir().expect("temp directory");
        let settings = PermissionSettings::load_from(&directory.path().join("settings.json"), None);
        let policy = serving_permission(PermissionMode::Auto, settings).expect("policy");
        policy
            .check(
                &declaration("storage.set", CapabilityRisk::Medium),
                &context(),
                &serde_json::json!({}),
            )
            .await
            .expect("auto mode allows a write capability");
    }
}
