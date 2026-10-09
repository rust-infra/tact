use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tact_protocol::{RunId, StepId, TrajectoryId};

pub type ActorId = String;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Internal,
    Sensitive,
    Secret,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrajectoryEventType {
    RunLifecycle,
    UserInput,
    ModelCall,
    ToolCall,
    Permission,
    PluginLifecycle,
    Interaction,
    Message,
    Error,
    Retry,
    Cancellation,
    Timeout,
    Compaction,
    Recovery,
    PluginCustom,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryEvent {
    pub trajectory_id: TrajectoryId,
    pub run_id: RunId,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub actor: ActorId,
    pub event_type: TrajectoryEventType,
    pub parent_step_id: Option<StepId>,
    pub payload: Value,
    pub sensitivity: Sensitivity,
}
