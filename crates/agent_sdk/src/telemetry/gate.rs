use std::sync::Arc;

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, GateDecision, ResolvedToolCall, ToolCall, ToolExecutionResult, ToolGate,
    ToolGateContext, ToolGateError, ToolResolver,
};
use tracing::info_span;

pub(crate) struct InstrumentedToolGate {
    inner: Arc<dyn ToolGate>,
}

impl InstrumentedToolGate {
    pub(crate) fn new(inner: Arc<dyn ToolGate>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolGate for InstrumentedToolGate {
    async fn before(
        &self,
        call: ToolCall,
        resolver: &dyn ToolResolver,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<GateDecision, ToolGateError> {
        let span = info_span!(
            "agent.gate.before",
            tool = %call.name,
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.before(call, resolver, context, cancel)).await
    }

    async fn after(
        &self,
        call: &ResolvedToolCall,
        result: ToolExecutionResult,
        context: &ToolGateContext,
        cancel: CancelToken,
    ) -> Result<ToolExecutionResult, ToolGateError> {
        let span = info_span!(
            "agent.gate.after",
            tool = %call.name,
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.after(call, result, context, cancel)).await
    }
}
