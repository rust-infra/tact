//! Official extensions registered through the same protocol as external ones.

use crate::{
    Agent,
    plugin::{PluginRegistry, RuntimePluginManifest},
};
use tact_protocol::CapabilityDeclaration;

pub mod agent;
pub mod chat;
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
    for manifest in official_manifests(agent) {
        registry.register(manifest)?;
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
        for manifest in ordered_manifests(Vec::new()) {
            registry
                .register(manifest)
                .expect("the official order satisfies every declared dependency");
        }
        assert_eq!(registry.manifests().len(), 5);
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
