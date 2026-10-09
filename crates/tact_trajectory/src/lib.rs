//! Trajectory: the durable record of a Runtime execution.
//!
//! Event transport delivers live; this crate stores ordered facts for replay,
//! recovery, debugging, audit, and evaluation. It implements the Kernel's
//! [`tact::TrajectoryService`] boundary, so a host that wants durable facts
//! installs one of these and the Kernel emits into it.

pub mod model;
pub mod service;

pub use model::{
    ActorId, Sensitivity, SqliteTrajectoryRecorder, TrajectoryEvent, TrajectoryEventType,
    TrajectoryRecorder,
};
pub use service::{KernelTrajectoryRecorder, SqliteTrajectoryService};

/// The redaction policy the durable recorder applies before persisting.
pub use tact::redact::RedactionConfig as TrajectoryRedactionConfig;
