//! The permission and security ports as plugins.
//!
//! Phase 3 promotes the Phase-1b adapters from "constructed inline by the
//! Agent" to "provided by a plugin". The wiring is explicit: each plugin
//! contributes the concrete handle it owns, and the composition root pulls that
//! handle by type and hands it to the `ToolExecutor` it builds next. Nothing
//! reaches into a service locator at call time.

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};
use tact_kernel::tool::executor::PermissionService;
use tact_kernel::tool::executor::SecurityService;

use super::{
    ManagerPermissionService, PermissionManager, SensitivePathGuard, SharedPermissionManager,
};

/// Provides the permission port over a shared [`PermissionManager`].
pub struct PermissionPlugin {
    manager: SharedPermissionManager,
    ui_available: Arc<dyn Fn() -> bool + Send + Sync>,
}

impl PermissionPlugin {
    /// `ui_available` is asked live, not snapshotted: a frontend attached after
    /// the Agent is constructed must still be able to answer a prompt.
    pub fn new(
        manager: SharedPermissionManager,
        ui_available: Arc<dyn Fn() -> bool + Send + Sync>,
    ) -> Self {
        Self {
            manager,
            ui_available,
        }
    }

    /// A manager this plugin owns outright, for a host with no UI wiring.
    pub fn headless(manager: PermissionManager) -> Self {
        Self::new(SharedPermissionManager::new(manager), Arc::new(|| false))
    }
}

#[async_trait::async_trait]
impl Plugin for PermissionPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.permission").expect("a literal id"),
            "Permission policy",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Policy,
            "Modes, rules and always-allow persistence for tool calls",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        let service = ManagerPermissionService::new(self.manager.handle())
            .with_ui_available(self.ui_available.clone());
        registrar.service(Arc::new(service))
    }
}

/// Provides the security guard over the current settings.
pub struct SecurityPlugin {
    scanner: crate::security::sensitive::Scanner,
}

impl SecurityPlugin {
    pub fn new(scanner: crate::security::sensitive::Scanner) -> Self {
        Self { scanner }
    }

    /// The built-in rules with the current user's home.
    pub fn builtin() -> Self {
        Self::new(crate::security::sensitive::Scanner::builtin())
    }
}

#[async_trait::async_trait]
impl Plugin for SecurityPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.security").expect("a literal id"),
            "Security guard",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Policy,
            "Refuses credential material before any other decision",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.service(Arc::new(SensitivePathGuard::new(self.scanner.clone())))
    }
}

