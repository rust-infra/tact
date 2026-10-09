use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{AgentUpdate, RunId, RuntimeEvent, StepId, TrajectoryId};

use tact::redact::{RedactionConfig, RedactionLevel};

use crate::model::{
    Sensitivity, SqliteTrajectoryRecorder, TrajectoryEventType, TrajectoryRecorder,
};

use tact::{KernelError, TrajectoryService};

#[derive(Clone, Default)]
pub struct KernelTrajectoryRecorder {
    recorder: TrajectoryRecorder,
}

#[derive(Clone)]
pub struct SqliteTrajectoryService {
    recorder: SqliteTrajectoryRecorder,
    redaction: RedactionConfig,
}

impl SqliteTrajectoryService {
    /// Builds the durable recorder with the default security policy.
    ///
    /// The default [`RedactionConfig`] resolves to [`RedactionLevel::Basic`],
    /// so payloads are redacted out of the box rather than only when a host
    /// remembers to install a policy.
    #[must_use]
    pub fn new(recorder: SqliteTrajectoryRecorder) -> Self {
        Self {
            recorder,
            redaction: RedactionConfig::default(),
        }
    }

    /// Builds the durable recorder with the session's effective policy.
    #[must_use]
    pub fn with_redaction(recorder: SqliteTrajectoryRecorder, redaction: RedactionConfig) -> Self {
        Self {
            recorder,
            redaction,
        }
    }
}

/// Redacts a serialized payload with the existing security policy.
///
/// Trajectory stores execution facts — streamed text, tool output, hook
/// context, notifications — so it is a persistence sink exactly like the
/// transcript and the session store, and a secret a tool printed must not land
/// in `trajectory_events.payload` verbatim. The payload is redacted in its
/// serialized form (the same text that would be written), then parsed back so
/// the stored value stays structured. A redaction replacement cannot break the
/// JSON shape (`[redacted:…]` contains no quotes or escapes), but if anything
/// else ever did, the redacted text is stored as a JSON string rather than the
/// secret being written — the safe direction to fail.
fn redact_payload(payload: &Value, config: &RedactionConfig) -> Value {
    let level = config.resolved_level();
    if !config.is_enabled() || level == RedactionLevel::Off {
        return payload.clone();
    }
    let Ok(text) = serde_json::to_string(payload) else {
        return payload.clone();
    };
    let redacted = tact::redact::redact(&text, level, &config.extra_patterns).into_owned();
    if redacted == text {
        return payload.clone();
    }
    serde_json::from_str(&redacted).unwrap_or(Value::String(redacted))
}

impl KernelTrajectoryRecorder {
    #[must_use]
    pub fn new(recorder: TrajectoryRecorder) -> Self {
        Self { recorder }
    }

    #[must_use]
    pub fn recorder(&self) -> &TrajectoryRecorder {
        &self.recorder
    }
}

