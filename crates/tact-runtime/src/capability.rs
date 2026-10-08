//! Runtime-facing ports implemented by native and external capability owners.

use anyhow::Result;
use async_trait::async_trait;
use tact_contracts::capability::{ToolCallResult, ToolDescriptor, ToolInvocation, ToolResources};

#[async_trait]
pub trait ToolExecutor: Send {
    fn describe(&self, name: &str) -> Option<ToolDescriptor>;
    fn resources(&self, call: &ToolInvocation) -> Result<ToolResources>;
    async fn execute(&self, call: &ToolInvocation) -> Result<ToolCallResult>;
}
