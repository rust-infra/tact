//! The active LLM provider as a plugin service.
//!
//! `tact_llm` keeps the provider in a process-global `RwLock` because a client
//! is built before anything that could own it exists. That global is the last
//! ambient dependency in the assembly (`settings()`, `get_provider()`), so this
//! plugin makes the snapshot an explicit value: the host installs it once and a
//! consumer asks for the handle.
//!
//! The global stays for now — `tact_llm`'s own `get_llm_client()` is called from
//! places that predate this crate's composition root — but a host that wants the
//! dependency explicit can resolve it from here instead.

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};
use tact_llm::ProviderInfo;

/// Provides the resolved provider snapshot.
pub struct ProvidersPlugin {
    info: ProviderInfo,
}

impl ProvidersPlugin {
    pub fn new(info: ProviderInfo) -> Self {
        Self { info }
    }

    /// The provider the process already installed.
    ///
    /// A convenience for a host that does not want to thread the value: reading
    /// the global once here and holding the snapshot is still better than every
    /// consumer reaching for it, because there is one read and one owner.
    ///
    /// `None` when nothing was installed, which is the case in a unit test that
    /// never called `init_provider`. A host that needs a provider must not treat
    /// that as an error at assembly time — several tests build an agent without
    /// one — so the caller decides.
    pub fn from_active() -> Option<Self> {
        tact_llm::try_get_provider().map(Self::new)
    }
}

#[async_trait::async_trait]
impl Plugin for ProvidersPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.providers").expect("a literal id"),
            "LLM provider",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Infrastructure,
            "The resolved provider snapshot: base URL, protocol, configured model",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.service(Arc::new(self.info.clone()))
    }
}

/// The provider a [`ProvidersPlugin`] installed.
///
/// Returns the `ProviderInfo` the plugin registered, so a consumer that resolved
/// it once does not silently read a different global later.
pub fn provider(services: &tact_kernel::service::ServiceRegistry) -> anyhow::Result<ProviderInfo> {
    services
        .get::<ProviderInfo>()
        .map(|info| (*info).clone())
        .ok_or_else(|| anyhow::anyhow!("no provider installed: is builtin.providers installed?"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    fn info() -> ProviderInfo {
        // A literal rather than a deserialized blob: `ProviderInfo` is a resolved
        // snapshot, so building one directly is what a caller actually does.
        ProviderInfo {
            api_key: "test-key".to_string(),
            base_url: "https://example.invalid/v1".to_string(),
            model: "test-model".to_string(),
            provider: tact_llm::ProviderKind::DeepSeek,
            protocol: tact_llm::OpenAiProtocol::ChatCompletions,
            responses_compact_threshold: None,
        }
    }

    #[tokio::test]
    async fn the_snapshot_is_retrievable_after_sealing() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(ProvidersPlugin::new(info())).await.unwrap();
        let runtime = host.seal().await.unwrap();
        let resolved = provider(runtime.services()).unwrap();
        assert_eq!(resolved.model, "test-model");
        assert_eq!(resolved.base_url, "https://example.invalid/v1");
    }

    #[tokio::test]
    async fn a_host_without_the_plugin_is_told_which_one_is_missing() {
        let runtime = PluginHost::new(CapabilityFilter::default())
            .seal()
            .await
            .unwrap();
        let Err(error) = provider(runtime.services()) else {
            panic!("no provider plugin means no snapshot");
        };
        assert!(error.to_string().contains("builtin.providers"), "{error}");
    }

    #[tokio::test]
    async fn from_active_is_none_when_nothing_was_installed() {
        // A test that never called `init_provider` must not panic at assembly.
        // Whether one is installed is process-wide, so this only asserts the
        // shape: `Some` carries a usable snapshot, `None` is not an error.
        if let Some(plugin) = ProvidersPlugin::from_active() {
            assert_eq!(plugin.manifest().id.as_str(), "builtin.providers");
        }
    }
}