#[async_trait]
impl TrajectoryService for KernelTrajectoryRecorder {
    async fn append(
        &self,
        supplied_trajectory_id: Option<&TrajectoryId>,
        supplied_run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        let run_id = match supplied_run_id.cloned().or_else(|| match &event {
            RuntimeEvent::RunStarted { run_id }
            | RuntimeEvent::RunFinished { run_id, .. }
            | RuntimeEvent::Cancelled { run_id }
            | RuntimeEvent::TimedOut { run_id }
            | RuntimeEvent::ModelCallStarted { run_id, .. }
            | RuntimeEvent::ModelCallFinished { run_id, .. }
            | RuntimeEvent::ToolCallStarted { run_id, .. }
            | RuntimeEvent::ToolCallFinished { run_id, .. } => Some(run_id.clone()),
            RuntimeEvent::Text { run_id, .. }
            | RuntimeEvent::Thinking { run_id, .. }
            | RuntimeEvent::ModelInfo { run_id, .. }
            | RuntimeEvent::TokenUsage { run_id, .. }
            | RuntimeEvent::TurnStats { run_id, .. }
            | RuntimeEvent::ToolProgress { run_id, .. }
            | RuntimeEvent::ViewUpdate { run_id, .. } => run_id.clone(),
            _ => None,
        }) {
            Some(run_id) => run_id,
            None => RunId::new("runtime").map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::InternalError,
                    error.to_string(),
                    "trajectory",
                    false,
                )
            })?,
        };
        let trajectory_id = supplied_trajectory_id.cloned().unwrap_or_else(|| {
            TrajectoryId::new(run_id.as_str()).expect("run ID is validated by protocol")
        });
        let event_type = match &event {
            RuntimeEvent::RunStarted { .. } | RuntimeEvent::RunFinished { .. } => {
                TrajectoryEventType::RunLifecycle
            }
            RuntimeEvent::Cancelled { .. } => TrajectoryEventType::Cancellation,
            RuntimeEvent::TimedOut { .. } => TrajectoryEventType::Timeout,
            RuntimeEvent::ModelCallStarted { .. }
            | RuntimeEvent::ModelCallFinished { .. }
            | RuntimeEvent::Thinking { .. }
            | RuntimeEvent::ModelInfo { .. }
            | RuntimeEvent::TokenUsage { .. }
            | RuntimeEvent::TurnStats { .. } => TrajectoryEventType::ModelCall,
            RuntimeEvent::ToolCallStarted { .. }
            | RuntimeEvent::ToolCallFinished { .. }
            | RuntimeEvent::ToolProgress { .. } => TrajectoryEventType::ToolCall,
            RuntimeEvent::PermissionRequested { .. } | RuntimeEvent::PermissionResolved { .. } => {
                TrajectoryEventType::Permission
            }
            RuntimeEvent::InteractionRequested { .. }
            | RuntimeEvent::InteractionResponded { .. } => TrajectoryEventType::Interaction,
            RuntimeEvent::Text { .. } | RuntimeEvent::Notification { .. } => {
                TrajectoryEventType::Message
            }
            RuntimeEvent::ViewUpdate { update, .. } => match update {
                AgentUpdate::StepAdded(_)
                | AgentUpdate::StepStarted { .. }
                | AgentUpdate::StepFinished { .. }
                | AgentUpdate::StepFailed { .. }
                | AgentUpdate::ToolProgress { .. }
                | AgentUpdate::ToolMeta { .. }
                | AgentUpdate::BackgroundTaskFinished { .. }
                | AgentUpdate::SubagentFinished { .. } => TrajectoryEventType::ToolCall,
                AgentUpdate::RequestSelect { .. } | AgentUpdate::RequestMultiSelect { .. } => {
                    TrajectoryEventType::Interaction
                }
                AgentUpdate::TaskCancelled => TrajectoryEventType::Cancellation,
                AgentUpdate::Error(_) => TrajectoryEventType::Error,
                _ => TrajectoryEventType::Message,
            },
            RuntimeEvent::Error { .. } => TrajectoryEventType::Error,
            RuntimeEvent::PluginStarted { .. }
            | RuntimeEvent::PluginStopped { .. }
            | RuntimeEvent::Plugin { .. } => TrajectoryEventType::PluginLifecycle,
        };
        let payload = serde_json::to_value(&event).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                error.to_string(),
                "trajectory",
                false,
            )
        })?;
        self.recorder
            .append(
                trajectory_id,
                run_id,
                "runtime".to_string(),
                event_type,
                None::<StepId>,
                payload,
                Sensitivity::Internal,
            )
            .map(|_| ())
            .map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::StorageError,
                    error,
                    "trajectory",
                    true,
                )
            })
    }
}

