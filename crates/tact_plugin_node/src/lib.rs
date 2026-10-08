//! Supervised Node.js plugin host using the shared Tact plugin protocol.

mod process;
mod transport;

use std::{path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use tact::{
    kernel::{CapabilityHandler, CapabilityRouter, InvocationContext, KernelError},
    plugin::{PluginHost, PluginState},
};
use tact_protocol::{
    CapabilityDeclaration, ErrorCategory, PluginId, PluginRequest, PluginResponse, ProtocolVersion,
    RequestId, RuntimeEvent, StepId,
};
use tokio::{sync::Mutex, time::Instant};

use crate::{process::NodePluginProcess, transport::make_envelope};

/// Default bound for a single host RPC. A timed out process is terminated so
/// its unread late response cannot corrupt the next request's correlation.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct NodePluginHost {
    program: PathBuf,
    args: Vec<String>,
    plugin_id: PluginId,
    protocol: ProtocolVersion,
    capabilities: Vec<CapabilityDeclaration>,
    process: Option<NodePluginProcess>,
    request_timeout: Duration,
    state: PluginState,
}

impl NodePluginHost {
    pub async fn start(
        program: impl Into<PathBuf>,
        args: &[String],
        plugin_id: PluginId,
        protocol: ProtocolVersion,
    ) -> Result<Self> {
        Self::start_with_timeout(program, args, plugin_id, protocol, DEFAULT_REQUEST_TIMEOUT).await
    }

    pub async fn start_with_timeout(
        program: impl Into<PathBuf>,
        args: &[String],
        plugin_id: PluginId,
        protocol: ProtocolVersion,
        request_timeout: Duration,
    ) -> Result<Self> {
        let mut host = Self {
            program: program.into(),
            args: args.to_vec(),
            plugin_id,
            protocol,
            capabilities: Vec::new(),
            process: None,
            request_timeout,
            state: PluginState::Registered,
        };
        host.start_process().await?;
        Ok(host)
    }

    async fn start_process(&mut self) -> Result<()> {
        self.state = PluginState::Registered;
        self.capabilities.clear();
        let result = self.start_process_inner().await;
        if result.is_err() {
            self.state = PluginState::Failed;
            if let Some(mut process) = self.process.take() {
                process.terminate().await;
            }
        }
        result
    }

