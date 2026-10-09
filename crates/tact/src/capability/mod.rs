pub(crate) mod mcp_tool;
pub(crate) mod native_tool;

use crate::{
    kernel::{CapabilityRouter, KernelError},
    mcp::MCPToolRouter,
    security::{RedactionConfig, sensitive::Scanner},
    tool::{ToolContext, ToolRouter},
};

/// Builds the shared invocation registry for the current native and MCP
/// toolsets. Agent preflight still owns interaction timing during migration;
/// execution itself enters through `CapabilityRouter::invoke`.
pub fn register_tool_capabilities(
    native: &ToolRouter,
    mcp: &MCPToolRouter,
    context: ToolContext,
    scanner: Scanner,
    redaction: RedactionConfig,
) -> Result<CapabilityRouter, KernelError> {
    let mut registrations = native.capability_registrations(context.clone(), scanner, redaction);
    registrations.extend(mcp.capability_registrations(context));
    let router = CapabilityRouter::new();
    router.register_many(registrations)?;
    Ok(router)
}

#[cfg(test)]
mod router_tests;
