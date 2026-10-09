//! Official extensions registered through the same protocol as external ones.

use crate::{
    Agent,
    plugin::{PluginRegistry, RuntimePluginManifest},
};

pub mod agent;
pub mod chat;
pub mod session;
pub mod tools;
pub mod workflow;

pub fn official_manifests(agent: &Agent) -> Vec<RuntimePluginManifest> {
    vec![
        agent::manifest(),
        session::manifest(),
        chat::manifest(),
        tools::manifest(agent.capability_declarations()),
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
}

#[cfg(test)]
mod agent_tests;

#[cfg(test)]
mod official_extensions_tests;

#[cfg(test)]
mod session_tests;
