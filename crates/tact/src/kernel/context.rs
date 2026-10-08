//! Context and service boundaries used by Kernel capability invocations.

use std::{sync::Arc, time::Duration};

use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{
    CapabilityDeclaration, InteractionRequest, InteractionResponse, PluginId, RequestId, RunId,
    RuntimeEvent, SessionId, TrajectoryId,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use super::{CancellationService, CapabilityRouter, KernelError};

/// Event delivery supplied by a Runtime host.
///
/// The trait deliberately takes protocol values rather than a concrete event
/// bus. Remote hosts can implement it by serializing the same value over IPC.
#[async_trait]
pub trait EventService: Send + Sync {
    async fn publish(&self, event: RuntimeEvent) -> Result<(), KernelError>;
}

/// Append-only execution facts supplied by a Runtime host.
#[async_trait]
pub trait TrajectoryService: Send + Sync {
    async fn append(
        &self,
        trajectory_id: Option<&TrajectoryId>,
        run_id: Option<&RunId>,
        event: RuntimeEvent,
    ) -> Result<(), KernelError>;
}

/// Permission boundary shared by native and remote capabilities.
///
/// Task 3 supplies the policy implementation. The Kernel only knows that an
/// invocation must be authorized before its handler runs.
#[async_trait]
pub trait PermissionService: Send + Sync {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        context: &InvocationContext,
        input: &Value,
    ) -> Result<(), KernelError>;

    async fn request(
        &self,
        _request: InteractionRequest,
        _context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError> {
        Err(KernelError::permission_denied(
            "permission interaction is not available",
        ))
    }
}

/// Namespaced storage boundary supplied by a Runtime host.
#[async_trait]
pub trait StorageService: Send + Sync {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>, KernelError>;

    async fn set(&self, namespace: &str, key: &str, value: Value) -> Result<(), KernelError>;
}

/// The services visible to one invocation.
#[derive(Clone)]
pub struct RuntimeServices {
    pub(crate) events: Arc<dyn EventService>,
    pub(crate) trajectory: Arc<dyn TrajectoryService>,
    pub(crate) permission: Arc<dyn PermissionService>,
    pub(crate) storage: Arc<dyn StorageService>,
}

impl RuntimeServices {
    #[must_use]
    pub fn new(
        events: Arc<dyn EventService>,
        trajectory: Arc<dyn TrajectoryService>,
        permission: Arc<dyn PermissionService>,
        storage: Arc<dyn StorageService>,
    ) -> Self {
        Self {
            events,
            trajectory,
            permission,
            storage,
        }
    }

    #[must_use]
    pub fn noop() -> Self {
        Self::new(
            Arc::new(NoopEventService),
            Arc::new(NoopTrajectoryService),
            Arc::new(AllowPermissionService),
            Arc::new(NoopStorageService),
        )
    }
}

/// Runtime-wide services and the generic capability router.
#[derive(Clone)]
pub struct RuntimeContext {
    router: CapabilityRouter,
    cancellation: CancellationService,
    services: RuntimeServices,
}

impl RuntimeContext {
    /// Creates a context with no-op event, trajectory, permission and storage
    /// services. This is useful for an in-process host that only needs routing.
    #[must_use]
    pub fn new(router: CapabilityRouter) -> Self {
        Self::with_services(router, RuntimeServices::noop())
    }

    #[must_use]
    pub fn with_services(router: CapabilityRouter, services: RuntimeServices) -> Self {
        Self {
            router,
            cancellation: CancellationService::new(),
            services,
        }
    }

    #[must_use]
    pub fn router(&self) -> &CapabilityRouter {
        &self.router
    }

    #[must_use]
    pub fn capability_router(&self) -> &CapabilityRouter {
        &self.router
    }

    #[must_use]
    pub fn cancellation(&self) -> &CancellationService {
        &self.cancellation
    }

    #[must_use]
    pub fn services(&self) -> &RuntimeServices {
        &self.services
    }

    /// Builds an invocation context using the Runtime's shared services.
    #[must_use]
    pub fn invocation(
        &self,
        request_id: RequestId,
        plugin_id: PluginId,
        actor: impl Into<String>,
    ) -> InvocationContext {
        let cancellation = self.cancellation.child_for(request_id.clone());
        InvocationContext::from_parts(
            request_id,
            plugin_id,
            actor,
            self.services.clone(),
            cancellation,
        )
    }
}

impl Default for RuntimeContext {
    fn default() -> Self {
        Self::new(CapabilityRouter::new())
    }
}

/// The per-call context passed to a capability implementation.
#[derive(Clone)]
pub struct InvocationContext {
    request_id: RequestId,
    plugin_id: PluginId,
    actor: String,
    session_id: Option<SessionId>,
    run_id: Option<RunId>,
    trajectory_id: Option<TrajectoryId>,
    deadline: Option<Instant>,
    cancellation: CancellationToken,
    services: RuntimeServices,
}

impl InvocationContext {
    #[must_use]
    pub fn new(request_id: RequestId, plugin_id: PluginId, actor: impl Into<String>) -> Self {
        Self::from_parts(
            request_id,
            plugin_id,
            actor,
            RuntimeServices::noop(),
            CancellationToken::new(),
        )
    }

    #[must_use]
    pub(crate) fn from_parts(
        request_id: RequestId,
        plugin_id: PluginId,
        actor: impl Into<String>,
        services: RuntimeServices,
        cancellation: CancellationToken,
    ) -> Self {
        Self {
            request_id,
            plugin_id,
            actor: actor.into(),
            session_id: None,
            run_id: None,
            trajectory_id: None,
            deadline: None,
            cancellation,
            services,
        }
    }

    #[must_use]
    pub fn request_id(&self) -> &RequestId {
        &self.request_id
    }

    #[must_use]
    pub fn plugin_id(&self) -> &PluginId {
        &self.plugin_id
    }

    #[must_use]
    pub fn actor(&self) -> &str {
        &self.actor
    }

    #[must_use]
    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    #[must_use]
    pub fn run_id(&self) -> Option<&RunId> {
        self.run_id.as_ref()
    }

    #[must_use]
    pub fn trajectory_id(&self) -> Option<&TrajectoryId> {
        self.trajectory_id.as_ref()
    }

    #[must_use]
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    #[must_use]
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }

    #[must_use]
    pub fn events(&self) -> &Arc<dyn EventService> {
        &self.services.events
    }

    #[must_use]
    pub fn trajectory(&self) -> &Arc<dyn TrajectoryService> {
        &self.services.trajectory
    }

    #[must_use]
    pub fn permission(&self) -> &Arc<dyn PermissionService> {
        &self.services.permission
    }

    #[must_use]
    pub fn storage(&self) -> &Arc<dyn StorageService> {
        &self.services.storage
    }

    #[must_use]
    pub fn with_session_id(mut self, session_id: SessionId) -> Self {
        self.session_id = Some(session_id);
        self
    }

    #[must_use]
    pub fn with_run_id(mut self, run_id: RunId) -> Self {
        self.run_id = Some(run_id);
        self
    }

    #[must_use]
    pub fn with_trajectory_id(mut self, trajectory_id: TrajectoryId) -> Self {
        self.trajectory_id = Some(trajectory_id);
        self
    }

    #[must_use]
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    #[must_use]
    pub fn with_timeout(self, timeout: Duration) -> Self {
        self.with_deadline(Instant::now() + timeout)
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancellation.is_cancelled()
    }

    #[must_use]
    pub fn is_expired(&self) -> bool {
        self.deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    pub(crate) fn ensure_active(&self) -> Result<(), KernelError> {
        if self.is_cancelled() {
            return Err(KernelError::cancelled()
                .with_request_id(self.request_id.clone())
                .with_plugin_id(self.plugin_id.clone()));
        }
        if self.is_expired() {
            return Err(KernelError::timeout()
                .with_request_id(self.request_id.clone())
                .with_plugin_id(self.plugin_id.clone()));
        }
        Ok(())
    }

    pub(crate) fn cancel(&self) {
        self.cancellation.cancel();
    }
}

struct NoopEventService;

#[async_trait]
impl EventService for NoopEventService {
    async fn publish(&self, _event: RuntimeEvent) -> Result<(), KernelError> {
        Ok(())
    }
}

struct NoopTrajectoryService;

#[async_trait]
impl TrajectoryService for NoopTrajectoryService {
    async fn append(
        &self,
        _trajectory_id: Option<&TrajectoryId>,
        _run_id: Option<&RunId>,
        _event: RuntimeEvent,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

struct AllowPermissionService;

#[async_trait]
impl PermissionService for AllowPermissionService {
    async fn check(
        &self,
        _declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        _input: &Value,
    ) -> Result<(), KernelError> {
        Ok(())
    }
}

struct NoopStorageService;

#[async_trait]
impl StorageService for NoopStorageService {
    async fn get(&self, _namespace: &str, _key: &str) -> Result<Option<Value>, KernelError> {
        Ok(None)
    }

    async fn set(&self, _namespace: &str, _key: &str, _value: Value) -> Result<(), KernelError> {
        Ok(())
    }
}
