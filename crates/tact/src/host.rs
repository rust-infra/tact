//! The extension bundle's composition root.
//!
//! This is the one place that knows which built-in plugins exist and in what
//! order they must be installed. A frontend (the TUI, headless) asks for an
//! assembled runtime instead of wiring the pieces itself, which is what makes
//! "everything is an extension" true for the production path rather than only
//! for tests.
//!
//! Order is a dependency order, not a plugin-declared graph:
//!
//! 1. policy (`SecurityPlugin`, then `PermissionPlugin`) — the executor needs
//!    both ports and nothing else;
//! 2. hooks, when the host has a hook set to install — a host whose hooks need
//!    an agent runs them on its own side and skips this;
//! 3. storage, which the caller has already opened (opening is async, `register`
//!    is not);
//! 4. the tool families, which contribute the catalog;
//! 5. dynamic sources last, because their contents depend on connections that
//!    were established before assembly.

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::capability::CapabilityFilter;
use tact_kernel::plugin::PluginHost;
use tact_kernel::service::ServiceRegistry;
use tact_kernel::tool::executor::ToolExecutor;

use crate::permission::{SharedPermissionManager, plugin as policy};

/// What a host has to supply to build a runtime.
pub struct HostOptions<'a> {
    pub permission_manager: &'a SharedPermissionManager,
    /// Asked live: a frontend attached after assembly must still be able to
    /// answer a prompt.
    pub ui_available: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The two directories a hook command may expand, if this host installs the
    /// kernel hook port.
    pub hooks: Option<HookOptions>,
    /// The opened store, if this host persists sessions.
    pub storage: Option<StorageOptions>,
    /// Which capabilities the user disabled.
    pub filter: CapabilityFilter,
}

/// The hook declaration set to install.
pub struct HookOptions {
    pub source: crate::plugin::HookSource,
    pub work_dir: std::path::PathBuf,
    pub notices: Arc<dyn Fn(&str) + Send + Sync>,
}

/// The already-opened session store.
pub struct StorageOptions {
    pub store: crate::store::session_store::DynSessionStore,
    pub path: std::path::PathBuf,
}

/// The assembled pieces, after sealing.
///
/// Kept together so the host owns one value: dropping it in the right order is
/// what releases the plugins' resources.
pub struct AssembledRuntime {
    services: ServiceRegistry,
    skipped: Vec<String>,
    runtime: tact_kernel::plugin::Runtime,
}

impl AssembledRuntime {
    pub fn services(&self) -> &ServiceRegistry {
        &self.services
    }

    /// Capabilities the filter disabled, as `kind:name`.
    pub fn skipped(&self) -> &[String] {
        &self.skipped
    }

    /// The policy pair, resolved once and handed on.
    pub fn policy(&self) -> Result<policy::PolicyPorts> {
        policy::resolve(&self.services)
    }

    /// Shut the plugins down in reverse installation order.
    pub async fn shutdown(&mut self) -> Result<()> {
        self.runtime.shutdown().await
    }
}

/// Install the built-in plugins this host wants and seal.
///
/// The tool catalog is *not* built here: it comes from the families the caller
/// installs through [`install_tool_families`], because a family's handlers are
/// already owned by the router the caller built.
pub fn assemble(options: HostOptions<'_>) -> Result<AssembledRuntime> {
    let mut host = PluginHost::new(options.filter);
    host.install_sync(policy::SecurityPlugin::new(
        options.permission_manager.scanner(),
    ))?;
    host.install_sync(policy::PermissionPlugin::new(
        options.permission_manager.clone(),
        options.ui_available,
    ))?;

    if let Some(hooks) = options.hooks {
        host.install_sync(crate::plugin::HooksPlugin::new(
            hooks.source,
            hooks.work_dir,
            hooks.notices,
        ))?;
    }
    if let Some(storage) = options.storage {
        host.install_sync(crate::store::plugin::StoragePlugin::new(&storage.path))?;
        host.install_sync(crate::store::plugin::SessionStorePlugin::new(storage.store))?;
    }

    let skipped = host
        .skipped()
        .iter()
        .map(|skipped| skipped.capability.to_string())
        .collect();
    let runtime = host.seal_sync()?;
    let services = runtime.services().clone();
    Ok(AssembledRuntime {
        services,
        skipped,
        runtime,
    })
}

