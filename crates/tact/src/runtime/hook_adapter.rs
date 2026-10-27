//! Narrow services available to plugin hooks.

use anyhow::Result;
use serde_json::Value;

use crate::mcp::McpCatalogHandle;

/// The only MCP capability exposed to hook callbacks.
///
/// This handle can invoke the reviewed `mcp_tool` declared by a hook. It does
/// not expose the model tool catalog, arbitrary tool execution, or router
/// mutation.
#[derive(Clone)]
pub(crate) struct HookMcpHandle {
    catalog: McpCatalogHandle,
}

impl HookMcpHandle {
    pub(crate) fn new(catalog: McpCatalogHandle) -> Self {
        Self { catalog }
    }

    pub(crate) async fn call(&self, name: &str, arguments: Value) -> Result<String> {
        self.catalog.call(name, arguments).await
    }
}
