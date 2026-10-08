use crate::{
    CapabilityDeclaration, InteractionRequest, InteractionResponse, PluginId, ProtocolError,
    ProtocolVersion, RequestId, RunId, StepId, TrajectoryId,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginRequest {
    Handshake {
        protocol_version: ProtocolVersion,
        features: Vec<String>,
    },
    Register {
        capabilities: Vec<CapabilityDeclaration>,
    },
    Invoke {
        capability: String,
        input: serde_json::Value,
    },
    Subscribe {
        from_sequence: Option<u64>,
    },
    Cancel {
        request_id: RequestId,
    },
    InteractionResponse {
        response: InteractionResponse,
    },
    HostCallResult {
        host_request_id: RequestId,
        output: Option<serde_json::Value>,
        error: Option<ProtocolError>,
    },
    Shutdown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PluginResponse {
    HandshakeAccepted {
        protocol_version: ProtocolVersion,
        features: Vec<String>,
    },
    Registered {
        capabilities: Vec<CapabilityDeclaration>,
    },
    Result {
        output: serde_json::Value,
    },
    Event {
        event: RuntimeEvent,
    },
    HostCall {
        host_request_id: RequestId,
        capability: String,
        input: serde_json::Value,
    },
    Error {
        error: ProtocolError,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeEvent {
    RunStarted {
        run_id: RunId,
    },
    RunFinished {
        run_id: RunId,
        success: bool,
    },
    ModelCallStarted {
        run_id: RunId,
        step_id: StepId,
        model: String,
    },
    ModelCallFinished {
        run_id: RunId,
        step_id: StepId,
    },
    ToolCallStarted {
        run_id: RunId,
        step_id: StepId,
        tool: String,
    },
    ToolCallFinished {
        run_id: RunId,
        step_id: StepId,
        success: bool,
    },
    PermissionRequested {
        request_id: RequestId,
        capability: String,
    },
    PermissionResolved {
        request_id: RequestId,
        allowed: bool,
    },
    PluginStarted {
        plugin_id: String,
    },
    PluginStopped {
        plugin_id: String,
    },
    InteractionRequested {
        request: InteractionRequest,
    },
    InteractionResponded {
        response: InteractionResponse,
    },
    Cancelled {
        run_id: RunId,
    },
    TimedOut {
        run_id: RunId,
    },
    Text {
        run_id: Option<RunId>,
        role: String,
        content: String,
    },
    Notification {
        level: String,
        content: String,
    },
    Error {
        run_id: Option<RunId>,
        message: String,
    },
    Plugin {
        plugin_id: PluginId,
        origin: String,
        event_type: String,
        payload: serde_json::Value,
    },
}

impl RuntimeEvent {
    pub fn validate_plugin_event(&self, plugin_id: &str) -> Result<(), String> {
        let RuntimeEvent::Plugin {
            plugin_id: event_plugin_id,
            origin,
            event_type,
            ..
        } = self
        else {
            return Err("plugins may only emit plugin events".into());
        };
        let declared_plugin_id = PluginId::new(plugin_id).map_err(|error| error.to_string())?;
        if declared_plugin_id.as_str() != event_plugin_id.as_str() {
            return Err("plugin event origin does not match plugin ID".into());
        }
        if origin != "plugin" {
            return Err("plugin event origin must be plugin".into());
        }
        if plugin_id.contains(':') || plugin_id.contains('/') {
            return Err("plugin ID cannot contain namespace separators".into());
        }
        let prefix = format!("plugin.{plugin_id}.");
        if !event_type.starts_with(&prefix) {
            return Err(format!("plugin event must use namespace {prefix}"));
        }
        let reserved = [
            "permission.",
            "tool.",
            "run.",
            "model.",
            "plugin.lifecycle.",
        ];
        let suffix = &event_type[prefix.len()..];
        if suffix.is_empty()
            || suffix.contains(char::is_whitespace)
            || reserved.iter().any(|prefix| suffix.starts_with(prefix))
        {
            return Err("event type is reserved for the host".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RuntimeCommand {
    StartRun {
        run_id: RunId,
        input: serde_json::Value,
    },
    CancelRun {
        run_id: RunId,
    },
    RespondInteraction {
        response: InteractionResponse,
    },
    Subscribe {
        from_sequence: Option<u64>,
    },
    Resume {
        trajectory_id: TrajectoryId,
        from_sequence: u64,
    },
    Invoke {
        capability: String,
        input: serde_json::Value,
    },
    Shutdown,
}
