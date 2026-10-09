//! Adapter from Tact's existing permission policy to the Kernel boundary.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{
    CapabilityDeclaration, CapabilityRisk as ProtocolRisk, InteractionRequest, InteractionResponse,
};

use crate::permission::{CapabilityRisk, PermissionBehavior, PermissionManager};

use tact::{InvocationContext, KernelError, PermissionService};

/// Client-facing approval hook used by a Runtime host.
#[async_trait]
pub trait PermissionResponder: Send + Sync {
    async fn request(
        &self,
        request: InteractionRequest,
        context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError>;
}

/// Makes the existing stateful permission manager available to every
/// capability invocation, including remote plugin calls.
pub struct PermissionManagerService {
    manager: Mutex<PermissionManager>,
    responder: Option<Arc<dyn PermissionResponder>>,
}

impl PermissionManagerService {
    #[must_use]
    pub fn new(manager: PermissionManager) -> Self {
        Self {
            manager: Mutex::new(manager),
            responder: None,
        }
    }

    #[must_use]
    pub fn with_responder(
        manager: PermissionManager,
        responder: Arc<dyn PermissionResponder>,
    ) -> Self {
        Self {
            manager: Mutex::new(manager),
            responder: Some(responder),
        }
    }

    fn risk(risk: ProtocolRisk) -> CapabilityRisk {
        match risk {
            ProtocolRisk::ReadOnly | ProtocolRisk::Low => CapabilityRisk::Read,
            ProtocolRisk::Medium => CapabilityRisk::Write,
            ProtocolRisk::High | ProtocolRisk::Critical => CapabilityRisk::High,
        }
    }

    fn decision_error(
        declaration: &CapabilityDeclaration,
        behavior: PermissionBehavior,
        reason: String,
    ) -> Result<(), KernelError> {
        match behavior {
            PermissionBehavior::Allow => Ok(()),
            PermissionBehavior::Deny => Err(KernelError::permission_denied(format!(
                "{}: {}",
                declaration.name, reason
            ))),
            PermissionBehavior::Ask => Err(KernelError::permission_denied(format!(
                "{} requires user approval: {}",
                declaration.name, reason
            ))),
        }
    }
}

#[async_trait]
impl PermissionService for PermissionManagerService {
    async fn check(
        &self,
        declaration: &CapabilityDeclaration,
        _context: &InvocationContext,
        input: &Value,
    ) -> Result<(), KernelError> {
        let mut manager = self.manager.lock().map_err(|_| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                "permission manager lock poisoned",
                "permission",
                true,
            )
        })?;
        let decision = manager.check(&declaration.name, Self::risk(declaration.risk), input);
        Self::decision_error(declaration, decision.behavior, decision.reason)
    }

    async fn request(
        &self,
        request: InteractionRequest,
        context: &InvocationContext,
    ) -> Result<InteractionResponse, KernelError> {
        let Some(responder) = self.responder.as_ref() else {
            return Err(KernelError::permission_denied(
                "permission interaction is not available",
            ));
        };
        responder.request(request, context).await
    }
}
