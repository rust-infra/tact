//! Protocol-neutral Runtime Kernel services.
//!
//! This module deliberately has no dependency on TUI, Chat, Agent, or a
//! concrete plugin language. Hosts provide the service traits in
//! [`context`] and register capability implementations with [`CapabilityRouter`].

mod cancellation;
mod capability;
mod context;
mod error;
mod event;
mod interaction;
mod permission;
mod storage;
mod trajectory;

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
pub use permission::{PermissionManagerService, PermissionResponder};
pub use storage::{SqliteStorageService, StorageNamespace, StorageServiceImpl};
pub use trajectory::{KernelTrajectoryRecorder, SqliteTrajectoryService};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod interaction_tests;
