//! Supervised stdio plugin host: handshake, registration, timeouts, restart.

use std::{collections::BTreeSet, path::PathBuf, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde_json::Value;
use tact::{
    CapabilityHandler, CapabilityRegistration, CapabilityRouter, InvocationContext, KernelError,
    PluginState, RuntimeContext,
};

use crate::host::PluginHost;
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, ErrorCategory, PluginId, PluginRequest, PluginResponse,
    ProtocolVersion, RequestId,
};
use tokio::{sync::Mutex, time::Instant};

use crate::{process::PluginProcess, transport::make_envelope};

/// Default bound for a single host RPC. A timed out process is terminated so
/// its unread late response cannot corrupt the next request's correlation.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
pub const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

/// The handshake features every stdio host asks for by default.
///
/// Exposed so a caller that must *add* a feature builds on the same list instead
/// of restating it: `tact_extensions` starts a hosted runtime plugin with
/// `host_calls` on top of this list when the plugin's manifest declares grants.
/// [`StdioPluginHost::start_with_timeout`] asks for exactly this, so the default
/// handshake cannot drift away from what a caller believes it is extending.
#[must_use]
pub fn default_handshake_features() -> Vec<String> {
    vec![
        "capability_registration".into(),
        "events".into(),
        "cancel".into(),
    ]
}

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
            default_handshake_features(),
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

    /// The stdio host's registration is the generic [`register_host_with_router`]
    /// over the `PluginHost` boundary: nothing here is stdio-specific, and going
    /// through the trait is what lets a host double register exactly as
    /// production does.
    pub fn register_with_router_and_host_calls(
        host: Arc<Mutex<Self>>,
        router: &CapabilityRouter,
        host_calls: Arc<dyn HostCallService>,
    ) -> Result<(), KernelError> {
        register_host_with_router(host, router, host_calls)
    }
}

/// Registers every capability a host declares on `router`, under the caller's
/// host-call policy.
///
/// Generic over [`PluginHost`] rather than the stdio host on purpose: a test
/// double registers through this same function, so what the registration tests
/// exercise *is* the production path. Registration is a single `register_many`
/// batch, so a name already on the router fails the whole host and none of its
/// capabilities are inserted — the router's duplicate rules stay intact.
pub fn register_host_with_router(
    host: Arc<Mutex<dyn PluginHost>>,
    router: &CapabilityRouter,
    host_calls: Arc<dyn HostCallService>,
) -> Result<(), KernelError> {
    let (declarations, request_timeout) = try_declared_capabilities(&host)?;
    register_declared_capabilities(
        host,
        declarations,
        request_timeout,
        router,
        host_calls,
        |_| true,
    )
}

/// Registers a host's declared **serving** capabilities: everything that is not
/// a [`CapabilityKind::Tool`].
///
/// A `Tool` declaration is deliberately kept *off* the serving router. A tool is
/// dispatched through the Agent's per-wave tool router, and that is the only
/// path carrying the tool preflight — the sensitive-target guard and the one-use
/// approval ticket the tool router demands before a handler runs. Registering
/// the same tool here as well would hand out exactly the authority those two
/// checks exist to withhold: the serving router's policy is the *capability*
/// policy (host control plane allow-listed, everything else by session policy),
/// not the tool policy. This is the entry the hosted-plugin bootstrap uses for
/// the `Command` / `App` / `Service` kinds.
pub fn register_host_serving_capabilities_with_router(
    host: Arc<Mutex<dyn PluginHost>>,
    router: &CapabilityRouter,
    host_calls: Arc<dyn HostCallService>,
) -> Result<(), KernelError> {
    let (declarations, request_timeout) = try_declared_capabilities(&host)?;
    register_declared_capabilities(
        host,
        declarations,
        request_timeout,
        router,
        host_calls,
        |declaration| declaration.kind != CapabilityKind::Tool,
    )
}

