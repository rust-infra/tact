//! Adapter from native `Tool` handlers to the Runtime Capability Router.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::{
    security::{RedactionConfig, redact::level_for_call, sensitive::Scanner},
    tool::{OutputPolicy, Tool, ToolCallResult, ToolContext, ToolMetadata},
};
use tact::{CapabilityHandler, InvocationContext, KernelError};

pub(crate) struct NativeToolCapabilityHandler {
    tool: Arc<dyn Tool>,
    metadata: &'static ToolMetadata,
    output_policy: OutputPolicy,
    context: ToolContext,
    scanner: Scanner,
    redaction: RedactionConfig,
}

impl NativeToolCapabilityHandler {
    pub(crate) fn new(
        tool: Arc<dyn Tool>,
        metadata: &'static ToolMetadata,
        context: ToolContext,
        scanner: Scanner,
        redaction: RedactionConfig,
    ) -> Self {
        Self {
            tool,
            metadata,
            output_policy: metadata.output,
            context,
            scanner,
            redaction,
        }
    }
}

#[async_trait]
impl CapabilityHandler for NativeToolCapabilityHandler {
    async fn invoke(&self, context: InvocationContext, input: Value) -> Result<Value, KernelError> {
        let sensitive = self
            .metadata
            .permission
            .sensitive(&input, &self.scanner)
            .is_some();
        let target = self.metadata.permission.target(&input);
        let redaction = level_for_call(&self.redaction, sensitive, target.as_deref());
        let tool_context = self
            .context
            .for_invocation_with_redaction(context.request_id().as_str(), redaction);
        let mut result: ToolCallResult =
            self.tool.call(tool_context, input).await.map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::ToolError,
                    error.to_string(),
                    "native_tool",
                    false,
                )
                .with_request_id(context.request_id().clone())
                .with_plugin_id(context.plugin_id().clone())
            })?;
        if self.output_policy == OutputPolicy::PersistLargeOutput {
            let tact_path = crate::consts::TactPath::new(&self.context.work_dir);
            result.content = crate::compact::persist_large_output(
                &tact_path,
                context.request_id().as_str(),
                &result.content,
            )
            .await
            .map_err(|error| {
                KernelError::new(
                    tact_protocol::ErrorCategory::StorageError,
                    format!("Error persisting large output: {error}"),
                    "storage",
                    true,
                )
                .with_request_id(context.request_id().clone())
                .with_plugin_id(context.plugin_id().clone())
            })?;
        }
        serde_json::to_value(result).map_err(|error| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                error.to_string(),
                "native_tool",
                false,
            )
            .with_request_id(context.request_id().clone())
            .with_plugin_id(context.plugin_id().clone())
        })
    }
}
