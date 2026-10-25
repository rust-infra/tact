use async_trait::async_trait;
use serde_json::Value;

use crate::tool::context::ToolCallContext;
use crate::tool::descriptor::ToolDescriptor;
use crate::tool::result::ToolCallResult;

#[async_trait]
pub trait Tool: Send + Sync {
    fn descriptor(&self) -> ToolDescriptor;

    async fn call(&self, ctx: ToolCallContext, input: Value) -> anyhow::Result<ToolCallResult>;
}
