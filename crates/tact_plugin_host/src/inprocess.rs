//! In-process Rust plugin host.
//!
//! A built-in Rust extension is registered with the same manifest, capability
//! router, permission checks, events, and trajectory hooks as an external host;
//! this wrapper exposes it through the same [`PluginHost`] lifecycle, so a
//! supervisor can address a Node, a WASM, and a built-in Rust extension
//! uniformly. In-process status buys no bypass: `Invoke` goes through
//! `CapabilityRouter::invoke`, so permission, events, and trajectory all run.

use anyhow::{Result, bail};
use async_trait::async_trait;
use tact::{CapabilityRouter, PluginState, RuntimeContext, RuntimePluginManifest};
use tact_protocol::{
    CapabilityDeclaration, PluginId, PluginRequest, PluginResponse, ProtocolVersion, RequestId,
};

use crate::host::PluginHost;

pub struct InProcessPluginHost {
    manifest: RuntimePluginManifest,
    router: CapabilityRouter,
    runtime: RuntimeContext,
    state: PluginState,
}

impl InProcessPluginHost {
    #[must_use]
    pub fn new(
        manifest: RuntimePluginManifest,
        router: CapabilityRouter,
        runtime: RuntimeContext,
    ) -> Self {
        Self {
            manifest,
            router,
            runtime,
            state: PluginState::Running,
        }
    }

    #[must_use]
    pub fn manifest(&self) -> &RuntimePluginManifest {
        &self.manifest
    }
}

#[async_trait]
impl PluginHost for InProcessPluginHost {
    fn plugin_id(&self) -> &PluginId {
        &self.manifest.id
    }

    fn protocol(&self) -> ProtocolVersion {
        self.manifest.protocol
    }

    fn capabilities(&self) -> &[CapabilityDeclaration] {
        &self.manifest.capabilities
    }

    fn state(&self) -> PluginState {
        self.state
    }

    async fn request(&mut self, request: PluginRequest) -> Result<PluginResponse> {
        match request {
            PluginRequest::Handshake {
                protocol_version, ..
            } => {
                if !protocol_version.compatible_with(self.manifest.protocol) {
                    bail!("in-process plugin protocol version is incompatible");
                }
                self.state = PluginState::Running;
                Ok(PluginResponse::HandshakeAccepted {
                    protocol_version: self.manifest.protocol,
                    features: vec![
                        "capability_registration".into(),
                        "events".into(),
                        "cancel".into(),
                    ],
                })
            }
            PluginRequest::Register { .. } => Ok(PluginResponse::Registered {
                capabilities: self.manifest.capabilities.clone(),
            }),
            PluginRequest::Invoke { capability, input } => {
                if self.state != PluginState::Running {
                    bail!("in-process plugin {} is not running", self.manifest.id);
                }
                let invocation = self.runtime.invocation(
                    RequestId::from(uuid::Uuid::new_v4().to_string()),
                    self.manifest.id.clone(),
                    "plugin",
                );
                match self.router.invoke(&capability, invocation, input).await {
                    Ok(output) => Ok(PluginResponse::Result { output }),
                    Err(error) => Ok(PluginResponse::Error {
                        error: error.into_protocol_error(),
                    }),
                }
            }
            PluginRequest::Shutdown => {
                self.state = PluginState::Stopped;
                Ok(PluginResponse::Result {
                    output: serde_json::Value::Null,
                })
            }
            other => bail!("in-process plugin does not handle {other:?}"),
        }
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.state = PluginState::Stopped;
        Ok(())
    }
}
