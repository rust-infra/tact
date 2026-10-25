//! The plugin marketplace as a plugin service.
//!
//! `MarketplaceService` and `PluginInstaller` were always constructed ad hoc from
//! a `PluginHome`. Providing them through the registry is what lets a host ask for
//! "the installer" without knowing how one is built — and, more importantly, makes
//! the plugin home an explicit dependency instead of something a caller derives
//! from the environment at the point of use.
//!
//! Both are provided by one plugin because they share one home: two plugins each
//! deriving their own would be two answers to "where do plugins live".

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

use crate::consts::PluginHome;
use crate::plugin::install::PluginInstaller;
use crate::plugin::marketplace::MarketplaceService;

/// Provides the marketplace pair over one plugin home.
pub struct MarketplacePlugin {
    home: PluginHome,
}

impl MarketplacePlugin {
    /// A plugin rooted at `home`.
    pub fn new(home: PluginHome) -> Self {
        Self { home }
    }

    /// A plugin at the current user's plugin home.
    ///
    /// `None` when `$HOME` is unset, which is the caller's decision to handle:
    /// a host that has no home cannot install plugins, but it can still run.
    pub fn from_env() -> Option<Self> {
        crate::consts::PluginHome::from_environment().map(Self::new)
    }
}

#[async_trait::async_trait]
impl Plugin for MarketplacePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.marketplace").expect("a literal id"),
            "Plugin marketplace",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Infrastructure,
            "Marketplace sources and the installer, over one plugin home",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.service(Arc::new(MarketplaceService::new(self.home.clone())))?;
        // The installer builds its own service internally; providing it as a
        // separate handle is what lets a command install without also being given
        // the marketplace.
        registrar.service(Arc::new(PluginInstaller::new(self.home.clone())))
    }
}

/// The marketplace service a [`MarketplacePlugin`] provided.
pub fn marketplace(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<MarketplaceService> {
    services
        .get::<MarketplaceService>()
        .map(|service| (*service).clone())
        .ok_or_else(|| {
            anyhow::anyhow!("no marketplace installed: is builtin.marketplace installed?")
        })
}

/// The installer a [`MarketplacePlugin`] provided.
///
/// Handed out by reference, not clone: an installer owns no shared state, but the
/// registry's handle is what a caller should use rather than building a second
/// one over the same home.
pub fn installer(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<Arc<PluginInstaller>> {
    services
        .get::<PluginInstaller>()
        .ok_or_else(|| anyhow::anyhow!("no installer installed: is builtin.marketplace installed?"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    fn home(name: &str) -> PluginHome {
        let dir = std::env::temp_dir().join(format!("tact-marketplace-plugin-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        PluginHome::from_home(&dir)
    }

    #[tokio::test]
    async fn both_handles_are_retrievable_after_sealing() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(MarketplacePlugin::new(home("both")))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(marketplace(runtime.services()).is_ok());
        assert!(installer(runtime.services()).is_ok());
    }

    #[tokio::test]
    async fn a_host_without_the_plugin_names_it() {
        let runtime = PluginHost::new(CapabilityFilter::default())
            .seal()
            .await
            .unwrap();
        let Err(error) = marketplace(runtime.services()) else {
            panic!("no marketplace plugin means no service");
        };
        assert!(error.to_string().contains("builtin.marketplace"), "{error}");
        let Err(error) = installer(runtime.services()) else {
            panic!("no marketplace plugin means no installer");
        };
        assert!(error.to_string().contains("builtin.marketplace"), "{error}");
    }

    #[tokio::test]
    async fn one_plugin_provides_both_so_the_home_has_one_answer() {
        // Two plugins each deriving a home would be two answers to "where do
        // plugins live"; this pins that both handles come from one install.
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(MarketplacePlugin::new(home("single")))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();
        assert_eq!(
            runtime.services().len(),
            2,
            "exactly the marketplace and the installer"
        );
        assert_eq!(
            runtime
                .services()
                .provider::<PluginInstaller>()
                .map(|id| id.as_str()),
            Some("builtin.marketplace")
        );
    }
}
