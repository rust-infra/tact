mod context;
mod events;
mod gate;
mod store;
mod usage;

pub use context::BasicContextPolicy;
pub use events::{CallbackEventPort, ContextCallbackEventPort, NoopEventPort};
pub use gate::{AllowAllGate, DenyAllGate};
pub use store::NoopTranscriptStore;
pub use usage::NoopUsageSink;

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, ResolvedToolCall, ToolCall, ToolError, ToolErrorKind, ToolExecutionContext,
    ToolExecutionResult, ToolPort, ToolResolver, ToolSnapshot,
};

/// A zero-sized tool port used when the host provides no tools.
#[derive(Debug, Clone, Copy, Default)]
pub struct EmptyTools;

fn unknown_tool(name: &str) -> ToolError {
    ToolError::new(ToolErrorKind::UnknownTool, format!("unknown tool {name}"))
}

#[async_trait]
impl ToolPort for EmptyTools {
    async fn refresh(&self, _cancel: CancelToken) -> Result<ToolSnapshot, ToolError> {
        Ok(ToolSnapshot {
            specs: Vec::new(),
            generation: 1,
        })
    }

    async fn execute(
        &self,
        call: ResolvedToolCall,
        _context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        Err(unknown_tool(&call.name))
    }
}

impl ToolResolver for EmptyTools {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
        Err(unknown_tool(&call.name))
    }
}