    async fn start_process_inner(&mut self) -> Result<()> {
        let mut process = NodePluginProcess::spawn(&self.program, &self.args)
            .await
            .context("start Node plugin")?;
        let handshake = make_envelope(
            self.protocol,
            &self.plugin_id,
            RequestId::from(format!("handshake-{}", uuid::Uuid::new_v4())),
            PluginRequest::Handshake {
                protocol_version: self.protocol,
                features: vec![
                    "capability_registration".into(),
                    "events".into(),
                    "cancel".into(),
                ],
            },
        );
        let response = process
            .request(&handshake, STARTUP_TIMEOUT)
            .await
            .context("Node plugin handshake")?;
        match response.response {
            PluginResponse::HandshakeAccepted {
                protocol_version, ..
            } if protocol_version.compatible_with(self.protocol) => {}
            PluginResponse::HandshakeAccepted { .. } => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("Node plugin protocol version is incompatible");
            }
            PluginResponse::Error { error } => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("Node plugin handshake failed: {error}");
            }
            other => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("unexpected Node plugin handshake response: {other:?}");
            }
        }

        let registration = make_envelope(
            self.protocol,
            &self.plugin_id,
            RequestId::from(format!("register-{}", uuid::Uuid::new_v4())),
            PluginRequest::Register {
                capabilities: Vec::new(),
            },
        );
        let response = process
            .request(&registration, STARTUP_TIMEOUT)
            .await
            .context("Node plugin registration")?;
        let capabilities = match response.response {
            PluginResponse::Registered { capabilities } => capabilities,
            PluginResponse::Error { error } => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("Node plugin registration failed: {error}");
            }
            other => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("unexpected Node plugin registration response: {other:?}");
            }
        };
        let mut names = std::collections::BTreeSet::new();
        for capability in &capabilities {
            capability
                .validate()
                .map_err(|error| anyhow!("invalid Node plugin capability: {error}"))?;
            if !names.insert(capability.name.clone()) {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!(
                    "Node plugin declared duplicate capability: {}",
                    capability.name
                );
            }
        }
        self.capabilities = capabilities;
        self.process = Some(process);
        self.state = PluginState::Running;
        Ok(())
    }

    pub fn capabilities(&self) -> &[CapabilityDeclaration] {
        &self.capabilities
    }

    pub fn state(&self) -> PluginState {
        self.state
    }

    pub async fn request(&mut self, request: PluginRequest) -> Result<PluginResponse, KernelError> {
        let request_id = RequestId::from(format!("node-{}", uuid::Uuid::new_v4()));
        self.request_with_id(request_id, request).await
    }

    async fn request_with_id(
        &mut self,
        request_id: RequestId,
        request: PluginRequest,
    ) -> Result<PluginResponse, KernelError> {
        if self.state != PluginState::Running {
            return Err(plugin_unavailable(
                "Node plugin is not running",
                &self.plugin_id,
            ));
        }
        let envelope = make_envelope(self.protocol, &self.plugin_id, request_id, request);
        let Some(process) = self.process.as_mut() else {
            return Err(plugin_unavailable(
                "Node plugin process is unavailable",
                &self.plugin_id,
            ));
        };
        match process.request(&envelope, self.request_timeout).await {
            Ok(response) => Ok(response.response),
            Err(error) => {
                let category = if error.to_string().contains("timed out") {
                    ErrorCategory::Timeout
                } else {
                    ErrorCategory::PluginCrashed
                };
                self.fail().await;
                Err(
                    KernelError::new(category, error.to_string(), "node_plugin", true)
                        .with_plugin_id(self.plugin_id.clone()),
                )
            }
        }
    }

    async fn request_with_context(
        &mut self,
        context: &InvocationContext,
        request: PluginRequest,
    ) -> Result<PluginResponse, KernelError> {
        if self.state != PluginState::Running {
            return Err(plugin_unavailable(
                "Node plugin is not running",
                &self.plugin_id,
            ));
        }
        let mut envelope = make_envelope(
            self.protocol,
            &self.plugin_id,
            context.request_id().clone(),
            request,
        );
        envelope.session_id = context.session_id().cloned();
        envelope.run_id = context.run_id().cloned();
        envelope.trajectory_id = context.trajectory_id().cloned();
        let cancellation = context.cancellation_token();
        let deadline = context
            .deadline()
            .unwrap_or_else(|| Instant::now() + self.request_timeout);
        let process = self.process.as_mut().ok_or_else(|| {
            plugin_unavailable("Node plugin process is unavailable", &self.plugin_id)
        })?;
        let response = tokio::select! {
            _ = cancellation.cancelled() => {
                self.fail().await;
                return Err(KernelError::cancelled().with_plugin_id(self.plugin_id.clone()));
            }
            _ = tokio::time::sleep_until(deadline) => {
                self.fail().await;
                return Err(KernelError::timeout().with_plugin_id(self.plugin_id.clone()));
            }
            response = process.request(&envelope, self.request_timeout) => response,
        };
        match response {
            Ok(response) => Ok(response.response),
            Err(error) => {
                self.fail().await;
                let category = if error.to_string().contains("timed out") {
                    ErrorCategory::Timeout
                } else {
                    ErrorCategory::PluginCrashed
                };
                Err(
                    KernelError::new(category, error.to_string(), "node_plugin", true)
                        .with_plugin_id(self.plugin_id.clone()),
                )
            }
        }
    }

    async fn fail(&mut self) {
        self.state = PluginState::Failed;
        if let Some(mut process) = self.process.take() {
            process.terminate().await;
        }
    }

    pub async fn stop(&mut self) -> Result<()> {
        if matches!(self.state, PluginState::Stopped | PluginState::Registered) {
            self.state = PluginState::Stopped;
            return Ok(());
        }
        self.state = PluginState::Stopping;
        let result = if let Some(mut process) = self.process.take() {
            let request = make_envelope(
                self.protocol,
                &self.plugin_id,
                RequestId::from(format!("shutdown-{}", uuid::Uuid::new_v4())),
                PluginRequest::Shutdown,
            );
            let result = process
                .request(&request, SHUTDOWN_TIMEOUT)
                .await
                .map(|_| ());
            if tokio::time::timeout(SHUTDOWN_TIMEOUT, process.wait())
                .await
                .is_err()
            {
                process.terminate().await;
            }
            result
        } else {
            Ok(())
        };
        self.state = if result.is_ok() {
            PluginState::Stopped
        } else {
            PluginState::Failed
        };
        result
    }

    pub async fn restart(&mut self) -> Result<()> {
        if let Some(mut process) = self.process.take() {
            process.terminate().await;
        }
        self.state = PluginState::Stopped;
        self.start_process().await
    }

    pub fn register_with_router(
        host: Arc<Mutex<Self>>,
        router: &CapabilityRouter,
    ) -> Result<(), KernelError> {
        let declarations = host
            .try_lock()
            .map_err(|_| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    "Node host is busy",
                    "node_plugin",
                    true,
                )
            })?
            .capabilities
            .clone();
        for declaration in declarations {
            let capability = declaration.name.clone();
            router.register_handler(
                declaration,
                NodeCapabilityHandler {
                    host: Arc::clone(&host),
                    capability,
                },
            )?;
        }
        Ok(())
    }
}

