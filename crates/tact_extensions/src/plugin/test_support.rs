//! Test doubles for the hosted-plugin lifecycle, shared by this crate's tests.
//!
//! The `plugin::runtime` lifecycle tests and the Agent's tool-dispatch tests both
//! need a host that declares capabilities, and both drive registration through
//! the *production* registration path. One double is shared rather than two
//! look-alikes that could drift apart — the whole point of the seam is that a
//! test double registers exactly as a spawned stdio host would.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact::{PluginState, RuntimePluginManifest};
use tact_plugin_host::PluginHost;
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, PluginRequest, PluginResponse,
    ProtocolVersion,
};

use super::{HostedPlugin, HostedPlugins};

/// A `PluginHost` double.
pub(crate) struct DoubleHost {
    id: PluginId,
    capabilities: Vec<CapabilityDeclaration>,
    state: PluginState,
    stopped: Arc<AtomicBool>,
}

impl DoubleHost {
    pub(crate) fn new(id: &str, capabilities: Vec<CapabilityDeclaration>) -> Self {
        Self {
            id: PluginId::from(id),
            capabilities,
            state: PluginState::Running,
            stopped: Arc::new(AtomicBool::new(false)),
        }
    }
}

#[async_trait]
impl PluginHost for DoubleHost {
    fn plugin_id(&self) -> &PluginId {
        &self.id
    }

    fn protocol(&self) -> ProtocolVersion {
        ProtocolVersion::CURRENT
    }

    fn capabilities(&self) -> &[CapabilityDeclaration] {
        &self.capabilities
    }

    fn state(&self) -> PluginState {
        self.state
    }

    async fn request(&mut self, request: PluginRequest) -> anyhow::Result<PluginResponse> {
        match request {
            PluginRequest::Invoke { capability, input } => Ok(PluginResponse::Result {
                // The envelope is a `ToolCallResult` plus the raw echo: the one
                // shape serves both the serving-router test (`capability` /
                // `echo`) and the Agent's tool path (`content`), and an unknown
                // field is ignored by the decoder rather than fatal.
                output: json!({
                    "content": format!("plugin:{capability}"),
                    "effects": [],
                    "image": Value::Null,
                    "capability": capability,
                    "echo": input,
                }),
            }),
            PluginRequest::Shutdown => Ok(PluginResponse::Result {
                output: Value::Null,
            }),
            other => Err(anyhow::anyhow!("double cannot handle {other:?}")),
        }
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.state = PluginState::Stopped;
        self.stopped.store(true, Ordering::SeqCst);
        Ok(())
    }
}

/// Builds a hosted double plus the flag its stop records.
pub(crate) fn hosted_double(
    id: &str,
    capabilities: Vec<CapabilityDeclaration>,
) -> (HostedPlugin, Arc<AtomicBool>) {
    let double = DoubleHost::new(id, capabilities);
    let stopped = Arc::clone(&double.stopped);
    let manifest = RuntimePluginManifest {
        id: double.id.clone(),
        version: "1".into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: double.capabilities.clone(),
        dependencies: Vec::new(),
    };
    let host: Arc<tokio::sync::Mutex<dyn PluginHost>> = Arc::new(tokio::sync::Mutex::new(double));
    (HostedPlugin::new(host, manifest), stopped)
}

/// A [`HostedPlugins`] holding one double that declares `capabilities`.
pub(crate) fn hosted(id: &str, capabilities: Vec<CapabilityDeclaration>) -> HostedPlugins {
    HostedPlugins::from_hosts(vec![hosted_double(id, capabilities).0])
}

/// A declaration of the given kind and risk. Versions are fixed: nothing under
/// test reads them.
pub(crate) fn declaration(
    name: &str,
    kind: CapabilityKind,
    risk: CapabilityRisk,
) -> CapabilityDeclaration {
    CapabilityDeclaration {
        name: name.into(),
        kind,
        version: "1".into(),
        description: None,
        input_schema: None,
        output_schema: None,
        risk,
    }
}
