//! Durable execution facts shared by Runtime clients and extensions.

mod model;
mod recorder;

pub use model::{ActorId, Sensitivity, TrajectoryEvent, TrajectoryEventType};
pub use recorder::TrajectoryRecorder;
