//! The resolved configuration as a plugin service.
//!
//! `config::settings()` is the last ambient dependency in the assembly: a
//! process-global `RwLock` that any module can reach without being handed
//! anything. That is why `tui` could not be decoupled from `tact` — 45 call sites
//! read it directly, and none of them is a dependency the type system can see.
//!
//! This plugin makes the snapshot an explicit value. It does not remove the
//! global (too many call sites depend on it today); it gives every *new* code a
//! way to receive the configuration instead of reaching for it, and it gives the
//! host one place to record which snapshot it assembled with.

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

use crate::config::ResolvedConfig;

/// Provides the resolved configuration snapshot.
pub struct ConfigPlugin {
    config: ResolvedConfig,
}

impl ConfigPlugin {
    /// A plugin over a caller-supplied snapshot.
    pub fn new(config: ResolvedConfig) -> Self {
        Self { config }
    }

    /// A plugin over the process-global configuration.
    ///
    /// `None` when nothing was installed, which is the case in a unit test that
    /// never called `config::init`. A host that needs configuration decides what
    /// a missing snapshot means; this does not.
    pub fn from_active() -> Option<Self> {
        crate::config::try_settings().map(Self::new)
    }
}

#[async_trait::async_trait]
impl Plugin for ConfigPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.config").expect("a literal id"),
            "Resolved configuration",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Infrastructure,
            "The resolved [llm]/[agent]/[tools]/[ui]/[voice]/[mcp] snapshot",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.service(Arc::new(self.config.clone()))
    }
}

/// The configuration a [`ConfigPlugin`] installed.
///
/// A clone, not a reference: a consumer that resolved it once must not observe a
/// later `install_or_override` that the host did not intend it to see.
pub fn config(services: &tact_kernel::service::ServiceRegistry) -> anyhow::Result<ResolvedConfig> {
    services
        .get::<ResolvedConfig>()
        .map(|config| (*config).clone())
        .ok_or_else(|| anyhow::anyhow!("no configuration installed: is builtin.config installed?"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    fn snapshot() -> ResolvedConfig {
        crate::config::test_support::install_default();
        crate::config::settings()
    }

    #[tokio::test]
    async fn the_snapshot_is_retrievable_after_sealing() {
        let expected = snapshot();
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(ConfigPlugin::new(expected.clone()))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();
        let resolved = super::config(runtime.services()).unwrap();
        assert_eq!(resolved.llm.model, expected.llm.model);
        assert_eq!(
            resolved.tools.bash_timeout_secs,
            expected.tools.bash_timeout_secs
        );
    }

    #[tokio::test]
    async fn a_host_without_the_plugin_is_told_which_one_is_missing() {
        let runtime = PluginHost::new(CapabilityFilter::default())
            .seal()
            .await
            .unwrap();
        let Err(error) = super::config(runtime.services()) else {
            panic!("no config plugin means no snapshot");
        };
        assert!(error.to_string().contains("builtin.config"), "{error}");
    }

    #[tokio::test]
    async fn the_snapshot_is_a_value_not_a_view_of_the_global() {
        // The point of handing over a value: a plugin installed with one
        // configuration keeps it, even when the global says something else. This
        // is what a consumer can rely on; the global can be replaced only under
        // `test-support`, so the assertion is made through a *different* snapshot
        // rather than by mutating the process state.
        let mut installed = snapshot();
        installed.llm.model = "installed-model".to_string();
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(ConfigPlugin::new(installed)).await.unwrap();
        let runtime = host.seal().await.unwrap();
        let resolved = super::config(runtime.services()).unwrap();
        assert_eq!(
            resolved.llm.model, "installed-model",
            "the registry keeps the snapshot it was given"
        );
        assert_ne!(
            resolved.llm.model,
            snapshot().llm.model,
            "and it is not the process global's value"
        );
    }
}
