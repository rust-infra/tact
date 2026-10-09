//! Adapters from MCP tools, prompts, and resources to the Capability Router.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value};

use crate::{
    mcp::{MCPToolRouter, McpPromptTool, McpResourceTool},
    tool::{ToolCallResult, ToolContext},
};
use tact::{CapabilityHandler, InvocationContext, KernelError};

pub(crate) enum McpCapabilityTarget {
    Tool {
        name: String,
        output_token_limit: Option<usize>,
    },
    Resource(McpResourceTool),
    Prompt(McpPromptTool),
}

pub(crate) struct McpToolCapabilityHandler {
    router: Arc<MCPToolRouter>,
    target: McpCapabilityTarget,
    tool_context: ToolContext,
}

impl McpToolCapabilityHandler {
    pub(crate) fn new(
        router: Arc<MCPToolRouter>,
        target: McpCapabilityTarget,
        tool_context: ToolContext,
    ) -> Self {
        Self {
            router,
            target,
            tool_context,
        }
    }
}

#[async_trait]
impl CapabilityHandler for McpToolCapabilityHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let result = match &self.target {
            McpCapabilityTarget::Tool {
                name,
                output_token_limit,
            } => {
                invoke_mcp_tool(
                    &self.router,
                    &self.tool_context,
                    name,
                    context.request_id().as_str(),
                    *output_token_limit,
                    input,
                )
                .await
            }
            McpCapabilityTarget::Resource(tool) => {
                invoke_resource(&self.router, *tool, &input).await
            }
            McpCapabilityTarget::Prompt(tool) => invoke_prompt(&self.router, *tool, &input).await,
        };
        let output = result.map_err(|error| {
            let error = error.to_string();
            let storage_error = error.starts_with("Error persisting large MCP output:");
            let message = match &self.target {
                McpCapabilityTarget::Tool { .. } => error,
                McpCapabilityTarget::Resource(tool) => {
                    format!("Error invoking {}: {error}", tool.name())
                }
                McpCapabilityTarget::Prompt(tool) => {
                    format!("Error invoking {}: {error}", tool.name())
                }
            };
            KernelError::new(
                if storage_error {
                    tact_protocol::ErrorCategory::StorageError
                } else {
                    tact_protocol::ErrorCategory::ToolError
                },
                message,
                if storage_error { "storage" } else { "mcp_tool" },
                true,
            )
            .with_request_id(context.request_id().clone())
            .with_plugin_id(context.plugin_id().clone())
        })?;
        serde_json::to_value(ToolCallResult::text(output)).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                error.to_string(),
                "mcp_tool",
                false,
            )
            .with_request_id(context.request_id().clone())
            .with_plugin_id(context.plugin_id().clone())
        })
    }
}

async fn persist_mcp_output(
    context: &ToolContext,
    tool_use_id: &str,
    output: &str,
    limit_tokens: Option<usize>,
) -> anyhow::Result<String> {
    let tact_path = crate::consts::TactPath::new(&context.work_dir);
    match limit_tokens {
        Some(limit) => {
            crate::compact::persist_large_output_over_tokens(&tact_path, tool_use_id, output, limit)
                .await
        }
        None => crate::compact::persist_large_output(&tact_path, tool_use_id, output).await,
    }
}

async fn invoke_mcp_tool(
    router: &MCPToolRouter,
    context: &ToolContext,
    name: &str,
    tool_use_id: &str,
    limit_tokens: Option<usize>,
    input: Value,
) -> anyhow::Result<String> {
    let output = router.call(name, input).await?;
    persist_mcp_output(context, tool_use_id, &output, limit_tokens)
        .await
        .map_err(|error| anyhow::anyhow!("Error persisting large MCP output: {error}"))
}

async fn invoke_resource(
    router: &MCPToolRouter,
    tool: McpResourceTool,
    input: &Value,
) -> anyhow::Result<String> {
    let server = input.get("server").and_then(Value::as_str);
    match tool {
        McpResourceTool::List => router.list_resources(server).await,
        McpResourceTool::Templates => router.list_resource_templates(server).await,
        McpResourceTool::Read => {
            let server = server.ok_or_else(|| anyhow::anyhow!("`server` is required"))?;
            let uri = input
                .get("uri")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("`uri` is required"))?;
            router.read_resource(server, uri).await
        }
    }
}

async fn invoke_prompt(
    router: &MCPToolRouter,
    tool: McpPromptTool,
    input: &Value,
) -> anyhow::Result<String> {
    let server = input.get("server").and_then(Value::as_str);
    match tool {
        McpPromptTool::List => router.list_prompts(server).await,
        McpPromptTool::Get => {
            let server = server.ok_or_else(|| anyhow::anyhow!("`server` is required"))?;
            let name = input
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("`name` is required"))?;
            router
                .get_prompt(server, name, prompt_arguments(input)?)
                .await
        }
    }
}

fn prompt_arguments(input: &Value) -> anyhow::Result<Option<Map<String, Value>>> {
    let Some(arguments) = input.get("arguments").and_then(Value::as_object) else {
        return Ok(None);
    };
    let mut normalized = Map::new();
    for (key, value) in arguments {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Number(_) | Value::Bool(_) => value.to_string(),
            _ => anyhow::bail!("argument `{key}` must be a string (got {value})"),
        };
        normalized.insert(key.clone(), Value::String(text));
    }
    Ok((!normalized.is_empty()).then_some(normalized))
}
