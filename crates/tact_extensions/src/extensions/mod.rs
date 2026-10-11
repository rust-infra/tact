//! Official extensions registered through the same protocol as external ones.

use crate::{
    Agent,
    plugin::{PluginRegistry, RuntimePluginManifest},
};
use tact_protocol::CapabilityDeclaration;

pub mod agent;
pub mod chat;
pub mod chat_input;
pub mod session;
pub mod tools;
pub mod workflow;

pub fn official_manifests(agent: &Agent) -> Vec<RuntimePluginManifest> {
    ordered_manifests(agent.capability_declarations())
}

/// The official extension set in **dependency order**.
///
/// `PluginRegistry::register` refuses a manifest whose dependencies are not
/// registered yet, so this order is a protocol requirement, not cosmetics:
/// `tact.agent` dispatches tool capabilities through the router during a run,
/// which means `tact.tools` has to exist first.
fn ordered_manifests(tool_capabilities: Vec<CapabilityDeclaration>) -> Vec<RuntimePluginManifest> {
    vec![
        session::manifest(),
        tools::manifest(tool_capabilities),
        agent::manifest(),
        chat::manifest(),
        workflow::manifest(),
    ]
}

pub fn register_official_manifests(
    registry: &PluginRegistry,
    agent: &Agent,
) -> Result<(), tact::KernelError> {
    register_in_order(registry, official_manifests(agent))
}

/// Registers each manifest in order and then marks it serving.
///
/// The in-process Rust host serves an extension the moment it is registered, so
/// the registry must not report it as merely declared: `state` and `health`
/// would otherwise understate what is actually live.
fn register_in_order(
    registry: &PluginRegistry,
    manifests: Vec<RuntimePluginManifest>,
) -> Result<(), tact::KernelError> {
    for manifest in manifests {
        let id = manifest.id.clone();
        registry.register(manifest)?;
        registry.start(&id)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_manifests_use_unique_ids() {
        let manifests = [session::manifest(), chat::manifest(), workflow::manifest()];
        let ids = manifests
            .iter()
            .map(|manifest| manifest.id.as_str())
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(ids.len(), manifests.len());
        assert!(ids.contains("tact.chat"));
    }

    #[test]
    fn official_manifests_register_in_dependency_order() {
        let registry = PluginRegistry::new(tact_protocol::ProtocolVersion::CURRENT);
        register_in_order(&registry, ordered_manifests(Vec::new()))
            .expect("the official order satisfies every declared dependency");
        assert_eq!(registry.manifests().len(), 5);

        // Registered means serving for the in-process host, and every official
        // extension must report that.
        let health = registry.health_all();
        assert_eq!(health.len(), 5);
        assert!(
            health.iter().all(|plugin| plugin.is_healthy()),
            "every official extension is serving: {health:?}"
        );
        assert!(
            health
                .iter()
                .all(|plugin| plugin.state == tact::PluginState::Running),
            "registration starts the in-process extension: {health:?}"
        );
    }

    #[test]
    fn official_manifests_register_on_the_session_runtime_registry() {
        // The bootstrap registers the official manifests through the *serving
        // context's* registry (`RuntimeContext::plugins`), not through an Agent
        // field: extension lifecycle is session state. A host reads the state
        // back through a clone of that context — the same handle the frontends
        // hold — so the clone must share the registry, not copy it.
        let context = tact::RuntimeContext::new(tact::CapabilityRouter::new());
        register_in_order(context.plugins(), ordered_manifests(Vec::new()))
            .expect("the official order satisfies every declared dependency");

        let host_view = context.clone();
        let health = host_view.plugins().health_all();
        assert_eq!(health.len(), 5);
        assert!(
            health.iter().all(|plugin| plugin.is_healthy()),
            "every official extension is serving: {health:?}"
        );
    }

    #[test]
    fn the_agent_extension_requires_the_tools_extension_first() {
        let registry = PluginRegistry::new(tact_protocol::ProtocolVersion::CURRENT);
        let error = registry
            .register(agent::manifest())
            .expect_err("agent declares a dependency on tools");
        assert_eq!(
            error.category(),
            tact_protocol::ErrorCategory::PluginUnavailable
        );

        registry
            .register(tools::manifest(Vec::new()))
            .expect("tools registers first");
        registry.register(agent::manifest()).expect("agent follows");
    }
}

#[cfg(test)]
mod agent_tests;

#[cfg(test)]
mod official_extensions_tests;

#[cfg(test)]
mod session_tests;
