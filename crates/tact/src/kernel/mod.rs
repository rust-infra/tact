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
pub use permission::{PermissionManagerService, PermissionResponder};
pub use storage::{StorageNamespace, StorageServiceImpl};
pub use trajectory::KernelTrajectoryRecorder;

#[cfg(test)]
mod tests;
