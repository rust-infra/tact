use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{RunId, RuntimeEvent, StepId, TrajectoryId};

use crate::trajectory::{
    Sensitivity, SqliteTrajectoryRecorder, TrajectoryEventType, TrajectoryRecorder,
};

use super::{KernelError, TrajectoryService};

#[derive(Clone, Default)]
pub struct KernelTrajectoryRecorder {
    recorder: TrajectoryRecorder,
}

#[derive(Clone)]
pub struct SqliteTrajectoryService {
    recorder: SqliteTrajectoryRecorder,
}

impl SqliteTrajectoryService {
    #[must_use]
    pub fn new(recorder: SqliteTrajectoryRecorder) -> Self {
        Self { recorder }
    }
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
            RuntimeEvent::ModelCallStarted { .. } | RuntimeEvent::ModelCallFinished { .. } => {
                TrajectoryEventType::ModelCall
            }
            RuntimeEvent::ToolCallStarted { .. } | RuntimeEvent::ToolCallFinished { .. } => {
                TrajectoryEventType::ToolCall
            }
            RuntimeEvent::PermissionRequested { .. } | RuntimeEvent::PermissionResolved { .. } => {
                TrajectoryEventType::Permission
            }
            RuntimeEvent::InteractionRequested { .. }
            | RuntimeEvent::InteractionResponded { .. } => TrajectoryEventType::Interaction,
            RuntimeEvent::Text { .. } | RuntimeEvent::Notification { .. } => {
                TrajectoryEventType::Message
            }
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
                Value::from(payload),
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
        RuntimeEvent::ModelCallStarted { .. } | RuntimeEvent::ModelCallFinished { .. } => {
            TrajectoryEventType::ModelCall
        }
        RuntimeEvent::ToolCallStarted { .. } | RuntimeEvent::ToolCallFinished { .. } => {
            TrajectoryEventType::ToolCall
        }
        RuntimeEvent::PermissionRequested { .. } | RuntimeEvent::PermissionResolved { .. } => {
            TrajectoryEventType::Permission
        }
        RuntimeEvent::InteractionRequested { .. } | RuntimeEvent::InteractionResponded { .. } => {
            TrajectoryEventType::Interaction
        }
        RuntimeEvent::Text { .. } | RuntimeEvent::Notification { .. } => {
            TrajectoryEventType::Message
        }
        RuntimeEvent::Error { .. } => TrajectoryEventType::Error,
        RuntimeEvent::PluginStarted { .. }
        | RuntimeEvent::PluginStopped { .. }
        | RuntimeEvent::Plugin { .. } => TrajectoryEventType::PluginLifecycle,
    }
}
