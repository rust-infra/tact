use std::{sync::Arc, time::Instant};

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, ResolvedToolCall, ToolCall, ToolError, ToolExecutionContext, ToolExecutionResult,
    ToolPort, ToolResolver, ToolSnapshot,
};
use tracing::{Instrument, info_span};

pub(crate) struct InstrumentedToolPort {
    inner: Arc<dyn ToolPort>,
}

impl InstrumentedToolPort {
    pub(crate) fn new(inner: Arc<dyn ToolPort>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ToolPort for InstrumentedToolPort {
    async fn refresh(&self, cancel: CancelToken) -> Result<ToolSnapshot, ToolError> {
        let span = info_span!("agent.tool.refresh", duration_ms = tracing::field::Empty);
        let started = Instant::now();
        let result = self.inner.refresh(cancel).instrument(span.clone()).await;
        let elapsed = started.elapsed().as_millis();
        span.record("duration_ms", elapsed);
        span.in_scope(|| {
            tracing::debug!(
                target: "tact_agent_sdk",
                metric = "agent.tool.refresh",
                duration_ms = elapsed,
                "agent tool snapshot refreshed"
            );
        });
        result
    }

    async fn execute(
        &self,
        call: ResolvedToolCall,
        context: ToolExecutionContext,
    ) -> Result<ToolExecutionResult, ToolError> {
        let span = info_span!(
            "agent.tool.execute",
            tool = %call.name,
            call_id = %call.call_id,
            duration_ms = tracing::field::Empty
        );
        let started = Instant::now();
        let result = self
            .inner
            .execute(call, context)
            .instrument(span.clone())
            .await;
        let elapsed = started.elapsed().as_millis();
        span.record("duration_ms", elapsed);
        span.in_scope(|| {
            tracing::debug!(
                target: "tact_agent_sdk",
                metric = "agent.tool.execute",
                duration_ms = elapsed,
                "agent tool execution finished"
            );
        });
        result
    }
}

impl ToolResolver for InstrumentedToolPort {
    fn resolve(&self, call: &ToolCall) -> Result<ResolvedToolCall, ToolError> {
        self.inner.resolve(call)
    }
}