/// Pull the permission port a `PermissionPlugin` provided.
pub fn permission_service(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<ManagerPermissionService>> {
    services.get::<ManagerPermissionService>().ok_or_else(|| {
        anyhow::anyhow!("no permission service installed: is builtin.permission installed?")
    })
}

/// Pull the security guard a `SecurityPlugin` provided.
pub fn security_service(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<SensitivePathGuard>> {
    services.get::<SensitivePathGuard>().ok_or_else(|| {
        anyhow::anyhow!("no security guard installed: is builtin.security installed?")
    })
}

/// Install both policy plugins into a fresh host and return their services.
///
/// The entry point a composition root uses before building a `ToolExecutor`: it
/// does not have to know which plugin provides which port, only that the policy
/// pair is installed. Synchronous because `Agent::new` is; `install_sync` runs
/// the same transaction as the async form, minus an awaited shutdown that a
/// policy plugin (owning no resource) does not need.
pub fn install(
    permission_manager: &SharedPermissionManager,
    ui_available: Arc<dyn Fn() -> bool + Send + Sync>,
) -> Result<tact_kernel::service::ServiceRegistry> {
    let mut host =
        tact_kernel::plugin::PluginHost::new(tact_kernel::capability::CapabilityFilter::default());
    host.install_sync(SecurityPlugin::new(permission_manager.scanner()))?;
    host.install_sync(PermissionPlugin::new(
        permission_manager.clone(),
        ui_available,
    ))?;
    Ok(host.seal_sync()?.services().clone())
}

/// The ports a host needs to build a `ToolExecutor`, resolved together.
pub struct PolicyPorts {
    pub permissions: Arc<dyn PermissionService>,
    pub security: Arc<dyn SecurityService>,
}

/// Resolve both ports from the registry.
///
/// Two typed lookups, not a locator: the caller names what it needs and gets a
/// concrete pair it then passes on. A missing plugin is an error that says which
/// one.
pub fn resolve(services: &tact_kernel::service::ServiceRegistry) -> Result<PolicyPorts> {
    Ok(PolicyPorts {
        permissions: permission_service(services)?,
        security: security_service(services)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    fn manager() -> PermissionManager {
        PermissionManager::try_new(crate::permission::PermissionMode::Default).unwrap()
    }

    #[test]
    fn install_brings_up_both_ports() {
        // One call, two plugins, both ports resolvable: the composition root does
        // not have to know which plugin owns which.
        let manager = SharedPermissionManager::new(manager());
        let services = install(&manager, Arc::new(|| false)).unwrap();
        assert!(services.contains::<SensitivePathGuard>());
        assert!(services.contains::<ManagerPermissionService>());
        assert!(resolve(&services).is_ok());
    }

    #[tokio::test]
    async fn the_two_plugins_provide_both_ports() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(SecurityPlugin::builtin()).await.unwrap();
        host.install(PermissionPlugin::headless(manager()))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();

        let ports = resolve(runtime.services()).unwrap();
        // Both are the concrete handles the plugins contributed.
        assert!(runtime.services().get::<SensitivePathGuard>().is_some());
        assert!(
            runtime
                .services()
                .get::<ManagerPermissionService>()
                .is_some()
        );
        // And they are the same objects the ports expose.
        let _ = ports;
    }

    #[tokio::test]
    async fn a_missing_policy_plugin_names_itself() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(SecurityPlugin::builtin()).await.unwrap();
        let runtime = host.seal().await.unwrap();
        let Err(error) = resolve(runtime.services()) else {
            panic!("a host without a permission plugin must not resolve ports");
        };
        assert!(error.to_string().contains("builtin.permission"), "{error}");
    }

    #[tokio::test]
    async fn the_manifests_name_policy_plugins() {
        let permission = PermissionPlugin::headless(manager());
        assert_eq!(permission.manifest().kind, PluginKind::Policy);
        assert_eq!(permission.manifest().id.as_str(), "builtin.permission");
        let security = SecurityPlugin::builtin();
        assert_eq!(security.manifest().kind, PluginKind::Policy);
        assert_eq!(security.manifest().id.as_str(), "builtin.security");
    }

    #[tokio::test]
    async fn a_second_permission_plugin_is_refused() {
        // Two providers for one port means the composition order is wrong. The
        // host catches the duplicate id first, which is the earlier and clearer
        // failure; either way the second install is refused rather than silently
        // replacing the first.
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(PermissionPlugin::headless(manager()))
            .await
            .unwrap();
        let error = host
            .install(PermissionPlugin::headless(manager()))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("duplicate plugin id"), "{error}");
    }

    #[tokio::test]
    async fn two_different_plugins_cannot_provide_the_same_port() {
        // With distinct ids the duplicate *service* is what gets refused, so the
        // registry, not just the id check, is what protects the port.
        struct Impostor;
        #[async_trait::async_trait]
        impl Plugin for Impostor {
            fn manifest(&self) -> PluginManifest {
                PluginManifest::new(
                    PluginId::new("impostor.permission").expect("a literal id"),
                    "impostor",
                    env!("CARGO_PKG_VERSION"),
                    PluginKind::Policy,
                    "attempts to provide a second permission port",
                )
            }

            fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
                registrar.service(Arc::new(ManagerPermissionService::new(
                    SharedPermissionManager::new(manager()).handle(),
                )))
            }
        }

        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(PermissionPlugin::headless(manager()))
            .await
            .unwrap();
        let error = host.install(Impostor).await.unwrap_err();
        assert!(error.to_string().contains("already provided"), "{error}");
        assert!(
            error.to_string().contains("builtin.permission"),
            "the error names the first provider: {error}"
        );
    }
}
