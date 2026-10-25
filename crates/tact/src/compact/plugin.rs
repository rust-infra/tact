//! Compaction, contributed as a plugin.
//!
//! The service is the replaceable half (micro-compaction, estimates, history
//! shaping, transcript). The trigger policy stays on the agent, because deciding
//! *when* to compact means rewriting the agent's own context mid-loop.
//!
//! Registered as a kernel service slot even though the trait is extension-level:
//! the registry is how a host reaches it, and the trait's location is what keeps
//! the kernel from depending on `tact_llm`.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

use crate::compact::service::{BuiltinCompaction, CompactionService};

/// Contributes the compaction policy.
pub struct CompactionPlugin {
    service: Arc<dyn CompactionService>,
}

impl CompactionPlugin {
    /// A plugin over an explicit policy.
    pub fn new(service: Arc<dyn CompactionService>) -> Self {
        Self { service }
    }

    /// A plugin over the crate's built-in policy.
    pub fn builtin() -> Self {
        Self::new(Arc::new(BuiltinCompaction))
    }
}

#[async_trait]
impl Plugin for CompactionPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.compaction_service").expect("a literal id"),
            "Compaction service",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Runtime,
            "Micro-compaction, token estimates, compacted-history shaping and transcripts",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        // A concrete newtype, not `Arc<dyn CompactionService>`: a `TypeId` keys
        // the registry, and a trait object's type is not the handle a caller can
        // name.
        registrar.service(Arc::new(CompactionServiceHandle(self.service.clone())))
    }
}

/// The compaction service a [`CompactionPlugin`] provided.
#[derive(Clone)]
pub struct CompactionServiceHandle(pub Arc<dyn CompactionService>);

impl std::ops::Deref for CompactionServiceHandle {
    type Target = dyn CompactionService;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref()
    }
}

/// The compaction policy a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `CompactionPlugin` was installed. A host that runs
/// turns must install one: compaction is not optional, because an uncompacted
/// context eventually exceeds the model's window.
pub fn compaction_service(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<dyn CompactionService>> {
    services
        .get::<CompactionServiceHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no compaction service installed; install CompactionPlugin"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    /// The plugin registers a policy the host can resolve and use.
    #[tokio::test]
    async fn the_plugin_provides_a_usable_service() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(CompactionPlugin::builtin()).await.unwrap();
        let runtime = host.seal().await.unwrap();

        let resolved = compaction_service(runtime.services()).expect("registered");
        // An empty context is cheap but not necessarily zero: the estimate
        // includes a serialization envelope. What matters is that it is small.
        assert!(
            resolved.estimate_tokens(&[]) < 100,
            "an empty context costs almost nothing: {}",
            resolved.estimate_tokens(&[])
        );
    }

    /// A host that replaces the policy gets its own behaviour.
    ///
    /// This is the whole point of the service being a trait: a host with a
    /// different idea of what to keep does not have to fork the agent.
    #[tokio::test]
    async fn a_replacement_policy_is_what_the_host_gets() {
        struct AlwaysZero;

        impl CompactionService for AlwaysZero {
            fn micro_compact(&self, _messages: &mut [tact_llm::Message], _enabled: bool) {}
            fn estimate_tokens(&self, _messages: &[tact_llm::Message]) -> usize {
                0
            }
            fn user_messages(&self, _messages: &[tact_llm::Message]) -> Vec<tact_llm::Message> {
                Vec::new()
            }
            fn build_history(
                &self,
                _user_messages: &[tact_llm::Message],
                _summary: String,
                _max_tokens: usize,
                _transcript: Option<&std::path::Path>,
            ) -> Vec<tact_llm::Message> {
                Vec::new()
            }
            fn compacted_context(&self, _summary: String) -> Vec<tact_llm::Message> {
                Vec::new()
            }
            fn write_transcript<'a>(
                &'a self,
                _tact_path: &'a crate::consts::TactPath,
                _messages: &'a [tact_llm::Message],
            ) -> std::pin::Pin<
                Box<
                    dyn std::future::Future<Output = anyhow::Result<std::path::PathBuf>>
                        + Send
                        + 'a,
                >,
            > {
                Box::pin(async { Ok(std::path::PathBuf::from("/nowhere")) })
            }
        }

        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(CompactionPlugin::new(Arc::new(AlwaysZero)))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();

        let resolved = compaction_service(runtime.services()).expect("registered");
        let user = tact_llm::Message::new_text(tact_llm::Role::User, "text");
        assert_eq!(
            resolved.estimate_tokens(&[user]),
            0,
            "the replacement policy answered, not the built-in"
        );
    }

    /// A host with no compaction plugin is told so by name.
    #[test]
    fn a_missing_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match compaction_service(&registry) {
            Ok(_) => panic!("no service was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("CompactionPlugin"), "{error}");
    }

    /// The manifest names what it contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        let manifest = CompactionPlugin::builtin().manifest();
        assert_eq!(manifest.id.as_str(), "builtin.compaction_service");
        assert_eq!(manifest.kind, PluginKind::Runtime);
    }
}
