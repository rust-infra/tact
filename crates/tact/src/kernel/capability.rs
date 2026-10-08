//! Registration and invocation of protocol-declared capabilities.

use std::{
    collections::BTreeMap,
    future::Future,
    pin::Pin,
    sync::{Arc, RwLock},
};

use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::CapabilityDeclaration;
use tokio::time;

use super::{InvocationContext, KernelError};

/// Object-safe implementation boundary for a capability.
#[async_trait]
pub trait CapabilityHandler: Send + Sync {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError>;
}

/// A declaration and its implementation, ready for registration.
pub struct CapabilityRegistration {
    declaration: CapabilityDeclaration,
    handler: Arc<dyn CapabilityHandler>,
}

impl CapabilityRegistration {
    #[must_use]
    pub fn new(declaration: CapabilityDeclaration, handler: Arc<dyn CapabilityHandler>) -> Self {
        Self {
            declaration,
            handler,
        }
    }

    #[must_use]
    pub fn declaration(&self) -> &CapabilityDeclaration {
        &self.declaration
    }

    #[must_use]
    pub fn handler(&self) -> &Arc<dyn CapabilityHandler> {
        &self.handler
    }
}

/// Adapts a closure to [`CapabilityHandler`] for small in-process extensions.
pub struct FnCapabilityHandler<F>(F);

impl<F> FnCapabilityHandler<F> {
    #[must_use]
    pub fn new(handler: F) -> Self {
        Self(handler)
    }
}

#[async_trait]
impl<F, Fut> CapabilityHandler for FnCapabilityHandler<F>
where
    F: Fn(InvocationContext, Value) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, KernelError>> + Send,
{
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        (self.0)(context, input).await
    }
}

struct RegisteredCapability {
    declaration: CapabilityDeclaration,
    handler: Arc<dyn CapabilityHandler>,
}

/// Concurrent registry and invocation entry point for all capabilities.
#[derive(Clone, Default)]
pub struct CapabilityRouter {
    capabilities: Arc<RwLock<BTreeMap<String, RegisteredCapability>>>,
}

impl CapabilityRouter {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers one capability. Names are unique within a Router.
    pub fn register(&self, registration: CapabilityRegistration) -> Result<(), KernelError> {
        registration.declaration.validate().map_err(|message| {
            KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                message,
                "kernel",
                false,
            )
        })?;
        let name = registration.declaration.name.clone();
        let mut capabilities = self.capabilities.write().map_err(|_| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                "capability registry lock poisoned",
                "kernel",
                true,
            )
        })?;
        if capabilities.contains_key(&name) {
            return Err(KernelError::duplicate_capability(name));
        }
        capabilities.insert(
            name,
            RegisteredCapability {
                declaration: registration.declaration,
                handler: registration.handler,
            },
        );
        Ok(())
    }

    /// Registers an implementation without requiring callers to construct the
    /// registration wrapper themselves.
    pub fn register_handler<H>(
        &self,
        declaration: CapabilityDeclaration,
        handler: H,
    ) -> Result<(), KernelError>
    where
        H: CapabilityHandler + 'static,
    {
        self.register(CapabilityRegistration::new(declaration, Arc::new(handler)))
    }

    /// Returns a single declaration without exposing its implementation.
    pub fn describe(&self, name: &str) -> Option<CapabilityDeclaration> {
        self.capabilities
            .read()
            .ok()?
            .get(name)
            .map(|registered| registered.declaration.clone())
    }

    /// Returns declarations in stable name order for discovery.
    pub fn describe_all(&self) -> Vec<CapabilityDeclaration> {
        self.capabilities
            .read()
            .map(|capabilities| {
                capabilities
                    .values()
                    .map(|registered| registered.declaration.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Invokes a capability through the common permission, cancellation and
    /// deadline path.
    pub async fn invoke(
        &self,
        name: &str,
        context: InvocationContext,
        input: Value,
    ) -> Result<Value, KernelError> {
        let (declaration, handler) = {
            let capabilities = self.capabilities.read().map_err(|_| {
                KernelError::new(
                    tact_protocol::ErrorCategory::InternalError,
                    "capability registry lock poisoned",
                    "kernel",
                    true,
                )
            })?;
            let Some(registered) = capabilities.get(name) else {
                return Err(KernelError::capability_not_found(name)
                    .with_request_id(context.request_id().clone())
                    .with_plugin_id(context.plugin_id().clone()));
            };
            (
                registered.declaration.clone(),
                Arc::clone(&registered.handler),
            )
        };

        context.ensure_active()?;
        let permission = context.permission().check(&declaration, &context, &input);
        guarded(&context, permission)
            .await
            .map_err(|error| enrich_error(error, &context))?;
        context.ensure_active()?;

        guarded(&context, handler.invoke(context.clone(), input))
            .await
            .map_err(|error| enrich_error(error, &context))
    }
}

async fn guarded<T, F>(context: &InvocationContext, future: F) -> Result<T, KernelError>
where
    F: Future<Output = Result<T, KernelError>> + Send,
{
    let cancellation = context.cancellation_token();
    tokio::pin!(future);
    match context.deadline() {
        Some(deadline) => {
            tokio::select! {
                result = &mut future => result,
                _ = cancellation.cancelled() => Err(KernelError::cancelled()),
                _ = time::sleep_until(deadline) => {
                    context.cancel();
                    Err(KernelError::timeout())
                },
            }
        }
        None => {
            tokio::select! {
                result = &mut future => result,
                _ = cancellation.cancelled() => Err(KernelError::cancelled()),
            }
        }
    }
}

fn enrich_error(mut error: KernelError, context: &InvocationContext) -> KernelError {
    if error.request_id().is_none() {
        error = error.with_request_id(context.request_id().clone());
    }
    if error.plugin_id().is_none() {
        error = error.with_plugin_id(context.plugin_id().clone());
    }
    error
}

/// Convenient boxed future alias for adapters that do not use `async_trait`.
pub type CapabilityFuture = Pin<Box<dyn Future<Output = Result<Value, KernelError>> + Send>>;
