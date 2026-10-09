//! Trajectory recording boundary.
//!
//! The Kernel records execution facts on one boundary; the durable model,
//! store, and replay implementation live in `tact_trajectory`, which implements
//! [`TrajectoryService`]. A host installs one of those and the Kernel emits
//! into it, so a fact is recorded on the same path that publishes the event.

use async_trait::async_trait;
use tact_protocol::{RunId, RuntimeEvent, TrajectoryEvent, TrajectoryId};

use crate::KernelError;

#[async_trait]
pub trait TrajectoryService: Send + Sync {
    async fn append(
        &self,
        trajectory_id: Option<&TrajectoryId>,
        run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError>;

    /// Returns the recorded facts for a trajectory from `from_sequence`,
    /// in ascending sequence order.
    ///
    /// A host that installed a durable recorder overrides this; the default is
    /// a named "not available" failure so a plugin that asks to read facts from
    /// a recorder-less runtime gets an explicit error rather than empty data.
    /// Returns every recorded fact for `run_id`, across trajectories.
    async fn query_by_run(&self, _run_id: &RunId) -> Result<Vec<TrajectoryEvent>, KernelError> {
        Err(KernelError::new(
            tact_protocol::ErrorCategory::CapabilityNotFound,
            "trajectory run query is not available",
            "trajectory",
            false,
        ))
    }

    async fn query(
        &self,
        _trajectory_id: &TrajectoryId,
        _from_sequence: u64,
    ) -> Result<Vec<TrajectoryEvent>, KernelError> {
        Err(KernelError::new(
            tact_protocol::ErrorCategory::CapabilityNotFound,
            "trajectory query is not available",
            "trajectory",
            false,
        ))
    }
}
