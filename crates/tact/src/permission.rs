//! Centralized permission boundary.
//!
//! Every capability invocation — native, MCP, or plugin — is authorized through
//! [`PermissionService`] before its handler runs. The Kernel owns the *decision
//! boundary*; a host supplies the policy implementation. `Ok(())` is "allow";
//! an `Err(KernelError)` is a denial whose category and reason survive the
//! protocol boundary.

use async_trait::async_trait;
use serde_json::Value;
use tact_protocol::{CapabilityDeclaration, InteractionRequest, InteractionResponse};

use crate::{InvocationContext, KernelError};

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
