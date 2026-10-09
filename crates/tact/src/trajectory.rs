//! Trajectory recording boundary.
//!
//! The Kernel records execution facts on one boundary; the durable model,
//! store, and replay implementation live in `tact_trajectory`, which implements
//! [`TrajectoryService`]. A host installs one of those and the Kernel emits
//! into it, so a fact is recorded on the same path that publishes the event.

use async_trait::async_trait;
use tact_protocol::{RunId, RuntimeEvent, TrajectoryId};

use crate::KernelError;

#[async_trait]
pub trait TrajectoryService: Send + Sync {
    async fn append(
        &self,
        trajectory_id: Option<&TrajectoryId>,
        run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError>;
}