/// Registers only a host's [`CapabilityKind::Tool`] declarations on `router`,
/// for the Agent's per-wave tool router.
///
/// Async, unlike the two bootstrap entries: the declarations live behind the
/// host's `tokio::sync::Mutex`, and a tool router is built on the turn's hot
/// path, where a host that is momentarily locked (its previous request has just
/// returned) must be waited out rather than failed. "The host is busy" is a
/// useful report at bootstrap and the wrong answer here.
pub async fn register_host_tools_with_router(
    host: Arc<Mutex<dyn PluginHost>>,
    router: &CapabilityRouter,
    host_calls: Arc<dyn HostCallService>,
) -> Result<(), KernelError> {
    let (declarations, request_timeout) = {
        let host = host.lock().await;
        (host.capabilities().to_vec(), host.request_timeout())
    };
    register_declared_capabilities(
        host,
        declarations,
        request_timeout,
        router,
        host_calls,
        |declaration| declaration.kind == CapabilityKind::Tool,
    )
}

/// The declarations and request timeout of `host`, read without waiting.
///
/// Shared by the two bootstrap entries because neither may block on a host that
/// is mid-request: a locked host at that point is a startup ordering bug to
/// report, not a wait.
fn try_declared_capabilities(
    host: &Arc<Mutex<dyn PluginHost>>,
) -> Result<(Vec<CapabilityDeclaration>, Duration), KernelError> {
    host.try_lock()
        .map_err(|_| {
            KernelError::new(
                ErrorCategory::InternalError,
                "plugin host is busy",
                "plugin_host",
                true,
            )
        })
        .map(|host| (host.capabilities().to_vec(), host.request_timeout()))
}

