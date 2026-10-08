//! Constrained WASM plugin host backed by the Wasmtime CLI.
//!
//! Guest capabilities use the same line-oriented Plugin Protocol transport as
//! Node hosts. Wasmtime receives only stdio: the host does not preopen guest
//! directories or inherit environment variables, and it disables WASI sockets.

mod host_functions;

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::Value;
use tact::{
    kernel::{CapabilityRouter, InvocationContext, KernelError, RuntimeContext},
    plugin::PluginState,
};
use tact_plugin_node::{HostCallService, NodePluginHost};
use tact_protocol::{
    CapabilityDeclaration, ErrorCategory, PluginId, PluginRequest, PluginResponse, ProtocolVersion,
    RequestId,
};
use tokio::sync::Mutex;

pub use host_functions::WasmHostFunctions;

/// Resource limits applied to each Wasmtime process.
#[derive(Debug, Clone)]
pub struct WasmHostConfig {
    pub memory_limit_bytes: u64,
    pub fuel: u64,
    pub timeout: Duration,
}

impl Default for WasmHostConfig {
    fn default() -> Self {
        Self {
            memory_limit_bytes: 64 * 1024 * 1024,
            fuel: 10_000_000,
            timeout: Duration::from_secs(30),
        }
    }
}

impl WasmHostConfig {
    fn validate(&self) -> Result<()> {
        if self.memory_limit_bytes == 0 || self.fuel == 0 || self.timeout.is_zero() {
            bail!("WASM memory, fuel and timeout limits must be positive");
        }
        Ok(())
    }

    fn runner_args(&self, module: &Path) -> Vec<String> {
        let timeout_ms = self.timeout.as_millis().max(1);
        vec![
            "run".into(),
            "--wasm".into(),
            format!(
                "fuel={},max-memory-size={},timeout={}ms",
                self.fuel, self.memory_limit_bytes, timeout_ms
            ),
            "--wasi".into(),
            "inherit-env=false,inherit-stdin=true,inherit-stdout=true,inherit-stderr=true,inherit-network=false,allow-ip-name-lookup=false,tcp=false,udp=false".into(),
            module.display().to_string(),
        ]
    }
}

/// Capability and service grants declared before a guest is instantiated.
#[derive(Debug, Clone)]
pub struct WasmPluginManifest {
    pub plugin_id: PluginId,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<CapabilityDeclaration>,
    pub host_capabilities: Vec<String>,
}

impl WasmPluginManifest {
    pub fn validate(&self) -> Result<()> {
        if self.plugin_id.as_str().contains(':') || self.plugin_id.as_str().contains('/') {
            bail!("WASM plugin ID contains a reserved namespace separator");
        }
        if !self.protocol.compatible_with(ProtocolVersion::CURRENT) {
            bail!("WASM plugin protocol version is not supported");
        }
        let mut capabilities = std::collections::BTreeSet::new();
        for declaration in &self.capabilities {
            declaration
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid WASM capability: {error}"))?;
            if !capabilities.insert(declaration.name.as_str()) {
                bail!(
                    "WASM manifest declares duplicate capability: {}",
                    declaration.name
                );
            }
        }
        let mut grants = std::collections::BTreeSet::new();
        for grant in &self.host_capabilities {
            if !host_functions::is_known_host_capability(grant) {
                bail!("WASM manifest requests unsupported host capability: {grant}");
            }
            if !grants.insert(grant.as_str()) {
                bail!("WASM manifest repeats host capability: {grant}");
            }
        }
        Ok(())
    }
}

/// A single Wasmtime instance connected to Tact's common capability router.
pub struct WasmPluginHost {
    inner: Arc<Mutex<NodePluginHost>>,
    plugin_id: PluginId,
    protocol: ProtocolVersion,
    capabilities: Vec<CapabilityDeclaration>,
    host_functions: WasmHostFunctions,
    state: AtomicU8,
}

impl WasmPluginHost {
    pub async fn instantiate(
        runner: impl AsRef<Path>,
        module: impl AsRef<Path>,
        manifest: WasmPluginManifest,
        config: WasmHostConfig,
    ) -> Result<Self> {
        manifest.validate()?;
        config.validate()?;
        let host_functions = WasmHostFunctions::new(
            manifest.plugin_id.clone(),
            manifest.host_capabilities.clone(),
        )?;
        if !module.as_ref().is_file() {
            bail!(
                "WASM plugin module does not exist: {}",
                module.as_ref().display()
            );
        }
        let args = config.runner_args(module.as_ref());
        let required_features = if manifest.host_capabilities.is_empty() {
            Vec::new()
        } else {
            vec!["host_calls".into()]
        };
        let host = NodePluginHost::start_with_feature_requirements(
            runner.as_ref().to_path_buf(),
            &args,
            manifest.plugin_id.clone(),
            manifest.protocol,
            config.timeout,
            vec![
                "capability_registration".into(),
                "events".into(),
                "cancel".into(),
                "host_calls".into(),
            ],
            required_features,
        )
        .await
        .context("instantiate WASM plugin through Wasmtime")?;
        if !same_declarations(host.capabilities(), &manifest.capabilities) {
            let mut host = host;
            let _ = host.stop().await;
            bail!("WASM registration does not match the validated manifest");
        }
        Ok(Self {
            inner: Arc::new(Mutex::new(host)),
            plugin_id: manifest.plugin_id,
            protocol: manifest.protocol,
            capabilities: manifest.capabilities,
            host_functions,
            state: AtomicU8::new(encode_state(PluginState::Running)),
        })
    }

    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    pub fn protocol(&self) -> ProtocolVersion {
        self.protocol
    }

