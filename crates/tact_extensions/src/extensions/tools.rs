//! Built-in native and MCP tool extension manifest.

use tact_protocol::{CapabilityDeclaration, PluginId, ProtocolVersion};

use crate::plugin::RuntimePluginManifest;

pub fn manifest(capabilities: Vec<CapabilityDeclaration>) -> RuntimePluginManifest {
    RuntimePluginManifest {
        id: PluginId::from("tact.tools"),
        version: env!("CARGO_PKG_VERSION").into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities,
    }
}
