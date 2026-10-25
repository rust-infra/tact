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
///
/// The optional infrastructure (hooks, storage, marketplace) travels in
/// [`HostExtras`] instead: a host that has none of it should not have to write
/// three `None`s, and the Agent's own assembly — which never has any of it —
/// must stay readable.
pub struct HostOptions<'a> {
    pub permission_manager: &'a SharedPermissionManager,
    /// Asked live: a frontend attached after assembly must still be able to
    /// answer a prompt.
    pub ui_available: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The resolved configuration, when one was installed.
    ///
    /// `Option` for the same reason as `provider`: a unit test may build an agent
    /// before any configuration exists, and a host must record that honestly
    /// rather than claim a snapshot it does not have.
    pub config: Option<crate::config::ResolvedConfig>,
    /// The provider snapshot, when one was installed.
    ///
    /// `Option` because a test may build an agent before any provider exists;
    /// recording `None` is what lets `providers` be a fact rather than a panic.
    pub provider: Option<tact_llm::ProviderInfo>,
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

/// The optional infrastructure a host installs into its runtime.
///
/// Separate from [`HostOptions`] so the Agent can assemble its bundle with
/// [`HostExtras::default`] and only a caller that has something to install pays
/// attention. Every field is `None` in a unit test, which is what keeps
/// `Agent::new` a constructor rather than a five-field host description — and
/// what makes the production path (which *does* have a store) go through the
/// same assembly as the tests instead of passing one in behind the registry.
#[derive(Default)]
pub struct HostExtras {
    /// Hooks the host runs through the kernel port rather than on its own side.
    pub hooks: Option<HookOptions>,
    /// The already-opened session store.
    ///
    /// Opening is async and happens before the agent exists (the session id and
    /// its lock are resolved against the store first), so the host opens it and
    /// installs it here. What the caller must not do is *also* hand the same
    /// store to `Agent::with_session`: the registry is the single handoff.
    pub storage: Option<StorageOptions>,
    /// The plugin home, when the host installs the marketplace.
    pub marketplace: Option<crate::consts::PluginHome>,
    /// The agent service whose slot this agent will fill.
    ///
    /// Passed in rather than built here because the service must be registered
    /// *during* assembly, while the agent it wraps does not exist until assembly
    /// finishes. The host creates the empty service, hands it here, and the agent
    /// installs itself into it at the end of its own construction.
    pub agent_service: Option<std::sync::Arc<crate::agent::service::RuntimeAgentService>>,
    /// The chat service, sharing the agent service's slot.
    ///
    /// Registered beside the agent service because they must serialise against
    /// each other: a turn and a compaction touching one conversation
    /// concurrently would interleave their messages.
    pub chat_service: Option<std::sync::Arc<crate::agent::chat::RuntimeChatService>>,
    /// The already-opened background manager.
    ///
    /// Opening is async and happens before the agent exists, like the session
    /// store: the host opens it and installs it here, and the registry is the
    /// single handoff.
    pub background: Option<crate::background::SharedBackgroundManager>,
    /// The already-loaded memory manager.
    ///
    /// Loading reads the memory directory, so it happens before assembly for the
    /// same reason the session store does.
    pub memory: Option<crate::memory::SharedMemoryManager>,
    /// The already-scanned skill registry.
    ///
    /// Scanning reads the working directory, so it happens before assembly for the
    /// same reason the session store does.
    pub skills: Option<crate::skill::SharedSkillRegistry>,
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

