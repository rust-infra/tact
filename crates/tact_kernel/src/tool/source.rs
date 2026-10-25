//! Dynamic tool sources and model-facing descriptor snapshots.

use std::sync::Arc;

use async_trait::async_trait;

use crate::capability::tool::Tool;
use crate::tool::descriptor::ToolDescriptor;

/// A plugin-provided set of tools discovered at runtime (MCP today).
#[async_trait]
pub trait ToolSource: Send + Sync {
    fn id(&self) -> &str;

    /// Re-read the source, reporting whether the tool set changed.
    ///
    /// The boolean is what keeps the generation honest: a source that re-lists
    /// nothing new must not bump it, or every call would look "current" because
    /// the snapshot it compares against was rebuilt on the spot.
    async fn refresh(&self) -> anyhow::Result<bool>;

    fn tools(&self) -> Vec<Arc<dyn Tool>>;

    /// The descriptors this source contributes, without any async work.
    ///
    /// A host that must rebuild the advertised list synchronously (prompt
    /// assembly and spec caching are not async) reads this; the async snapshot
    /// path uses it too, so the two cannot disagree.
    fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.tools()
            .into_iter()
            .map(|tool| tool.descriptor())
            .collect()
    }
}

/// What the Agent may send to the model: no handler references.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDescriptorSnapshot {
    pub generation: u64,
    pub tools: Vec<ToolDescriptor>,
}
