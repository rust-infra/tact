//! Dynamic tool sources and model-facing descriptor snapshots.

use std::sync::Arc;

use async_trait::async_trait;

use crate::capability::tool::Tool;
use crate::tool::descriptor::ToolDescriptor;

/// A plugin-provided set of tools discovered at runtime (MCP today).
#[async_trait]
pub trait ToolSource: Send + Sync {
    fn id(&self) -> &str;
    async fn refresh(&self) -> anyhow::Result<()>;
    fn tools(&self) -> Vec<Arc<dyn Tool>>;
}

/// What the Agent may send to the model: no handler references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptorSnapshot {
    pub generation: u64,
    pub tools: Vec<ToolDescriptor>,
}
