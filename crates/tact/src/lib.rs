//! The Tact Runtime Kernel.
//!
//! This crate is the protocol-neutral core the architecture diagram calls
//! *Tact Runtime Kernel*: plugin lifecycle, the capability router, the
//! permission boundary, event transport, the trajectory recorder, minimal
//! storage, protocol versioning, and cancellation / timeout / error handling.
//!
//! It deliberately does not depend on the extension crate
//! (`tact_extensions`), on any frontend, or on a concrete plugin language.
//! Hosts — `tact_plugin_node`, `tact_plugin_wasm`, and the in-process Rust
//! host that lives beside the Agent — all consume the same pieces from here,
//! so a capability cannot be invoked through a path that skips permission,
//! events, or the trajectory.

pub mod capability;
pub mod context;
pub mod error;
pub mod event;
pub mod interaction;
pub mod lifecycle;
pub mod paths;
pub mod permission;
pub mod protocol;
pub mod redact;
pub mod services;
pub mod sqlite;
pub mod storage;
pub mod trajectory;

mod cancellation;

pub use cancellation::CancellationService;
pub use capability::{
    CapabilityFuture, CapabilityHandler, CapabilityRegistration, CapabilityRouter,
    FnCapabilityHandler,
};
pub use context::{InvocationContext, RuntimeContext, RuntimeServices};
pub use error::KernelError;
pub use event::{EventObserver, EventService, EventSubscription, EventTransport, RuntimeEventSink};
pub use interaction::{InteractionBroker, InteractionService, InteractionSubscription};
pub use lifecycle::{PluginRegistry, PluginState, RuntimePluginManifest};
pub use permission::PermissionService;
pub use redact::{RedactionConfig, RedactionLevel};
pub use storage::{SqliteStorageService, StorageNamespace, StorageService, StorageServiceImpl};
pub use trajectory::TrajectoryService;

#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod tests;
