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
pub mod paths;
pub mod plugin;
pub mod redact;
pub mod sqlite;
pub mod storage;

mod cancellation;

pub use cancellation::CancellationService;
pub use capability::{
    CapabilityFuture, CapabilityHandler, CapabilityRegistration, CapabilityRouter,
    FnCapabilityHandler,
};
pub use context::{
    EventService, InvocationContext, PermissionService, RuntimeContext, RuntimeServices,
    StorageService, TrajectoryService,
};
pub use error::KernelError;
pub use event::{EventObserver, EventSubscription, EventTransport, RuntimeEventSink};
pub use interaction::{InteractionBroker, InteractionService, InteractionSubscription};
pub use plugin::{PluginRegistry, PluginState, RuntimePluginManifest};
pub use redact::{RedactionConfig, RedactionLevel};
pub use storage::{SqliteStorageService, StorageNamespace, StorageServiceImpl};

#[cfg(test)]
mod interaction_tests;
#[cfg(test)]
mod tests;
