//! Test doubles for the hosted-plugin lifecycle, shared by this crate's tests.
//!
//! The `plugin::runtime` lifecycle tests and the Agent's tool-dispatch tests both
//! need a host that declares capabilities, and both drive registration through
//! the *production* registration path. One double is shared rather than two
//! look-alikes that could drift apart — the whole point of the seam is that a
//! test double registers exactly as a spawned stdio host would.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use async_trait::async_trait;
use serde_json::{Value, json};
use tact::{PluginState, RuntimePluginManifest};
use tact_plugin_host::PluginHost;
use tact_protocol::{
    CapabilityDeclaration, CapabilityKind, CapabilityRisk, PluginId, PluginRequest, PluginResponse,
    ProtocolError, ProtocolVersion, RequestId,
};

use super::{HostedPlugin, HostedPlugins};

/// The `HostCallResult` the host sent back, as the plugin received it.
///
/// Recorded by the double rather than answered blindly: a host-call test asserts
/// on *this* (what the plugin was told) as well as on the plugin's own output.
pub(crate) struct RecordedHostCall {
    pub(crate) output: Option<Value>,
    pub(crate) error: Option<ProtocolError>,
}

/// A `PluginHost` double.
pub(crate) struct DoubleHost {
    id: PluginId,
    capabilities: Vec<CapabilityDeclaration>,
    state: PluginState,
    stopped: Arc<AtomicBool>,
    /// The one host call this double makes while answering an `Invoke`, taken on
    /// first use so the *next* request after the host's `HostCallResult` gets a
    /// plain result.
    host_call: Option<(String, Value)>,
    host_call_result: Arc<Mutex<Option<RecordedHostCall>>>,
}

impl DoubleHost {
    pub(crate) fn new(id: &str, capabilities: Vec<CapabilityDeclaration>) -> Self {
        Self {
            id: PluginId::from(id),
            capabilities,
            state: PluginState::Running,
            stopped: Arc::new(AtomicBool::new(false)),
            host_call: None,
            host_call_result: Arc::new(Mutex::new(None)),
        }
    }

    /// Makes the double ask the host for one capability while it answers its
    /// next `Invoke`.
    fn with_host_call(mut self, capability: String, input: Value) -> Self {
        self.host_call = Some((capability, input));
        self
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
            PluginRequest::Invoke { capability, input } => match self.host_call.take() {
                Some((host_capability, host_input)) => Ok(PluginResponse::HostCall {
                    host_request_id: RequestId::from("double-host-call"),
                    capability: host_capability,
                    input: host_input,
                }),
                None => Ok(PluginResponse::Result {
                    // The envelope is a `ToolCallResult` plus the raw echo: the
                    // one shape serves both the serving-router test
                    // (`capability` / `echo`) and the Agent's tool path
                    // (`content`), and an unknown field is ignored by the
                    // decoder rather than fatal.
                    output: json!({
                        "content": format!("plugin:{capability}"),
                        "effects": [],
                        "image": Value::Null,
                        "capability": capability,
                        "echo": input,
                    }),
                }),
            },
            // What the host answered the plugin's host call with. The plugin's
            // reply to *that* is its own result: the real plugin would use the
            // output; the double hands it straight back so the test can assert
            // the output travelled the whole way.
            PluginRequest::HostCallResult { output, error, .. } => {
                let reply = json!({
                    "content": "plugin:host-call",
                    "host_call": { "output": output, "error": error },
                });
                *self.host_call_result.lock().unwrap() = Some(RecordedHostCall { output, error });
                Ok(PluginResponse::Result { output: reply })
            }
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

/// Wraps a double in the manifest and the `PluginHost` boundary registration
/// expects; `grants` is the manifest's host-call grant list.
fn as_hosted(double: DoubleHost, grants: Vec<String>) -> HostedPlugin {
    let manifest = RuntimePluginManifest {
        id: double.id.clone(),
        version: "1".into(),
        protocol: ProtocolVersion::CURRENT,
        capabilities: double.capabilities.clone(),
        dependencies: Vec::new(),
    };
    let host: Arc<tokio::sync::Mutex<dyn PluginHost>> = Arc::new(tokio::sync::Mutex::new(double));
    HostedPlugin::new(host, manifest).with_grants(grants)
}

/// Builds a hosted double plus the flag its stop records.
pub(crate) fn hosted_double(
    id: &str,
    capabilities: Vec<CapabilityDeclaration>,
) -> (HostedPlugin, Arc<AtomicBool>) {
    let double = DoubleHost::new(id, capabilities);
    let stopped = Arc::clone(&double.stopped);
    (as_hosted(double, Vec::new()), stopped)
}

/// Builds a hosted double that asks the host for `capability` while it answers
/// its next `Invoke`, with `grants` as its manifest's host-call grants.
///
/// Returns the record of the `HostCallResult` the host sent back, which the
/// test reads after the invocation has returned.
pub(crate) fn hosted_double_with_host_call(
    id: &str,
    capabilities: Vec<CapabilityDeclaration>,
    grants: &[&str],
    capability: &str,
    input: Value,
) -> (HostedPlugin, Arc<Mutex<Option<RecordedHostCall>>>) {
    let double = DoubleHost::new(id, capabilities).with_host_call(capability.to_string(), input);
    let recorded = Arc::clone(&double.host_call_result);
    let grants = grants.iter().map(|grant| (*grant).to_string()).collect();
    (as_hosted(double, grants), recorded)
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