    pub fn capabilities(&self) -> &[CapabilityDeclaration] {
        &self.capabilities
    }

    pub fn state(&self) -> PluginState {
        decode_state(self.state.load(Ordering::Acquire))
    }

    pub fn host_functions(&self) -> &WasmHostFunctions {
        &self.host_functions
    }

    pub fn register_with_router(&self, router: &CapabilityRouter) -> Result<(), KernelError> {
        NodePluginHost::register_with_router_and_host_calls(
            Arc::clone(&self.inner),
            router,
            Arc::new(WasmHostCallService {
                functions: self.host_functions.clone(),
            }),
        )
    }

    pub async fn invoke(
        &self,
        runtime: &RuntimeContext,
        capability: &str,
        context: InvocationContext,
        input: Value,
    ) -> Result<Value, KernelError> {
        let result = runtime.router().invoke(capability, context, input).await;
        if result.as_ref().is_err_and(|error| {
            matches!(
                error.category(),
                ErrorCategory::PluginCrashed | ErrorCategory::Timeout | ErrorCategory::Cancelled
            )
        }) {
            self.state
                .store(encode_state(PluginState::Failed), Ordering::Release);
        }
        result
    }

    pub fn interrupt(
        &self,
        cancellation: &tact::kernel::CancellationService,
        request_id: &RequestId,
    ) -> bool {
        let cancelled = cancellation.cancel_request(request_id);
        if cancelled {
            self.state
                .store(encode_state(PluginState::Stopping), Ordering::Release);
        }
        cancelled
    }

    pub async fn request(&self, request: PluginRequest) -> Result<PluginResponse, KernelError> {
        match self.inner.lock().await.request(request).await {
            Ok(response) => Ok(response),
            Err(error) => {
                if matches!(
                    error.category(),
                    ErrorCategory::PluginCrashed
                        | ErrorCategory::Timeout
                        | ErrorCategory::Cancelled
                ) {
                    self.state
                        .store(encode_state(PluginState::Failed), Ordering::Release);
                }
                Err(error)
            }
        }
    }

    /// Gracefully stop the process and release its guest instance.
    pub async fn drop_instance(&self) -> Result<()> {
        self.state
            .store(encode_state(PluginState::Stopping), Ordering::Release);
        let result = self.inner.lock().await.stop().await;
        self.state.store(
            encode_state(if result.is_ok() {
                PluginState::Stopped
            } else {
                PluginState::Failed
            }),
            Ordering::Release,
        );
        result
    }
}

struct WasmHostCallService {
    functions: WasmHostFunctions,
}

#[async_trait]
impl HostCallService for WasmHostCallService {
    async fn handle(
        &self,
        context: &InvocationContext,
        router: &CapabilityRouter,
        capability: &str,
        input: Value,
    ) -> Result<Value, KernelError> {
        self.functions
            .handle_call(context, router, capability, input)
            .await
    }
}

fn same_declarations(left: &[CapabilityDeclaration], right: &[CapabilityDeclaration]) -> bool {
    fn normalized(items: &[CapabilityDeclaration]) -> Option<Vec<Value>> {
        let mut items = items.iter().collect::<Vec<_>>();
        items.sort_by(|left, right| left.name.cmp(&right.name));
        items
            .into_iter()
            .map(serde_json::to_value)
            .collect::<Result<Vec<_>, _>>()
            .ok()
    }
    normalized(left) == normalized(right)
}

#[async_trait::async_trait]
impl tact::plugin::PluginHost for WasmPluginHost {
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
        WasmPluginHost::state(self)
    }

    async fn request(&mut self, request: PluginRequest) -> anyhow::Result<PluginResponse> {
        WasmPluginHost::request(self, request)
            .await
            .map_err(anyhow::Error::from)
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.drop_instance().await
    }
}

fn encode_state(state: PluginState) -> u8 {
    match state {
        PluginState::Registered => 0,
        PluginState::Running => 1,
        PluginState::Stopping => 2,
        PluginState::Stopped => 3,
        PluginState::Failed => 4,
    }
}

fn decode_state(state: u8) -> PluginState {
    match state {
        1 => PluginState::Running,
        2 => PluginState::Stopping,
        3 => PluginState::Stopped,
        4 => PluginState::Failed,
        _ => PluginState::Registered,
    }
}