/// The one registration body, shared by all three public entries: `keep`
/// selects which declarations are registered, so "all", "serving only" and
/// "tools only" differ by a predicate rather than by three copies of the
/// handler wiring.
fn register_declared_capabilities(
    host: Arc<Mutex<dyn PluginHost>>,
    declarations: Vec<CapabilityDeclaration>,
    request_timeout: Duration,
    router: &CapabilityRouter,
    host_calls: Arc<dyn HostCallService>,
    keep: impl Fn(&CapabilityDeclaration) -> bool,
) -> Result<(), KernelError> {
    // `own_capabilities` is the *whole* declared set, not the registered
    // subset: the guard it feeds — "a plugin cannot recursively invoke its own
    // capability" — is a property of the plugin, not of which router this
    // registration targeted.
    let own_capabilities = Arc::new(
        declarations
            .iter()
            .map(|declaration| declaration.name.clone())
            .collect::<BTreeSet<_>>(),
    );
    let registrations = declarations
        .into_iter()
        .filter(keep)
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

/// The default host-call policy: every guest host call is denied, so a plugin
/// cannot reach the Kernel services until a product surface grants it. Public so
/// a caller that registers a host directly lands on the same fail-closed policy
/// the stdio host uses by default.
pub struct DenyHostCalls;

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

/// The host-call policy for a plugin whose manifest **declares** the
/// capabilities it may invoke.
///
/// Two checks stand between the plugin and the capability, and they answer
/// different questions:
///
/// - The grant list is the *manifest's* own statement. A name the package did
///   not declare is refused by name, before any policy is consulted, so the
///   declaration is what bounds the reach and a plugin cannot talk its way into
///   a capability its author never named.
/// - The kind check reads the declaration off the **serving** router and refuses
///   anything that is not a [`CapabilityKind::Service`]. One check, four
///   exclusions: a `Tool` must keep its per-wave preflight path (the
///   sensitive-target guard and the one-use approval ticket live there, not
///   here), and `Command` / `App` / `View` / `EventHandler` are the product
///   capabilities of an extension the plugin is not.
///
/// The router is the right source for that fact. `tact_plugin_wasm`'s runner
/// keeps a hard-coded capability list (`host_functions.rs`), which is right for
/// a sandbox that must decide *before* any router exists; here the Kernel has
/// already registered its services on the router this call lands on, so asking
/// it is what keeps this policy in step with every capability the Kernel ever
/// adds — and it is the only source that can also tell a `Service` from a
/// `Tool` of the same name.
///
/// The capability is then invoked on that same serving router, with a **fresh**
/// invocation: a new [`RequestId`] (reusing the outer one would make two
/// requests indistinguishable in the trajectory), the plugin's own
/// [`PluginId`] — so the ordinary permission path decides this call exactly as
/// it would decide any other — and the host's own `DEFAULT_REQUEST_TIMEOUT`, so
/// a Kernel service that never answers fails the host call instead of hanging
/// the plugin's process.
///
/// The plugin identity matters: it is the namespace
/// `storage.*` authorizes against, and it is *not* a host identity, so the host
/// control plane (`crates/tact_ui/src/permission.rs`) refuses `chat.submit` /
/// `runs.*` for it.
///
/// The `router` argument the trait hands over is ignored on purpose. Kernel
/// services live on the serving router; an inner invoke on the per-wave tool
/// router would hit `PreflightPermissionGate`, whose one-use ticket belongs to
/// the outer tool call and cannot be spent by a nested one.
pub struct GrantedHostCalls {
    serving: RuntimeContext,
    plugin_id: PluginId,
    grants: BTreeSet<String>,
}

impl GrantedHostCalls {
    #[must_use]
    pub fn new(
        serving: RuntimeContext,
        plugin_id: PluginId,
        grants: impl IntoIterator<Item = String>,
    ) -> Self {
        Self {
            serving,
            plugin_id,
            grants: grants.into_iter().collect(),
        }
    }
}

#[async_trait]
impl HostCallService for GrantedHostCalls {
    async fn handle(
        &self,
        _context: &InvocationContext,
        _router: &CapabilityRouter,
        capability: &str,
        input: Value,
    ) -> Result<Value, KernelError> {
        if !self.grants.contains(capability) {
            return Err(KernelError::permission_denied(format!(
                "plugin {} was not granted the host call {capability}",
                self.plugin_id
            )));
        }
        let Some(declaration) = self.serving.router().describe(capability) else {
            return Err(KernelError::permission_denied(format!(
                "plugin {} cannot host-call {capability}: the serving router does not \
                 carry it",
                self.plugin_id
            )));
        };
        if declaration.kind != CapabilityKind::Service {
            return Err(KernelError::permission_denied(format!(
                "plugin {} cannot host-call {capability}: it is a {:?} capability, \
                 not a Kernel service",
                self.plugin_id, declaration.kind
            )));
        }
        let invocation = self
            .serving
            .invocation(
                // A fresh request id: the outer call's id belongs to the caller's
                // request, and reusing it would make the host call look like the
                // same request in the trajectory.
                RequestId::from(uuid::Uuid::new_v4().to_string()),
                self.plugin_id.clone(),
                // The plugin is the actor of its own call. Session-level hops that
                // carry a caller forward (`chat.submit` → `runs.start`, a Workflow
                // step → `chat.submit`) keep the caller's actor instead; there is no
                // caller to carry here — the plugin process is the origin.
                format!("plugin:{}", self.plugin_id),
            )
            // The inner call is bounded by the host's own default request timeout.
            // A plugin is an external participant, so a Kernel service that never
            // answers must fail this host call rather than hang the plugin's
            // process forever; and the caller's own bound cannot cover this hop —
            // `HostCapabilityHandler` bounds the *plugin's* call, which is still
            // waiting for an answer that will never come. `DEFAULT_REQUEST_TIMEOUT`
            // is the value this crate already puts on one host RPC, so a host call
            // is bounded exactly like any other exchange with the host.
            .with_timeout(DEFAULT_REQUEST_TIMEOUT);
        self.serving
            .router()
            .invoke(capability, invocation, input)
            .await
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

    fn request_timeout(&self) -> Duration {
        self.request_timeout
    }

    async fn request_with_context(
        &mut self,
        context: &InvocationContext,
        request: PluginRequest,
    ) -> Result<PluginResponse, KernelError> {
        StdioPluginHost::request_with_context(self, context, request).await
    }

    async fn interrupt(&mut self) {
        StdioPluginHost::interrupt(self).await;
    }

    async fn shutdown(&mut self) -> Result<()> {
        self.stop().await
    }
}

struct HostCapabilityHandler {
    host: Arc<Mutex<dyn PluginHost>>,
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
        let result = self.invoke_plugin(&mut *host, &context, input).await;
        completed.store(true, std::sync::atomic::Ordering::Release);
        cleanup.abort();
        result
    }
}

impl HostCapabilityHandler {
    async fn invoke_plugin(
        &self,
        host: &mut dyn PluginHost,
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
