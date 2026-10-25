//! The agent loop, contributed as a plugin.
//!
//! The loop itself lives in [`crate::agent`]; this module is the composition
//! seam that makes it replaceable. A host that wants a different loop registers a
//! different `AgentService` and the view never learns the difference.
//!
//! The plugin registers the service during assembly, before the agent exists, so
//! the service holds an empty slot that the agent fills at the end of its own
//! construction. See [`RuntimeAgentService::pending`] for why that is honest
//! rather than a workaround.

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};
use tact_kernel::runtime::AgentService;

use crate::agent::service::RuntimeAgentService;

/// Contributes the model loop as a host-facing service.
pub struct AgentPlugin {
    service: Arc<RuntimeAgentService>,
}

impl AgentPlugin {
    /// A plugin over a service whose slot is not filled yet.
    pub fn new(service: Arc<RuntimeAgentService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Plugin for AgentPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.agent_service").expect("a literal id"),
            "Agent service",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Runtime,
            "The model loop: streaming, tool scheduling, prompt assembly and recovery",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        // A concrete newtype, not `Arc<RuntimeAgentService>`: a `TypeId` keys the
        // registry, and `Arc<T>` is a different type from `T`, so providing the
        // `Arc` would make the handle unretrievable as the service it implements.
        registrar.service(Arc::new(AgentServiceHandle(self.service.clone())))
    }
}

/// The agent service an [`AgentPlugin`] provided.
#[derive(Clone)]
pub struct AgentServiceHandle(pub Arc<RuntimeAgentService>);

impl std::ops::Deref for AgentServiceHandle {
    type Target = RuntimeAgentService;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The agent service a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `AgentPlugin` was installed, which is a composition
/// bug: a host that wants to run turns must install one.
pub fn agent_service(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<RuntimeAgentService>> {
    services
        .get::<AgentServiceHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no agent service installed; install AgentPlugin"))
}

/// The service as the kernel's trait object, for a host that stores it that way.
pub fn as_service(service: &Arc<RuntimeAgentService>) -> Arc<dyn AgentService> {
    service.clone()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    fn pending() -> Arc<RuntimeAgentService> {
        Arc::new(RuntimeAgentService::pending(Arc::new(
            std::sync::atomic::AtomicBool::new(false),
        )))
    }

    /// The plugin registers a service the host can resolve.
    #[tokio::test]
    async fn the_plugin_provides_a_resolvable_service() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(AgentPlugin::new(pending())).await.unwrap();
        let runtime = host.seal().await.unwrap();

        let resolved = agent_service(runtime.services()).expect("the service is registered");
        assert!(!resolved.is_installed(), "the slot starts empty");
    }

    /// The chat plugin registers a service the host can resolve.
    #[tokio::test]
    async fn the_chat_plugin_provides_a_resolvable_service() {
        let chat = Arc::new(crate::agent::chat::RuntimeChatService::new(
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(ChatPlugin::new(chat)).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(chat_service(runtime.services()).is_ok());
    }

    /// A host with no chat plugin is told so by name.
    #[test]
    fn a_missing_chat_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match chat_service(&registry) {
            Ok(_) => panic!("no service was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("ChatPlugin"), "{error}");
    }

    /// The chat manifest says what it contributes.
    #[test]
    fn the_chat_manifest_says_what_it_is() {
        let chat = Arc::new(crate::agent::chat::RuntimeChatService::new(
            Arc::new(tokio::sync::Mutex::new(None)),
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
        ));
        let manifest = ChatPlugin::new(chat).manifest();
        assert_eq!(manifest.id.as_str(), "builtin.chat_service");
        assert_eq!(manifest.kind, PluginKind::Runtime);
    }

    /// A host with no agent plugin gets a named error, not a silent `None`.
    ///
    /// A caller that runs turns must be told the composition is wrong; returning
    /// an empty service would turn that into a turn that fails for no visible
    /// reason.
    #[test]
    fn a_missing_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match agent_service(&registry) {
            Ok(_) => panic!("no service was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("AgentPlugin"), "{error}");
    }

    /// The manifest names what the plugin contributes.
    #[test]
    fn the_manifest_says_what_it_is() {
        let manifest = AgentPlugin::new(pending()).manifest();
        assert_eq!(manifest.id.as_str(), "builtin.agent_service");
        assert_eq!(manifest.kind, PluginKind::Runtime);
    }

    /// A turn before the agent is installed fails by name, not by hanging.
    ///
    /// The slot is empty between assembly and the agent's own construction. A
    /// caller that reaches the service in that window is a composition bug, and
    /// the error says which one.
    #[tokio::test]
    async fn a_turn_before_installation_names_the_problem() {
        use tact_kernel::runtime::{AgentService as _, TurnOptions, TurnRequest};
        let service = pending();
        let error = match service
            .run(TurnRequest::text("hi"), TurnOptions::default())
            .await
        {
            Ok(_) => panic!("no agent is installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("not installed"), "{error}");
    }
}

/// Contributes the turn lifecycle.
pub struct ChatPlugin {
    service: Arc<crate::agent::chat::RuntimeChatService>,
}

impl ChatPlugin {
    /// A plugin over a service sharing the agent service's slot.
    pub fn new(service: Arc<crate::agent::chat::RuntimeChatService>) -> Self {
        Self { service }
    }
}

#[async_trait]
impl Plugin for ChatPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.chat_service").expect("a literal id"),
            "Chat service",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Runtime,
            "The user turn lifecycle: message assembly, Stop-hook continuation and completion",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.service(Arc::new(ChatServiceHandle(self.service.clone())))
    }
}

/// The chat service a [`ChatPlugin`] provided.
#[derive(Clone)]
pub struct ChatServiceHandle(pub Arc<crate::agent::chat::RuntimeChatService>);

impl std::ops::Deref for ChatServiceHandle {
    type Target = crate::agent::chat::RuntimeChatService;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The chat service a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `ChatPlugin` was installed: a view that submits turns
/// needs one.
pub fn chat_service(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<crate::agent::chat::RuntimeChatService>> {
    services
        .get::<ChatServiceHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no chat service installed; install ChatPlugin"))
}
