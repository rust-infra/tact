//! Compatibility facade for the independent runtime crate.
//!
//! The application still owns Agent composition here while the lifecycle and
//! capability ports live in `tact-runtime`.

pub use tact_contracts::capability::{
    ToolCallResult, ToolDescriptor, ToolImage, ToolInvocation, ToolOrigin, ToolResources,
};
pub use tact_contracts::run::{RunId, RunOutcome, RunProgress};
pub use tact_runtime::capability::ToolExecutor;
pub use tact_runtime::{RunError, RunMachine, RunState};
