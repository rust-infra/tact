//! Built-in conversational application extension.

use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, ProtocolVersion,
};

use crate::plugin::RuntimePluginManifest;

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
