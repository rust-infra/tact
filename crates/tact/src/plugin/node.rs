//! Node.js plugin process transport.

use std::process::Stdio;

use anyhow::{Context, Result, bail};
use serde::{Serialize, de::DeserializeOwned};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};

use tact_protocol::{
    CapabilityDeclaration, PluginId, PluginRequest, PluginRequestEnvelope, PluginResponse,
    PluginResponseEnvelope, ProtocolVersion, RequestId,
};

pub struct NodePluginProcess {
    child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
}

pub struct NodePluginHost {
    pub plugin_id: PluginId,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<CapabilityDeclaration>,
    process: NodePluginProcess,
}

impl NodePluginHost {
    pub async fn start(
        program: impl AsRef<std::path::Path>,
        args: &[String],
        plugin_id: PluginId,
        protocol: ProtocolVersion,
    ) -> Result<Self> {
        let mut process = NodePluginProcess::spawn(program, args).await?;
        let request_id = RequestId::from(format!("handshake-{}", plugin_id.as_str()));
        let handshake = PluginRequestEnvelope {
            protocol_version: protocol,
            request_id: request_id.clone(),
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
        };
        let response = process.request(&handshake).await?;
        match response.response {
            PluginResponse::HandshakeAccepted {
                protocol_version, ..
            } if protocol_version.compatible_with(protocol) => {}
            PluginResponse::HandshakeAccepted { .. } => {
                bail!("Node plugin protocol version is incompatible")
            }
            PluginResponse::Error { error } => bail!("Node plugin handshake failed: {error}"),
            other => bail!("unexpected Node plugin handshake response: {other:?}"),
        }
        let registration = PluginRequestEnvelope {
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
        };
        let response = process.request(&registration).await?;
        let capabilities = match response.response {
            PluginResponse::Registered { capabilities } => capabilities,
            PluginResponse::Error { error } => bail!("Node plugin registration failed: {error}"),
            other => bail!("unexpected Node plugin registration response: {other:?}"),
        };
        Ok(Self {
            plugin_id,
            protocol,
            capabilities,
            process,
        })
    }

    pub async fn request(&mut self, request: PluginRequest) -> Result<PluginResponse> {
        let envelope = PluginRequestEnvelope {
            protocol_version: self.protocol,
            request_id: RequestId::from(uuid::Uuid::new_v4().to_string()),
            plugin_id: self.plugin_id.clone(),
            session_id: None,
            run_id: None,
            trajectory_id: None,
            deadline: None,
            request,
        };
        Ok(self.process.request(&envelope).await?.response)
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.process.shutdown().await
    }
}

impl NodePluginProcess {
    pub async fn spawn(program: impl AsRef<std::path::Path>, args: &[String]) -> Result<Self> {
        let mut command = Command::new(program.as_ref());
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = command.spawn().context("start Node plugin process")?;
        let stdin = child
            .stdin
            .take()
            .context("Node plugin stdin unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("Node plugin stdout unavailable")?;
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
        })
    }

    pub async fn send(&mut self, request: &PluginRequestEnvelope) -> Result<()> {
        let mut encoded = serde_json::to_vec(request).context("encode plugin request")?;
        encoded.push(b'\n');
        self.stdin
            .write_all(&encoded)
            .await
            .context("write plugin request")?;
        self.stdin.flush().await.context("flush plugin request")?;
        Ok(())
    }

    pub async fn receive(&mut self) -> Result<PluginResponseEnvelope> {
        let Some(line) = self
            .stdout
            .next_line()
            .await
            .context("read plugin response")?
        else {
            let status = self.child.try_wait().context("check Node plugin status")?;
            bail!("Node plugin exited before responding: {status:?}");
        };
        serde_json::from_str(&line).context("decode plugin response")
    }

    pub async fn request(
        &mut self,
        request: &PluginRequestEnvelope,
    ) -> Result<PluginResponseEnvelope> {
        self.send(request).await?;
        self.receive().await
    }

    pub async fn shutdown(&mut self) -> Result<()> {
        self.child.kill().await.context("stop Node plugin process")
    }

    pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
        serde_json::to_vec(value).context("encode Node plugin value")
    }

    pub fn decode<T: DeserializeOwned>(value: &[u8]) -> Result<T> {
        serde_json::from_slice(value).context("decode Node plugin value")
    }
}
