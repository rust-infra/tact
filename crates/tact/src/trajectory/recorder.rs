use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use serde_json::Value;
use tact_protocol::{RunId, StepId, TrajectoryId};

use super::{ActorId, Sensitivity, TrajectoryEvent, TrajectoryEventType};

#[derive(Clone, Default)]
pub struct TrajectoryRecorder {
    events: Arc<Mutex<BTreeMap<TrajectoryId, Vec<TrajectoryEvent>>>>,
}

impl TrajectoryRecorder {
    pub fn append(
        &self,
        trajectory_id: TrajectoryId,
        run_id: RunId,
        actor: ActorId,
        event_type: TrajectoryEventType,
        parent_step_id: Option<StepId>,
        payload: Value,
        sensitivity: Sensitivity,
    ) -> Result<TrajectoryEvent, String> {
        let mut events = self
            .events
            .lock()
            .map_err(|_| "trajectory lock poisoned".to_string())?;
        let sequence = events
            .get(&trajectory_id)
            .and_then(|items| items.last())
            .map_or(0, |event| event.sequence + 1);
        let event = TrajectoryEvent {
            trajectory_id: trajectory_id.clone(),
            run_id,
            sequence,
            timestamp: Utc::now(),
            actor,
            event_type,
            parent_step_id,
            payload,
            sensitivity,
        };
        events.entry(trajectory_id).or_default().push(event.clone());
        Ok(event)
    }

    pub fn query(
        &self,
        trajectory_id: &TrajectoryId,
        from_sequence: u64,
    ) -> Result<Vec<TrajectoryEvent>, String> {
        let events = self
            .events
            .lock()
            .map_err(|_| "trajectory lock poisoned".to_string())?;
        Ok(events
            .get(trajectory_id)
            .map(|items| {
                items
                    .iter()
                    .filter(|event| event.sequence >= from_sequence)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }
}
