//! WASM plugin host over the same line-oriented protocol as external hosts.
//!
//! The Runtime intentionally does not link a WASM engine. A deployment can
//! provide Wasmtime (or a compatible runner) as the process executable, while
//! protocol, permission and lifecycle semantics remain owned by Tact.

use anyhow::{Context, Result, bail};
use tact_protocol::{
    CapabilityDeclaration, PluginId, PluginRequest, PluginRequestEnvelope, PluginResponse,
    ProtocolVersion, RequestId,
};

use super::node::NodePluginProcess;
use super::{PluginHost, PluginState};

pub struct WasmPluginHost {
    pub plugin_id: PluginId,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<CapabilityDeclaration>,
    process: NodePluginProcess,
    state: PluginState,
}

impl WasmPluginHost {
    pub async fn start(
        runner: impl AsRef<std::path::Path>,
        module: impl AsRef<std::path::Path>,
        plugin_id: PluginId,
        protocol: ProtocolVersion,
    ) -> Result<Self> {
        let args = vec!["run".to_string(), module.as_ref().display().to_string()];
        let mut process = NodePluginProcess::spawn(runner, &args).await?;
        let response = process
            .request(&PluginRequestEnvelope {
                protocol_version: protocol,
                request_id: RequestId::from(format!("handshake-{}", plugin_id.as_str())),
                plugin_id: plugin_id.clone(),
                session_id: None,
                run_id: None,
                trajectory_id: None,
                deadline: None,
                request: PluginRequest::Handshake {
                    protocol_version: protocol,
                    features: vec![
                        "capability_registration".into(),
                        "events".into(),
                        "cancel".into(),
                    ],
                },
            })
            .await
            .context("WASM plugin handshake")?;
        match response.response {
            PluginResponse::HandshakeAccepted {
                protocol_version, ..
            } if protocol_version.compatible_with(protocol) => {}
            PluginResponse::Error { error } => bail!("WASM plugin handshake failed: {error}"),
            _ => bail!("WASM plugin returned an invalid handshake response"),
        }
        let response = process
            .request(&PluginRequestEnvelope {
                protocol_version: protocol,
                request_id: RequestId::from(format!("register-{}", plugin_id.as_str())),
                plugin_id: plugin_id.clone(),
                session_id: None,
                run_id: None,
                trajectory_id: None,
                deadline: None,
                request: PluginRequest::Register {
                    capabilities: Vec::new(),
                },
            })
            .await
            .context("WASM plugin registration")?;
        let capabilities = match response.response {
            PluginResponse::Registered { capabilities } => capabilities,
            PluginResponse::Error { error } => bail!("WASM plugin registration failed: {error}"),
            _ => bail!("WASM plugin returned an invalid registration response"),
        };
        for capability in &capabilities {
            capability
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid WASM plugin capability: {error}"))?;
        }
        Ok(Self {
            plugin_id,
            protocol,
            capabilities,
            process,
            state: PluginState::Running,
        })
    }

    pub async fn request(&mut self, request: PluginRequest) -> Result<PluginResponse> {
        let response = self
            .process
            .request(&PluginRequestEnvelope {
                protocol_version: self.protocol,
                request_id: RequestId::from(uuid::Uuid::new_v4().to_string()),
                plugin_id: self.plugin_id.clone(),
                session_id: None,
                run_id: None,
                trajectory_id: None,
                deadline: None,
                request,
            })
            .await?;
        Ok(response.response)
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.state = PluginState::Stopping;
        let result = self.process.shutdown().await;
        self.state = if result.is_ok() {
            PluginState::Stopped
        } else {
            PluginState::Failed
        };
        result
    }
}

#[async_trait::async_trait]
impl PluginHost for WasmPluginHost {
    fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    fn protocol(&self) -> ProtocolVersion {
        self.protocol
    }

    fn capabilities(&self) -> &[CapabilityDeclaration] {
        &self.capabilities
    }

    fn state(&self) -> PluginState {
        self.state
    }

    async fn request(&mut self, request: PluginRequest) -> Result<PluginResponse> {
        WasmPluginHost::request(self, request).await
    }

    async fn shutdown(&mut self) -> Result<()> {
        WasmPluginHost::shutdown(self).await
    }
}