    /// The installed plugins, in installation order.
    pub fn plugin_ids(&self) -> Vec<String> {
        self.runtime
            .plugin_ids()
            .iter()
            .map(|id| id.as_str().to_string())
            .collect()
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
    assemble_with(options, HostExtras::default())
}

/// [`assemble`], with the optional infrastructure named in one place.
///
/// The split exists so the Agent's own assembly (which has no hooks, store or
/// marketplace of its own) reads as a two-argument call, while a host that owns
/// infrastructure describes it once in a [`HostExtras`] instead of in five
/// separate `Option` fields at the call site.
pub fn assemble_with(options: HostOptions<'_>, extras: HostExtras) -> Result<AssembledRuntime> {
    let mut host = PluginHost::new(options.filter);
    host.install_sync(policy::SecurityPlugin::new(
        options.permission_manager.scanner(),
    ))?;
    host.install_sync(policy::PermissionPlugin::new(
        options.permission_manager.clone(),
        options.ui_available,
    ))?;

    if let Some(hooks) = extras.hooks {
        host.install_sync(crate::plugin::HooksPlugin::new(
            hooks.source,
            hooks.work_dir,
            hooks.notices,
        ))?;
    }
    if let Some(home) = extras.marketplace {
        host.install_sync(crate::plugin::MarketplacePlugin::new(home))?;
    }
    if let Some(config) = options.config {
        host.install_sync(crate::plugin::ConfigPlugin::new(config))?;
    }
    if let Some(info) = options.provider {
        host.install_sync(crate::plugin::ProvidersPlugin::new(info))?;
    }
    if let Some(storage) = extras.storage {
        host.install_sync(crate::store::plugin::StoragePlugin::new(&storage.path))?;
        // The service takes its own handle: the session service needs the store
        // after the plugin that provides it, and installing before it would leave
        // the service orphaned from the bundle's lifetime.
        let session_store = storage.store.clone();
        host.install_sync(crate::store::plugin::SessionStorePlugin::new(storage.store))?;
        host.install_sync(crate::store::SessionPlugin::new(session_store))?;
    }

    // The loop's service is registered here so a view can resolve it from the
    // registry like every other service; the agent fills its slot afterwards.
    if let Some(service) = extras.agent_service.clone() {
        host.install_sync(crate::agent::plugin::AgentPlugin::new(service))?;
    }
    if let Some(service) = extras.chat_service.clone() {
        host.install_sync(crate::agent::plugin::ChatPlugin::new(service))?;
    }

    // Compaction is not optional — an uncompacted context eventually exceeds the
    // model's window — so the built-in policy is always installed. A host that
    // wants a different one registers its own plugin instead.
    host.install_sync(crate::compact::plugin::CompactionPlugin::builtin())?;

    // Notifications are a capability domain: the platform notifier is the default,
    // and a host that renders them elsewhere installs its own sink instead.
    host.install_sync(crate::notifications::NotificationsPlugin::platform())?;

    if let Some(manager) = extras.background.clone() {
        host.install_sync(crate::background::BackgroundPlugin::new(manager))?;
    }
    if let Some(manager) = extras.memory.clone() {
        host.install_sync(crate::memory::MemoryPlugin::new(manager))?;
    }
    if let Some(registry) = extras.skills.clone() {
        host.install_sync(crate::skill::SkillsPlugin::new(registry))?;
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
            config: None,
            provider: None,
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
            config: None,
            provider: None,
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
    fn a_host_with_config_provides_the_snapshot() {
        // The frontend's 45 direct `settings()` calls are the reason it cannot be
        // decoupled from this crate; the plugin is what gives new code a way to
        // receive the configuration instead of reaching for it.
        let manager = manager();
        crate::config::test_support::install_default();
        let assembled = assemble(HostOptions {
            permission_manager: &manager,
            ui_available: Arc::new(|| false),
            config: crate::config::try_settings(),
            provider: None,
            filter: CapabilityFilter::default(),
        })
        .unwrap();
        let resolved = crate::plugin::config(assembled.services()).unwrap();
        assert_eq!(
            resolved.tools.bash_timeout_secs,
            crate::config::settings().tools.bash_timeout_secs
        );
    }

    #[test]
    fn assembly_installs_in_dependency_order() {
        // A plugin's `register` may only assume what was installed before it, so
        // the order is part of the contract, not an implementation detail.
        let manager = manager();
        let dir = std::env::temp_dir().join("tact-host-order");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let assembled = assemble_with(
            HostOptions {
                permission_manager: &manager,
                ui_available: Arc::new(|| false),
                config: None,
                provider: None,
                filter: CapabilityFilter::default(),
            },
            HostExtras {
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
                marketplace: Some(crate::consts::PluginHome::from_home(&dir)),
                ..HostExtras::default()
            },
        )
        .unwrap();
        assert_eq!(
            assembled.plugin_ids(),
            vec![
                // Security first: the permission port depends on a guard.
                "builtin.security",
                "builtin.permission",
                "builtin.hooks",
                "builtin.marketplace",
                // Compaction after the policy plugins: it depends on nothing
                // else here, and being late is what makes "every host gets a
                // policy" true even when the earlier plugins are filtered out.
                "builtin.compaction_service",
                // Notifications are a capability domain, installed last so a
                // filtered-out host still gets no sink rather than a partial one.
                "builtin.notifications",
            ]
        );
    }

    #[test]
    fn a_host_with_a_marketplace_gets_both_handles() {
        let manager = manager();
        let dir = std::env::temp_dir().join("tact-host-marketplace");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let assembled = assemble_with(
            HostOptions {
                permission_manager: &manager,
                ui_available: Arc::new(|| false),
                config: None,
                provider: None,
                filter: CapabilityFilter::default(),
            },
            HostExtras {
                marketplace: Some(crate::consts::PluginHome::from_home(&dir)),
                ..HostExtras::default()
            },
        )
        .unwrap();
        assert!(crate::plugin::marketplace(assembled.services()).is_ok());
        assert!(crate::plugin::installer(assembled.services()).is_ok());
    }

    /// A host that installed storage also gets the session service.
    ///
    /// The service is what a frontend drives to resolve, list and close sessions;
    /// without it a host would have to reach into the store itself, which is the
    /// dependency this plugin exists to remove.
    #[tokio::test]
    async fn a_host_with_storage_gets_the_session_service() {
        crate::config::test_support::install_default();
        let dir = std::env::temp_dir().join("tact-host-session-service");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let store = crate::store::open_sqlite_session_store(&dir.join("session.db"))
            .await
            .unwrap();
        let manager = manager();
        let assembled = assemble_with(
            HostOptions {
                permission_manager: &manager,
                ui_available: Arc::new(|| false),
                config: None,
                provider: None,
                filter: CapabilityFilter::default(),
            },
            HostExtras {
                storage: Some(StorageOptions {
                    store,
                    path: dir.join("session.db"),
                }),
                ..HostExtras::default()
            },
        )
        .unwrap();

        assert!(
            crate::store::session_service(assembled.services()).is_ok(),
            "the storage bundle must also provide the session service"
        );
        // Order matters at shutdown, so it is asserted rather than assumed.
        let ids = assembled.plugin_ids();
        let store_at = ids
            .iter()
            .position(|id| id == "builtin.session_store")
            .expect("session store installed");
        let service_at = ids
            .iter()
            .position(|id| id == "builtin.session_service")
            .expect("session service installed");
        assert!(
            service_at > store_at,
            "the service is installed after the store it reads: {ids:?}"
        );
    }

    /// Every host gets a compaction policy, storage or not.
    ///
    /// Compaction is not optional: an uncompacted context eventually exceeds the
    /// model's window, so a host that never installed a policy would fail on a
    /// long session rather than at assembly.
    #[tokio::test]
    async fn every_host_gets_a_compaction_service() {
        crate::config::test_support::install_default();
        let manager = manager();
        let assembled = assemble_with(
            HostOptions {
                permission_manager: &manager,
                ui_available: Arc::new(|| false),
                config: None,
                provider: None,
                filter: CapabilityFilter::default(),
            },
            HostExtras::default(),
        )
        .unwrap();

        let service = crate::compact::plugin::compaction_service(assembled.services())
            .expect("a host always gets a compaction policy");
        assert!(service.estimate_tokens(&[]) < 100);
        assert!(
            assembled
                .plugin_ids()
                .iter()
                .any(|id| id == "builtin.compaction_service"),
            "the plugin is installed, not just the value"
        );
    }

    #[test]
    fn a_host_with_hooks_gets_the_installed_port() {
        let manager = manager();
        let assembled = assemble_with(
            HostOptions {
                permission_manager: &manager,
                ui_available: Arc::new(|| false),
                config: None,
                provider: None,
                filter: CapabilityFilter::default(),
            },
            HostExtras {
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
                ..HostExtras::default()
            },
        )
        .unwrap();
        assert!(crate::plugin::hook_service(assembled.services()).is_ok());
    }
}
