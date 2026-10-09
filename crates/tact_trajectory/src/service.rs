use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{RunId, RuntimeEvent, StepId, TrajectoryId};

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

    async fn query_facts(
        &self,
        trajectory_id: &TrajectoryId,
        from_sequence: u64,
    ) -> Result<Vec<tact_protocol::TrajectoryEvent>, KernelError> {
        self.recorder
            .query(trajectory_id, from_sequence)
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
impl TrajectoryService for KernelTrajectoryRecorder {
    async fn query_by_run(
        &self,
        run_id: &tact_protocol::RunId,
    ) -> Result<Vec<tact_protocol::TrajectoryEvent>, KernelError> {
        self.recorder.query_by_run(run_id).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::StorageError,
                error,
                "trajectory",
                true,
            )
        })
    }

    async fn query(
        &self,
        trajectory_id: &tact_protocol::TrajectoryId,
        from_sequence: u64,
    ) -> Result<Vec<tact_protocol::TrajectoryEvent>, KernelError> {
        self.query_facts(trajectory_id, from_sequence).await
    }

    async fn append(
        &self,
        supplied_trajectory_id: Option<&TrajectoryId>,
        supplied_run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        let run_id = match supplied_run_id.cloned().or_else(|| match &event {
            RuntimeEvent::RunStarted { run_id } => Some(run_id.clone()),
            RuntimeEvent::RunFinished { run_id, .. } | RuntimeEvent::Cancelled { run_id } => {
                run_id.clone()
            }
            RuntimeEvent::TimedOut { run_id }
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
            | RuntimeEvent::StepAdded { run_id, .. }
            | RuntimeEvent::StepStarted { run_id, .. }
            | RuntimeEvent::StepFinished { run_id, .. }
            | RuntimeEvent::StepFailed { run_id, .. }
            | RuntimeEvent::TaskComplete { run_id, .. }
            | RuntimeEvent::Info { run_id, .. }
            | RuntimeEvent::MdInfo { run_id, .. }
            | RuntimeEvent::HookContext { run_id, .. }
            | RuntimeEvent::HookStatus { run_id, .. }
            | RuntimeEvent::PopupMarkdown { run_id, .. }
            | RuntimeEvent::TasksChanged { run_id, .. }
            | RuntimeEvent::ToolMeta { run_id, .. }
            | RuntimeEvent::BackgroundTaskFinished { run_id, .. }
            | RuntimeEvent::SubagentFinished { run_id, .. }
            | RuntimeEvent::SubagentsChanged { run_id, .. }
            | RuntimeEvent::Compaction { run_id, .. }
            | RuntimeEvent::Recovery { run_id, .. }
            | RuntimeEvent::Retry { run_id, .. } => run_id.clone(),
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
            RuntimeEvent::Compaction { .. } => TrajectoryEventType::Compaction,
            RuntimeEvent::Recovery { .. } => TrajectoryEventType::Recovery,
            RuntimeEvent::Retry { .. } => TrajectoryEventType::Retry,
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
            RuntimeEvent::Text { role, .. } if role == "user" => TrajectoryEventType::UserInput,
            RuntimeEvent::Text { .. } | RuntimeEvent::Notification { .. } => {
                TrajectoryEventType::Message
            }
            RuntimeEvent::StepAdded { .. }
            | RuntimeEvent::StepStarted { .. }
            | RuntimeEvent::StepFinished { .. }
            | RuntimeEvent::ToolMeta { .. }
            | RuntimeEvent::BackgroundTaskFinished { .. }
            | RuntimeEvent::SubagentFinished { .. }
            | RuntimeEvent::SubagentsChanged { .. } => TrajectoryEventType::ToolCall,
            RuntimeEvent::TaskComplete { .. }
            | RuntimeEvent::Info { .. }
            | RuntimeEvent::MdInfo { .. }
            | RuntimeEvent::HookContext { .. }
            | RuntimeEvent::HookStatus { .. }
            | RuntimeEvent::PopupMarkdown { .. }
            | RuntimeEvent::TasksChanged { .. } => TrajectoryEventType::Message,
            RuntimeEvent::StepFailed { .. } | RuntimeEvent::Error { .. } => {
                TrajectoryEventType::Error
            }
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
        let parent_step_id = parent_step_of(&event);
        self.recorder
            .append(
                trajectory_id,
                run_id,
                "runtime".to_string(),
                event_type,
                parent_step_id,
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
    async fn query_by_run(
        &self,
        run_id: &tact_protocol::RunId,
    ) -> Result<Vec<tact_protocol::TrajectoryEvent>, KernelError> {
        self.recorder.query_by_run(run_id).await.map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::StorageError,
                error.to_string(),
                "trajectory",
                true,
            )
        })
    }

    async fn query(
        &self,
        trajectory_id: &tact_protocol::TrajectoryId,
        from_sequence: u64,
    ) -> Result<Vec<tact_protocol::TrajectoryEvent>, KernelError> {
        self.recorder
            .query(trajectory_id, from_sequence)
            .await
            .map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::StorageError,
                    error.to_string(),
                    "trajectory",
                    true,
                )
            })
    }

    async fn append(
        &self,
        trajectory_id: Option<&tact_protocol::TrajectoryId>,
        run_id: Option<&tact_protocol::RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        let run_id = run_id
            .cloned()
            .or_else(|| match &event {
                RuntimeEvent::RunStarted { run_id } => Some(run_id.clone()),
                RuntimeEvent::RunFinished { run_id, .. } | RuntimeEvent::Cancelled { run_id } => {
                    run_id.clone()
                }
                RuntimeEvent::TimedOut { run_id }
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
                | RuntimeEvent::StepAdded { run_id, .. }
                | RuntimeEvent::StepStarted { run_id, .. }
                | RuntimeEvent::StepFinished { run_id, .. }
                | RuntimeEvent::StepFailed { run_id, .. }
                | RuntimeEvent::TaskComplete { run_id, .. }
                | RuntimeEvent::Info { run_id, .. }
                | RuntimeEvent::MdInfo { run_id, .. }
                | RuntimeEvent::HookContext { run_id, .. }
                | RuntimeEvent::HookStatus { run_id, .. }
                | RuntimeEvent::PopupMarkdown { run_id, .. }
                | RuntimeEvent::TasksChanged { run_id, .. }
                | RuntimeEvent::ToolMeta { run_id, .. }
                | RuntimeEvent::BackgroundTaskFinished { run_id, .. }
                | RuntimeEvent::SubagentFinished { run_id, .. }
                | RuntimeEvent::SubagentsChanged { run_id, .. }
                | RuntimeEvent::Compaction { run_id, .. }
                | RuntimeEvent::Recovery { run_id, .. }
                | RuntimeEvent::Retry { run_id, .. } => run_id.clone(),
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
        let parent_step_id = parent_step_of(&event);
        self.recorder
            .append(
                trajectory_id,
                run_id,
                "runtime".into(),
                event_type,
                parent_step_id,
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

/// The step a fact is nested under, when the event carries one.
fn parent_step_of(event: &RuntimeEvent) -> Option<StepId> {
    match event {
        RuntimeEvent::ToolCallStarted { parent_step_id, .. }
        | RuntimeEvent::ToolCallFinished { parent_step_id, .. } => parent_step_id.clone(),
        _ => None,
    }
}

fn event_type(event: &RuntimeEvent) -> TrajectoryEventType {
    match event {
        RuntimeEvent::RunStarted { .. } | RuntimeEvent::RunFinished { .. } => {
            TrajectoryEventType::RunLifecycle
        }
        RuntimeEvent::Cancelled { .. } => TrajectoryEventType::Cancellation,
        RuntimeEvent::Compaction { .. } => TrajectoryEventType::Compaction,
        RuntimeEvent::Recovery { .. } => TrajectoryEventType::Recovery,
        RuntimeEvent::Retry { .. } => TrajectoryEventType::Retry,
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
        RuntimeEvent::Text { role, .. } if role == "user" => TrajectoryEventType::UserInput,
        RuntimeEvent::Text { .. } | RuntimeEvent::Notification { .. } => {
            TrajectoryEventType::Message
        }
        RuntimeEvent::StepAdded { .. }
        | RuntimeEvent::StepStarted { .. }
        | RuntimeEvent::StepFinished { .. }
        | RuntimeEvent::ToolMeta { .. }
        | RuntimeEvent::BackgroundTaskFinished { .. }
        | RuntimeEvent::SubagentFinished { .. }
        | RuntimeEvent::SubagentsChanged { .. } => TrajectoryEventType::ToolCall,
        RuntimeEvent::TaskComplete { .. }
        | RuntimeEvent::Info { .. }
        | RuntimeEvent::MdInfo { .. }
        | RuntimeEvent::HookContext { .. }
        | RuntimeEvent::HookStatus { .. }
        | RuntimeEvent::PopupMarkdown { .. }
        | RuntimeEvent::TasksChanged { .. } => TrajectoryEventType::Message,
        RuntimeEvent::StepFailed { .. } | RuntimeEvent::Error { .. } => TrajectoryEventType::Error,
        RuntimeEvent::PluginStarted { .. }
        | RuntimeEvent::PluginStopped { .. }
        | RuntimeEvent::Plugin { .. } => TrajectoryEventType::PluginLifecycle,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_protocol::{RunId, RuntimeEvent, StepId, TrajectoryId};

    #[tokio::test]
    async fn query_returns_appended_facts_in_order() {
        let service = KernelTrajectoryRecorder::new(TrajectoryRecorder::default());
        let trajectory = TrajectoryId::from("trajectory-query-test");
        let run = RunId::from("run-query-test");
        let events = vec![
            RuntimeEvent::RunStarted {
                run_id: run.clone(),
            },
            RuntimeEvent::Text {
                run_id: Some(run.clone()),
                role: "assistant".into(),
                content: "first".into(),
            },
            RuntimeEvent::RunFinished {
                run_id: Some(run.clone()),
                success: true,
            },
        ];
        for event in events {
            service
                .append(Some(&trajectory), Some(&run), event)
                .await
                .expect("append fact");
        }
        let facts = service.query(&trajectory, 0).await.expect("query facts");
        assert_eq!(facts.len(), 3);
        assert!(facts.windows(2).all(|w| w[0].sequence < w[1].sequence));
        assert_eq!(facts[0].event_type, TrajectoryEventType::RunLifecycle);

        // `replay` is the whole trajectory, so it starts at sequence 0.
        let replayed = service.replay(&trajectory).await.expect("replay facts");
        assert_eq!(
            replayed
                .iter()
                .map(|fact| fact.sequence)
                .collect::<Vec<_>>(),
            facts.iter().map(|fact| fact.sequence).collect::<Vec<_>>()
        );
        assert!(replayed[0].sequence == 0);
    }

    #[tokio::test]
    async fn nested_tool_call_records_its_parent_step() {
        let service = KernelTrajectoryRecorder::new(TrajectoryRecorder::default());
        let trajectory = TrajectoryId::from("trajectory-parent-test");
        let run = RunId::from("run-parent-test");
        service
            .append(
                Some(&trajectory),
                Some(&run),
                RuntimeEvent::ToolCallStarted {
                    run_id: run.clone(),
                    step_id: StepId::from("child-step"),
                    tool: "read_file".into(),
                    parent_step_id: Some(StepId::from("parent-step")),
                },
            )
            .await
            .expect("append");
        let facts = service.query(&trajectory, 0).await.expect("query");
        assert_eq!(
            facts[0].parent_step_id.as_ref().map(StepId::as_str),
            Some("parent-step")
        );
    }

    #[tokio::test]
    async fn typed_facts_classify_compaction_recovery_retry_and_user_input() {
        let service = KernelTrajectoryRecorder::new(TrajectoryRecorder::default());
        let trajectory = TrajectoryId::from("trajectory-facts-test");
        let run = RunId::from("run-facts-test");
        let events = vec![
            RuntimeEvent::Compaction {
                run_id: Some(run.clone()),
                trigger: "auto".into(),
                focus: None,
            },
            RuntimeEvent::Recovery {
                run_id: Some(run.clone()),
                attempt: 1,
                reason: "context too large".into(),
            },
            RuntimeEvent::Retry {
                run_id: Some(run.clone()),
                attempt: 1,
                reason: "transient".into(),
            },
            RuntimeEvent::Text {
                run_id: Some(run.clone()),
                role: "user".into(),
                content: "hello".into(),
            },
        ];
        for event in events {
            service
                .append(Some(&trajectory), Some(&run), event)
                .await
                .expect("append fact");
        }
        let facts = service.query(&trajectory, 0).await.expect("query facts");
        let types: Vec<_> = facts.iter().map(|fact| fact.event_type.clone()).collect();
        assert_eq!(
            types,
            vec![
                TrajectoryEventType::Compaction,
                TrajectoryEventType::Recovery,
                TrajectoryEventType::Retry,
                TrajectoryEventType::UserInput,
            ]
        );
    }
}