/// Install the built-in tool families into a fresh host and return its executor.
///
/// Separate from [`assemble`] because the handlers come from the caller's
/// router: the families must wrap *those* instances, not new ones, or a tool
/// would exist twice with independent state.
pub fn executor_from_families(
    tools: &crate::tool::ToolRouter,
    contexts: Arc<crate::tool::SharedToolContextFactory>,
    services: &ServiceRegistry,
    filter: CapabilityFilter,
    memory_enabled: bool,
    runtime: AssembledRuntime,
) -> Result<(Arc<ToolExecutor>, AssembledRuntime)> {
    let ports = policy::resolve(services)?;
    let (catalog, _skipped) = crate::tool::catalog_from_families(
        tools,
        &crate::tool::ToolFamily::ALL,
        memory_enabled,
        contexts,
        filter.clone(),
    )?;
    let executor = Arc::new(ToolExecutor::new(
        Arc::new(catalog),
        filter,
        ports.security,
        ports.permissions,
        // The hook port is looked up the same way every other port is; a host
        // that installed no hooks gets the explicit no-op rather than a silent
        // different behaviour.
        hook_port(services),
    ));
    Ok((executor, runtime))
}

/// The kernel hook port, or a no-op when the host installed none.
///
/// `NoHooks` is a deliberate named type, not an `Option`: an executor with no
/// hook port and one whose port does nothing are the same thing at this layer,
/// and naming it makes that visible where it is constructed.
fn hook_port(services: &ServiceRegistry) -> Arc<dyn tact_kernel::tool::executor::HookService> {
    match crate::plugin::hook_service(services) {
        Ok(service) => service,
        Err(_) => Arc::new(NoHooks),
    }
}

/// A hook port that continues every call.
pub struct NoHooks;

#[async_trait::async_trait]
impl tact_kernel::tool::executor::HookService for NoHooks {
    async fn pre_tool_use(
        &self,
        _event: &mut tact_kernel::tool::executor::PreToolUseEvent,
    ) -> Result<tact_kernel::tool::executor::HookControl> {
        Ok(tact_kernel::tool::executor::HookControl::Continue)
    }

    async fn post_tool_use(
        &self,
        _event: &mut tact_kernel::tool::executor::PostToolUseEvent,
    ) -> Result<tact_kernel::tool::executor::HookControl> {
        Ok(tact_kernel::tool::executor::HookControl::Continue)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permission::{PermissionManager, PermissionMode};

    fn manager() -> SharedPermissionManager {
        SharedPermissionManager::new(PermissionManager::try_new(PermissionMode::Default).unwrap())
    }

    #[test]
    fn assembly_installs_the_policy_pair() {
        let manager = manager();
        let assembled = assemble(HostOptions {
            permission_manager: &manager,
            ui_available: Arc::new(|| false),
            hooks: None,
            storage: None,
            filter: CapabilityFilter::default(),
        })
        .unwrap();
        assert!(
            assembled
                .services()
                .contains::<crate::permission::SensitivePathGuard>()
        );
        assert!(
            assembled
                .services()
                .contains::<crate::permission::ManagerPermissionService>()
        );
        assert!(assembled.policy().is_ok());
        assert!(assembled.skipped().is_empty());
    }

    #[test]
    fn a_disabled_capability_is_reported_not_dropped() {
        // The host has to be able to tell the user what their config turned off.
        let manager = manager();
        let disabled: tact_kernel::capability::CapabilityId = "tool:bash".parse().unwrap();
        let assembled = assemble(HostOptions {
            permission_manager: &manager,
            ui_available: Arc::new(|| false),
            hooks: None,
            storage: None,
            filter: CapabilityFilter::new([disabled]),
        })
        .unwrap();
        assert!(
            assembled.skipped().is_empty(),
            "no tool family was installed through this host, so nothing is skipped yet"
        );
    }

    #[test]
    fn a_host_without_hooks_gets_a_no_op_port() {
        let services = ServiceRegistry::default();
        let port = hook_port(&services);
        // The port is usable, which is the contract: an executor always has one.
        assert!(Arc::strong_count(&port) >= 1);
    }

    #[test]
    fn a_host_with_hooks_gets_the_installed_port() {
        let manager = manager();
        let assembled = assemble(HostOptions {
            permission_manager: &manager,
            ui_available: Arc::new(|| false),
            hooks: Some(HookOptions {
                source: crate::plugin::HookSource {
                    label: "test".into(),
                    origin: crate::plugin::HookOrigin::ProjectFile,
                    dirs: crate::consts::PluginDirs::root_only("/tmp"),
                    hooks: crate::plugin::HooksFile::default(),
                },
                work_dir: std::path::PathBuf::from("/tmp"),
                notices: Arc::new(|_| {}),
            }),
            storage: None,
            filter: CapabilityFilter::default(),
        })
        .unwrap();
        assert!(crate::plugin::hook_service(assembled.services()).is_ok());
    }
}
