//! Official extensions registered through the same protocol as external ones.

use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, ProtocolVersion,
};

use crate::{Agent, plugin::RuntimePluginManifest};

pub mod agent;
pub mod session;

pub mod chat {
    use super::*;
    pub fn manifest() -> RuntimePluginManifest {
        RuntimePluginManifest {
            id: PluginId::from("tact.chat"),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: ProtocolVersion::CURRENT,
            capabilities: vec![CapabilityDeclaration {
                name: "chat.start_run".into(),
                kind: CapabilityKind::App,
                version: "1".into(),
                description: Some("Start and follow a conversational run".into()),
                input_schema: None,
                output_schema: None,
                risk: CapabilityRisk::Medium,
            }],
        }
    }
}

pub mod tools {
    use super::*;
    pub fn manifest(capabilities: Vec<CapabilityDeclaration>) -> RuntimePluginManifest {
        RuntimePluginManifest {
            id: PluginId::from("tact.tools"),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: ProtocolVersion::CURRENT,
            capabilities,
        }
    }
}

pub mod workflow {
    use super::*;
    pub fn manifest() -> RuntimePluginManifest {
        RuntimePluginManifest {
            id: PluginId::from("tact.workflow"),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: ProtocolVersion::CURRENT,
            capabilities: vec![CapabilityDeclaration {
                name: "workflow.run".into(),
                kind: CapabilityKind::Command,
                version: "1".into(),
                description: Some("Run a multi-step workflow".into()),
                input_schema: None,
                output_schema: None,
                risk: CapabilityRisk::Medium,
            }],
        }
    }
}

pub fn official_manifests(agent: &Agent) -> Vec<RuntimePluginManifest> {
    vec![
        agent::manifest(),
        session::manifest(),
        chat::manifest(),
        tools::manifest(agent.capability_declarations()),
        workflow::manifest(),
    ]
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
mod session_tests;