#[async_trait]
impl PluginHost for NodePluginHost {
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
        NodePluginHost::request(self, request)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.stop().await
    }
}

struct NodeCapabilityHandler {
    host: Arc<Mutex<NodePluginHost>>,
    capability: String,
}

#[async_trait]
impl CapabilityHandler for NodeCapabilityHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let step_id = StepId::from(context.request_id().as_str());
        if let Some(run_id) = context.run_id() {
            let event = RuntimeEvent::ToolCallStarted {
                run_id: run_id.clone(),
                step_id: step_id.clone(),
                tool: self.capability.clone(),
            };
            context
                .trajectory()
                .append(context.trajectory_id(), Some(run_id), event.clone())
                .await?;
            context.events().publish(event).await?;
        }

        let cancellation = context.cancellation_token();
        let deadline = context.deadline();
        let cleanup_host = Arc::clone(&self.host);
        let cleanup = tokio::spawn(async move {
            tokio::select! {
                _ = cancellation.cancelled() => {},
                _ = async {
                    if let Some(deadline) = deadline {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {},
            }
            cleanup_host.lock().await.fail().await;
        });
        let mut host = self.host.lock().await;
        let result = host
            .request_with_context(
                &context,
                PluginRequest::Invoke {
                    capability: self.capability.clone(),
                    input,
                },
            )
            .await;
        cleanup.abort();
        let result = match result {
            Ok(PluginResponse::Result { output }) => Ok(output),
            Ok(PluginResponse::Error { error }) => Err(KernelError::from_protocol(error)),
            Ok(other) => Err(KernelError::new(
                ErrorCategory::PluginCrashed,
                format!("unexpected Node plugin invocation response: {other:?}"),
                "node_plugin",
                true,
            )),
            Err(error) => Err(error),
        };
        if let Some(run_id) = context.run_id() {
            let event = RuntimeEvent::ToolCallFinished {
                run_id: run_id.clone(),
                step_id,
                success: result.is_ok(),
            };
            context
                .trajectory()
                .append(context.trajectory_id(), Some(run_id), event.clone())
                .await?;
            context.events().publish(event).await?;
        }
        result
    }
}

fn plugin_unavailable(message: impl Into<String>, plugin_id: &PluginId) -> KernelError {
    KernelError::new(
        ErrorCategory::PluginUnavailable,
        message,
        "node_plugin",
        true,
    )
    .with_plugin_id(plugin_id.clone())
}