#[async_trait]
impl TrajectoryService for SqliteTrajectoryService {
    async fn append(
        &self,
        trajectory_id: Option<&tact_protocol::TrajectoryId>,
        run_id: Option<&tact_protocol::RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        let run_id = run_id
            .cloned()
            .or_else(|| match &event {
                RuntimeEvent::RunStarted { run_id }
                | RuntimeEvent::RunFinished { run_id, .. }
                | RuntimeEvent::Cancelled { run_id }
                | RuntimeEvent::TimedOut { run_id }
                | RuntimeEvent::ModelCallStarted { run_id, .. }
                | RuntimeEvent::ModelCallFinished { run_id, .. }
                | RuntimeEvent::ToolCallStarted { run_id, .. }
                | RuntimeEvent::ToolCallFinished { run_id, .. } => Some(run_id.clone()),
                RuntimeEvent::Text { run_id, .. }
                | RuntimeEvent::Thinking { run_id, .. }
                | RuntimeEvent::ModelInfo { run_id, .. }
                | RuntimeEvent::TokenUsage { run_id, .. }
                | RuntimeEvent::TurnStats { run_id, .. }
                | RuntimeEvent::ToolProgress { run_id, .. }
                | RuntimeEvent::ViewUpdate { run_id, .. } => run_id.clone(),
                _ => None,
            })
            // Notifications and plugin lifecycle events can be emitted before
            // a run exists. Keep them replayable in the runtime stream rather
            // than dropping them or making startup depend on a synthetic run.
            .unwrap_or_else(|| tact_protocol::RunId::from("runtime"));
        let trajectory_id = trajectory_id
            .cloned()
            .unwrap_or_else(|| tact_protocol::TrajectoryId::from(run_id.as_str()));
        let event_type = event_type(&event);
        let payload = serde_json::to_value(&event).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                error.to_string(),
                "trajectory",
                false,
            )
        })?;
        let payload = redact_payload(&payload, &self.redaction);
        self.recorder
            .append(
                trajectory_id,
                run_id,
                "runtime".into(),
                event_type,
                None,
                payload,
                Sensitivity::Internal,
            )
            .await
            .map(|_| ())
            .map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::StorageError,
                    error.to_string(),
                    "trajectory",
                    true,
                )
            })
    }
}

fn event_type(event: &RuntimeEvent) -> TrajectoryEventType {
    match event {
        RuntimeEvent::RunStarted { .. } | RuntimeEvent::RunFinished { .. } => {
            TrajectoryEventType::RunLifecycle
        }
        RuntimeEvent::Cancelled { .. } => TrajectoryEventType::Cancellation,
        RuntimeEvent::TimedOut { .. } => TrajectoryEventType::Timeout,
        RuntimeEvent::ModelCallStarted { .. }
        | RuntimeEvent::ModelCallFinished { .. }
        | RuntimeEvent::Thinking { .. }
        | RuntimeEvent::ModelInfo { .. }
        | RuntimeEvent::TokenUsage { .. }
        | RuntimeEvent::TurnStats { .. } => TrajectoryEventType::ModelCall,
        RuntimeEvent::ToolCallStarted { .. }
        | RuntimeEvent::ToolCallFinished { .. }
        | RuntimeEvent::ToolProgress { .. } => TrajectoryEventType::ToolCall,
        RuntimeEvent::PermissionRequested { .. } | RuntimeEvent::PermissionResolved { .. } => {
            TrajectoryEventType::Permission
        }
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionResponded { .. } => {
            TrajectoryEventType::Interaction
        }
        RuntimeEvent::Text { .. } | RuntimeEvent::Notification { .. } => {
            TrajectoryEventType::Message
        }
        RuntimeEvent::ViewUpdate { update, .. } => match update {
            AgentUpdate::StepAdded(_)
            | AgentUpdate::StepStarted { .. }
            | AgentUpdate::StepFinished { .. }
            | AgentUpdate::StepFailed { .. }
            | AgentUpdate::ToolProgress { .. }
            | AgentUpdate::ToolMeta { .. }
            | AgentUpdate::BackgroundTaskFinished { .. }
            | AgentUpdate::SubagentFinished { .. } => TrajectoryEventType::ToolCall,
            AgentUpdate::RequestSelect { .. } | AgentUpdate::RequestMultiSelect { .. } => {
                TrajectoryEventType::Interaction
            }
            AgentUpdate::TaskCancelled => TrajectoryEventType::Cancellation,
            AgentUpdate::Error(_) => TrajectoryEventType::Error,
            _ => TrajectoryEventType::Message,
        },
        RuntimeEvent::Error { .. } => TrajectoryEventType::Error,
        RuntimeEvent::PluginStarted { .. }
        | RuntimeEvent::PluginStopped { .. }
        | RuntimeEvent::Plugin { .. } => TrajectoryEventType::PluginLifecycle,
    }
}
