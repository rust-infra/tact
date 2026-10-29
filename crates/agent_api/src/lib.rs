//! Provider-neutral contracts for one bounded Tact agent activation.
//!
//! This crate intentionally contains no provider, storage, MCP, UI, or
//! filesystem implementation. Product crates implement these capability traits
//! and inject them into the kernel.

mod event;
mod extension;
mod message;
mod model;
mod state;
mod tool;
mod usage;

mod context;
mod store;

#[cfg(any(test, feature = "test-support"))]
pub mod test_support;

pub use context::{
    CompactionKind, CompactionOutcome, CompactionRequest, ContextError, ContextPolicy,
    ContextPreparation, ContextReplacement, ContextView, PreparationDecision, PreparedRequest,
    RequestPlan,
};
pub use event::{AgentEvent, AgentNotice, EventDelivery};
pub use extension::{EventObserver, EventPort};
pub use message::{
    AgentContentBlock, AgentImageSource, AgentMessage, AgentRole, MessageId, MessageOrigin,
};
pub use model::{
    MessageDelta, ModelCallKind, ModelCallOptions, ModelCapabilities, ModelCapabilitiesError,
    ModelCursor, ModelError, ModelErrorKind, ModelEvent, ModelMessage, ModelPort, ModelRequest,
    ModelResponse, ModelStream, ProviderActivity, ProviderActivityKind, ProviderActivityPhase,
    ReasoningEffort, StopReason, ThinkingDelta, TokenUsage,
};
pub use state::{
    AgentState, CancelToken, HostYieldReason, MessageRange, RecoveryStats, RunId, RunInput,
    RunOutcome, RunPhase, RunStop, RuntimeErrorInfo, SessionRef,
};
pub use store::{
    CursorUpdate, MessageReceipt, ReplaceReceipt, StoreError, TranscriptSnapshot, TranscriptStore,
};
pub use tool::{
    GateDecision, ResolvedToolCall, ResourceAccess, ResourceKey, ResourceScope, ToolCall,
    ToolDescriptor, ToolDirective, ToolError, ToolErrorKind, ToolExecutionContext,
    ToolExecutionError, ToolExecutionErrorKind, ToolExecutionResult, ToolExecutionStatus, ToolGate,
    ToolGateContext, ToolGateError, ToolOutcome, ToolOutputChunk, ToolOutputStream, ToolPort,
    ToolProgressSink, ToolResolver, ToolSnapshot, ToolSpec,
};
pub use usage::{UsageError, UsageRecord, UsageSink};

#[cfg(test)]
mod object_safety {
    use std::sync::Arc;

    use crate::{
        ContextPolicy, EventObserver, EventPort, ModelPort, ToolGate, ToolPort, ToolResolver,
        TranscriptStore, UsageSink,
    };

    fn model(_: Arc<dyn ModelPort>) {}
    fn resolver(_: &dyn ToolResolver) {}
    fn tools(_: Arc<dyn ToolPort>) {}
    fn gate(_: Arc<dyn ToolGate>) {}
    fn context(_: Arc<dyn ContextPolicy>) {}
    fn transcript(_: Arc<dyn TranscriptStore>) {}
    fn usage(_: Arc<dyn UsageSink>) {}
    fn events(_: Arc<dyn EventPort>) {}
    fn observers(_: Arc<dyn EventObserver>) {}

    #[test]
    fn every_required_capability_is_object_safe() {
        let _ = (
            model, resolver, tools, gate, context, transcript, usage, events, observers,
        );
    }
}
