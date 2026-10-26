//! The agent's session settings, as a service a command can reach.
//!
//! The three settings a session can change — model, thinking budget, reasoning
//! effort — are fields on the running agent, and the setters take `&mut self`. A
//! command has no agent, so the service records a *validated change* and the host
//! applies it, the same seam [`crate::goal::service::GoalActivator`] uses for Goal
//! activations.
//!
//! Validation lives here rather than in the command: an unparseable effort must be
//! a usage error the user can see, not a log line. The driver used to print one to
//! stderr and carry on.

use tact_protocol::ReasoningEffort;

/// One session setting to change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsChange {
    /// The model this session calls.
    Model(String),
    /// The thinking budget, in tokens.
    ThinkingBudget(usize),
    /// The reasoning effort, or `None` to clear it.
    ReasoningEffort(Option<ReasoningEffort>),
}

/// What a settings command needs from the runtime.
pub trait SettingsService: Send + Sync {
    /// Record a change for the host to apply.
    ///
    /// # Errors
    ///
    /// Returns an error when the host is gone, which means the session is shutting
    /// down — the change cannot land, and saying so is better than dropping it.
    fn apply(&self, change: SettingsChange) -> anyhow::Result<()>;
}

/// A service that records changes for the host to apply.
///
/// Applying needs `&mut Agent`, and a command runs while its caller holds one: a
/// service that reached for the agent here would deadlock on its own lock. The
/// change is queued instead, and the caller drains the queue once its borrow is
/// free — the same shape as the Goal activation queue.
pub struct QueueingSettings {
    queued: tokio::sync::mpsc::UnboundedSender<SettingsChange>,
}

impl QueueingSettings {
    /// A queue draining into `queued`.
    pub fn new(queued: tokio::sync::mpsc::UnboundedSender<SettingsChange>) -> Self {
        Self { queued }
    }
}

impl SettingsService for QueueingSettings {
    fn apply(&self, change: SettingsChange) -> anyhow::Result<()> {
        self.queued
            .send(change)
            .map_err(|_| anyhow::anyhow!("the settings queue is closed"))
    }
}

/// The settings service a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `SettingsPlugin` was installed, so `/model` reports
/// that it cannot apply rather than accepting a value nothing reads.
pub fn service(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<std::sync::Arc<dyn SettingsService>> {
    services
        .get::<SettingsHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no settings service installed; install SettingsPlugin"))
}

/// The service a plugin provided.
#[derive(Clone)]
pub struct SettingsHandle(pub std::sync::Arc<dyn SettingsService>);

/// Session settings, contributed as a plugin.
pub struct SettingsPlugin {
    service: std::sync::Arc<dyn SettingsService>,
}

impl SettingsPlugin {
    /// A plugin over the given service.
    pub fn new(service: std::sync::Arc<dyn SettingsService>) -> Self {
        Self { service }
    }
}

#[async_trait::async_trait]
impl tact_kernel::plugin::Plugin for SettingsPlugin {
    fn manifest(&self) -> tact_kernel::plugin::PluginManifest {
        tact_kernel::plugin::PluginManifest::new(
            tact_kernel::plugin::PluginId::new("builtin.settings").expect("a literal id"),
            "Session settings",
            env!("CARGO_PKG_VERSION"),
            tact_kernel::plugin::PluginKind::Capability,
            "Model, thinking budget and reasoning effort for this session",
        )
    }

    fn register(&self, registrar: &mut tact_kernel::plugin::Registrar<'_>) -> anyhow::Result<()> {
        registrar.service(std::sync::Arc::new(SettingsHandle(self.service.clone())))
    }
}

#[cfg(test)]
mod plugin_tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    /// The plugin registers a service the host can resolve.
    #[tokio::test]
    async fn the_plugin_provides_a_resolvable_service() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<SettingsChange>();
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(SettingsPlugin::new(std::sync::Arc::new(
            QueueingSettings::new(tx),
        )))
        .await
        .unwrap();
        let runtime = host.seal().await.unwrap();

        service(runtime.services())
            .unwrap()
            .apply(SettingsChange::Model("m".into()))
            .unwrap();
        assert_eq!(rx.try_recv().unwrap(), SettingsChange::Model("m".into()));
    }

    /// A host with no plugin is told so by name.
    #[test]
    fn a_missing_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match service(&registry) {
            Ok(_) => panic!("no service was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("SettingsPlugin"), "{error}");
    }

    /// A closed queue is an error, not a silent drop.
    ///
    /// The session is shutting down when this happens, and a change that cannot land
    /// must say so rather than leaving a command reporting success.
    #[test]
    fn a_closed_queue_is_an_error() {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SettingsChange>();
        drop(rx);
        let service = QueueingSettings::new(tx);
        assert!(service.apply(SettingsChange::ThinkingBudget(1)).is_err());
    }

    /// The manifest says what it contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        use tact_kernel::plugin::Plugin;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel::<SettingsChange>();
        let manifest =
            SettingsPlugin::new(std::sync::Arc::new(QueueingSettings::new(tx))).manifest();
        assert_eq!(manifest.id.as_str(), "builtin.settings");
        assert_eq!(manifest.kind, tact_kernel::plugin::PluginKind::Capability);
    }
}
