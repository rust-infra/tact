//! The durable execution-fact domain type shared by the Kernel boundary and the
//! trajectory store.
//!
//! This is a wire/domain type, not a Rust-only struct: a host serializes it
//! exactly like a Runtime event, so a View can replay facts from a sequence
//! without depending on the recorder implementation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{RunId, StepId, TrajectoryId};

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
    /// `None` for a fact that belongs to no run (a plugin lifecycle event, a
    /// notice, a turn cancelled before it started). It is not attributed to a
    /// synthetic run: two unrelated facts must not share an identity.
    pub run_id: Option<RunId>,
    pub sequence: u64,
    pub timestamp: DateTime<Utc>,
    pub actor: ActorId,
    pub event_type: TrajectoryEventType,
    pub parent_step_id: Option<StepId>,
    pub payload: Value,
    pub sensitivity: Sensitivity,
}
