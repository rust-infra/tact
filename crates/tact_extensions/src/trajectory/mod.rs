//! Durable execution facts.
//!
//! Trajectory model, store, and replay live in the `tact_trajectory` crate —
//! the Kernel owns the `TrajectoryService` boundary and this crate owns the
//! durable implementation. This module keeps the historical `tact::trajectory`
//! path working inside the extension crate.

pub use tact_trajectory::*;
