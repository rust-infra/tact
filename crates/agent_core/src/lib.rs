//! Provider-neutral runtime kernel for one bounded Tact agent activation.

mod deps;
mod kernel;
mod model_scope;
mod recovery;
mod scheduling;
mod state;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use deps::{AgentKernel, KernelDeps, KernelError, KernelExtensions};
pub use recovery::{
    CONTINUATION_MESSAGE, CONVERGENCE_CONTINUATION_MESSAGE, MAX_CONTINUATION_ATTEMPTS,
    MAX_TRANSPORT_ATTEMPTS, backoff_delay, continuation_message,
};
pub use scheduling::{calls_conflict, schedule_waves, waves_grouped};
