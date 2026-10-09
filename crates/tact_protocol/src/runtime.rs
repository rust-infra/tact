use crate::{
    CapabilityDeclaration, InteractionRequest, InteractionResponse, ModelCallParams, PlanStep,
    PluginId, ProtocolError, ProtocolVersion, RequestId, RunId, StepId, StepResult,
    SubagentRunSnapshot, TaskSnapshot, TasksChangeReason, ThinkingChunk, TokenUsageInfo,
    ToolOutputChunk, ToolPresentationInfo, TrajectoryId,
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
    /// History was compacted. `trigger` names what asked for it
    /// (`user` / `auto` / `recovery`); `focus` is the optional user focus.
    Compaction {
        run_id: Option<RunId>,
        trigger: String,
        focus: Option<String>,
    },
    /// A failed attempt was recovered (compaction on overflow, continuation on
    /// truncation). `attempt` is 1-based.
    Recovery {
        run_id: Option<RunId>,
        attempt: u32,
        reason: String,
    },
    /// A transient failure was retried after a backoff. `attempt` is 1-based.
    Retry {
        run_id: Option<RunId>,
        attempt: u32,
        reason: String,
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
    Thinking {
        run_id: Option<RunId>,
        chunk: ThinkingChunk,
    },
    ToolProgress {
        run_id: Option<RunId>,
        tool_id: String,
        chunks: Vec<ToolOutputChunk>,
    },
    ModelInfo {
        run_id: Option<RunId>,
        params: ModelCallParams,
    },
    TokenUsage {
        run_id: Option<RunId>,
        usage: TokenUsageInfo,
    },
    TurnStats {
        run_id: Option<RunId>,
        turns_taken: u32,
        max_turns: Option<u32>,
    },
    Error {
        run_id: Option<RunId>,
        message: String,
    },
    // ── Structured View events ───────────────────────────────────────────
    // A View consumes these directly; they replace the opaque
    // `ViewUpdate { update: AgentUpdate }` pass-through so the Runtime stream
    // carries the view data as first-class, serializable protocol values.
    StepAdded {
        run_id: Option<RunId>,
        step: PlanStep,
    },
    StepStarted {
        run_id: Option<RunId>,
        idx: usize,
        tool_id: String,
        tool_name: String,
        arg_summary: String,
        arg_full: String,
        presentation: ToolPresentationInfo,
    },
    StepFinished {
        run_id: Option<RunId>,
        idx: usize,
        tool_id: String,
        result: StepResult,
    },
    StepFailed {
        run_id: Option<RunId>,
        idx: usize,
        tool_id: String,
        arg_summary: String,
        error: String,
    },
    TaskComplete {
        run_id: Option<RunId>,
        content: String,
    },
    Info {
        run_id: Option<RunId>,
        content: String,
    },
    MdInfo {
        run_id: Option<RunId>,
        content: String,
    },
    HookContext {
        run_id: Option<RunId>,
        source: Option<String>,
        text: String,
    },
    HookStatus {
        run_id: Option<RunId>,
        id: u64,
        source: Option<String>,
        message: String,
        elapsed_ms: Option<u64>,
    },
    PopupMarkdown {
        run_id: Option<RunId>,
        title: String,
        source: String,
    },
    TasksChanged {
        run_id: Option<RunId>,
        tasks: Vec<TaskSnapshot>,
        reason: TasksChangeReason,
    },
    ToolMeta {
        run_id: Option<RunId>,
        tool_id: String,
        model: Option<String>,
        token_usage: Option<TokenUsageInfo>,
        task_id: Option<String>,
    },
    BackgroundTaskFinished {
        run_id: Option<RunId>,
        tool_id: String,
        success: bool,
        message: String,
        output: String,
    },
    SubagentFinished {
        run_id: Option<RunId>,
        tool_id: String,
        child_id: String,
        success: bool,
        summary: String,
    },
    SubagentsChanged {
        run_id: Option<RunId>,
        runs: Vec<SubagentRunSnapshot>,
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
