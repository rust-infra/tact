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
    /// Appends one fact.
    ///
    /// `too_many_arguments` is allowed because this is the recorder's stable
    /// positional contract, mirroring the spec's `TrajectoryEvent` fields one
    /// for one; collapsing them into a struct would only move the parameter
    /// list here without changing the API a caller writes.
    #[allow(clippy::too_many_arguments)]
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

    /// Every fact recorded for `run_id`, across trajectories, in
    /// (trajectory, sequence) order.
    pub fn query_by_run(&self, run_id: &RunId) -> Result<Vec<TrajectoryEvent>, String> {
        let events = self
            .events
            .lock()
            .map_err(|_| "trajectory lock poisoned".to_string())?;
        let mut facts: Vec<TrajectoryEvent> = events
            .values()
            .flatten()
            .filter(|event| &event.run_id == run_id)
            .cloned()
            .collect();
        facts.sort_by(|a, b| {
            (a.trajectory_id.as_str(), a.sequence).cmp(&(b.trajectory_id.as_str(), b.sequence))
        });
        Ok(facts)
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
