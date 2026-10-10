//! Supervised stdio plugin host: handshake, registration, timeouts, restart.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use tact::{
    CapabilityHandler, CapabilityRegistration, CapabilityRouter, InvocationContext, KernelError,
    PluginState,
};

use crate::host::PluginHost;
use tact_protocol::{
    CapabilityDeclaration, ErrorCategory, PluginId, PluginRequest, PluginResponse, ProtocolVersion,
    RequestId,
};
use tokio::{sync::Mutex, time::Instant};

use crate::{process::PluginProcess, transport::make_envelope};

/// Default bound for a single host RPC. A timed out process is terminated so
/// its unread late response cannot corrupt the next request's correlation.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct StdioPluginHost {
    program: PathBuf,
    args: Vec<String>,
    plugin_id: PluginId,
    protocol: ProtocolVersion,
    capabilities: Vec<CapabilityDeclaration>,
    process: Option<PluginProcess>,
    request_timeout: Duration,
    handshake_features: Vec<String>,
    required_features: Vec<String>,
    state: PluginState,
}

impl StdioPluginHost {
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
        Self::start_with_timeout_and_features(
            program,
            args,
            plugin_id,
            protocol,
            request_timeout,
            vec![
                "capability_registration".into(),
                "events".into(),
                "cancel".into(),
            ],
        )
        .await
    }

    pub async fn start_with_timeout_and_features(
        program: impl Into<PathBuf>,
        args: &[String],
        plugin_id: PluginId,
        protocol: ProtocolVersion,
        request_timeout: Duration,
        handshake_features: Vec<String>,
    ) -> Result<Self> {
        Self::start_with_feature_requirements(
            program,
            args,
            plugin_id,
            protocol,
            request_timeout,
            handshake_features,
            Vec::new(),
        )
        .await
    }

    pub async fn start_with_feature_requirements(
        program: impl Into<PathBuf>,
        args: &[String],
        plugin_id: PluginId,
        protocol: ProtocolVersion,
        request_timeout: Duration,
        handshake_features: Vec<String>,
        required_features: Vec<String>,
    ) -> Result<Self> {
        let mut host = Self {
            program: program.into(),
            args: args.to_vec(),
            plugin_id,
            protocol,
            capabilities: Vec::new(),
            process: None,
            request_timeout,
            handshake_features,
            required_features,
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
        let mut process = PluginProcess::spawn(&self.program, &self.args)
            .await
            .context("start plugin process")?;
        let handshake = make_envelope(
            self.protocol,
            &self.plugin_id,
            RequestId::from(format!("handshake-{}", uuid::Uuid::new_v4())),
            PluginRequest::Handshake {
                protocol_version: self.protocol,
                features: self.handshake_features.clone(),
            },
        );
        let response = process
            .request(&handshake, STARTUP_TIMEOUT)
            .await
            .context("plugin handshake")?;
        match response.response {
            PluginResponse::HandshakeAccepted {
                protocol_version,
                features,
            } if protocol_version.compatible_with(self.protocol) => {
                if let Some(required) = self
                    .required_features
                    .iter()
                    .find(|required| !features.contains(required))
                {
                    process.terminate().await;
                    self.state = PluginState::Failed;
                    anyhow::bail!("plugin does not support required feature: {required}");
                }
            }
            PluginResponse::HandshakeAccepted { .. } => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("plugin protocol version is incompatible");
            }
            PluginResponse::Error { error } => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("plugin handshake failed: {error}");
            }
            other => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("unexpected plugin handshake response: {other:?}");
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
            .context("plugin registration")?;
        let capabilities = match response.response {
            PluginResponse::Registered { capabilities } => capabilities,
            PluginResponse::Error { error } => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("plugin registration failed: {error}");
            }
            other => {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("unexpected plugin registration response: {other:?}");
            }
        };
        let mut names = std::collections::BTreeSet::new();
        for capability in &capabilities {
            capability
                .validate()
                .map_err(|error| anyhow!("invalid plugin capability: {error}"))?;
            if !names.insert(capability.name.clone()) {
                process.terminate().await;
                self.state = PluginState::Failed;
                anyhow::bail!("plugin declared duplicate capability: {}", capability.name);
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
        if matches!(
            &request,
            PluginRequest::Invoke { .. } | PluginRequest::HostCallResult { .. }
        ) {
            return Err(KernelError::permission_denied(
                "capability invocation must go through CapabilityRouter",
            )
            .with_plugin_id(self.plugin_id.clone()));
        }
        let request_id = RequestId::from(format!("host-{}", uuid::Uuid::new_v4()));
        let response = self.request_with_id(request_id, request).await?;
        match &response {
            PluginResponse::HostCall { .. } => {
                self.fail().await;
                return Err(KernelError::new(
                    ErrorCategory::PluginCrashed,
                    "plugin requested a host call outside capability invocation",
                    "plugin_host",
                    false,
                )
                .with_plugin_id(self.plugin_id.clone()));
            }
            PluginResponse::Event { event } => {
                if let Err(message) = event.validate_plugin_event(self.plugin_id.as_str()) {
                    self.fail().await;
                    return Err(KernelError::new(
                        ErrorCategory::InvalidRequest,
                        message,
                        "plugin_host",
                        false,
                    )
                    .with_plugin_id(self.plugin_id.clone()));
                }
            }
            _ => {}
        }
        Ok(response)
    }

    async fn request_with_id(
        &mut self,
        request_id: RequestId,
        request: PluginRequest,
    ) -> Result<PluginResponse, KernelError> {
        if self.state != PluginState::Running {
            return Err(plugin_unavailable("plugin is not running", &self.plugin_id));
        }
        let envelope = make_envelope(self.protocol, &self.plugin_id, request_id, request);
        let Some(process) = self.process.as_mut() else {
            return Err(plugin_unavailable(
                "plugin process is unavailable",
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
                    KernelError::new(category, error.to_string(), "plugin_host", true)
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
            return Err(plugin_unavailable("plugin is not running", &self.plugin_id));
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
        let process = self
            .process
            .as_mut()
            .ok_or_else(|| plugin_unavailable("plugin process is unavailable", &self.plugin_id))?;
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
                    KernelError::new(category, error.to_string(), "plugin_host", true)
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

    pub async fn interrupt(&mut self) {
        self.fail().await;
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
        Self::register_with_router_and_host_calls(host, router, Arc::new(DenyHostCalls))
    }

    pub fn register_with_router_and_host_calls(
        host: Arc<Mutex<Self>>,
        router: &CapabilityRouter,
        host_calls: Arc<dyn HostCallService>,
    ) -> Result<(), KernelError> {
        let (declarations, request_timeout) = host
            .try_lock()
            .map_err(|_| {
                KernelError::new(
                    ErrorCategory::InternalError,
                    "plugin host is busy",
                    "plugin_host",
                    true,
                )
            })
            .map(|host| (host.capabilities.clone(), host.request_timeout))?;
        let own_capabilities = Arc::new(
            declarations
                .iter()
                .map(|declaration| declaration.name.clone())
                .collect::<BTreeSet<_>>(),
        );
        let registrations = declarations
            .into_iter()
            .map(|declaration| {
                let capability = declaration.name.clone();
                CapabilityRegistration::new(
                    declaration,
                    Arc::new(HostCapabilityHandler {
                        host: Arc::clone(&host),
                        host_calls: Arc::clone(&host_calls),
                        router: router.clone(),
                        own_capabilities: Arc::clone(&own_capabilities),
                        request_timeout,
                        capability,
                    }),
                )
            })
            .collect();
        router.register_many(registrations)
    }
}

/// Mediates guest-initiated host calls made during a routed capability call.
#[async_trait]
pub trait HostCallService: Send + Sync {
    async fn handle(
        &self,
        context: &InvocationContext,
        router: &CapabilityRouter,
        capability: &str,
        input: Value,
    ) -> Result<Value, KernelError>;
}

struct DenyHostCalls;

#[async_trait]
impl HostCallService for DenyHostCalls {
    async fn handle(
        &self,
        _context: &InvocationContext,
        _router: &CapabilityRouter,
        capability: &str,
        _input: Value,
    ) -> Result<Value, KernelError> {
        Err(KernelError::permission_denied(format!(
            "plugin host call is unavailable: {capability}"
        )))
    }
}

#[async_trait]
impl PluginHost for StdioPluginHost {
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
        StdioPluginHost::request(self, request)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.stop().await
    }
}

struct HostCapabilityHandler {
    host: Arc<Mutex<StdioPluginHost>>,
    host_calls: Arc<dyn HostCallService>,
    router: CapabilityRouter,
    own_capabilities: Arc<BTreeSet<String>>,
    request_timeout: Duration,
    capability: String,
}

#[async_trait]
impl CapabilityHandler for HostCapabilityHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let context = if context.deadline().is_none() {
            context.with_timeout(self.request_timeout)
        } else {
            context
        };
        let cancellation = context.cancellation_token();
        let deadline = context.deadline();
        let cleanup_host = Arc::clone(&self.host);
        let completed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let cleanup_completed = Arc::clone(&completed);
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
            if !cleanup_completed.load(std::sync::atomic::Ordering::Acquire) {
                cleanup_host.lock().await.interrupt().await;
            }
        });
        let mut host = self.host.lock().await;
        let result = self.invoke_plugin(&mut host, &context, input).await;
        completed.store(true, std::sync::atomic::Ordering::Release);
        cleanup.abort();
        result
    }
}

impl HostCapabilityHandler {
    async fn invoke_plugin(
        &self,
        host: &mut StdioPluginHost,
        context: &InvocationContext,
        input: Value,
    ) -> Result<Value, KernelError> {
        let mut response = host
            .request_with_context(
                context,
                PluginRequest::Invoke {
                    capability: self.capability.clone(),
                    input,
                },
            )
            .await?;
        for _ in 0..64 {
            match response {
                PluginResponse::HostCall {
                    host_request_id,
                    capability,
                    input,
                } => {
                    let result = if let Some(name) = capability
                        .strip_prefix("capability:")
                        .filter(|name| self.own_capabilities.contains(*name))
                    {
                        Err(KernelError::permission_denied(format!(
                            "plugin cannot recursively invoke its own capability: {name}"
                        )))
                    } else {
                        self.host_calls
                            .handle(context, &self.router, &capability, input)
                            .await
                    };
                    let (output, error) = match result {
                        Ok(output) => (Some(output), None),
                        Err(error) => {
                            let mut error = error.into_protocol_error();
                            error.request_id = Some(context.request_id().clone());
                            error.plugin_id = Some(context.plugin_id().clone());
                            (None, Some(error))
                        }
                    };
                    response = host
                        .request_with_context(
                            context,
                            PluginRequest::HostCallResult {
                                host_request_id,
                                output,
                                error,
                            },
                        )
                        .await?;
                }
                PluginResponse::Result { output } => return Ok(output),
                PluginResponse::Error { error } => {
                    return Err(KernelError::from_protocol(error));
                }
                other => {
                    return Err(KernelError::new(
                        ErrorCategory::PluginCrashed,
                        format!("unexpected plugin invocation response: {other:?}"),
                        "plugin_host",
                        true,
                    ));
                }
            }
        }
        host.interrupt().await;
        Err(KernelError::new(
            ErrorCategory::PluginCrashed,
            "plugin exceeded the host-call limit",
            "plugin_host",
            false,
        ))
    }
}

fn plugin_unavailable(message: impl Into<String>, plugin_id: &PluginId) -> KernelError {
    KernelError::new(
        ErrorCategory::PluginUnavailable,
        message,
        "plugin_host",
        true,
    )
    .with_plugin_id(plugin_id.clone())
}
