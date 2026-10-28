use std::sync::Arc;

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, CompactionOutcome, CompactionRequest, ContextError, ContextPolicy,
    ContextPreparation, ContextView, ModelPort, PreparationDecision, RequestPlan, RunInput,
};
use tracing::info_span;

pub(crate) struct InstrumentedContextPolicy {
    inner: Arc<dyn ContextPolicy>,
    config: super::TelemetryConfig,
}

impl InstrumentedContextPolicy {
    pub(crate) fn new(inner: Arc<dyn ContextPolicy>, config: super::TelemetryConfig) -> Self {
        Self { inner, config }
    }
}

#[async_trait]
impl ContextPolicy for InstrumentedContextPolicy {
    async fn prepare_run(
        &self,
        input: &RunInput,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError> {
        let span = info_span!(
            "agent.context.prepare_run",
            run_id = %input.run_id,
            duration_ms = tracing::field::Empty
        );
        if self.config.record_content_enabled() {
            for message in &input.inputs {
                super::debug_content("message", "input", &message.text_content());
            }
        }
        super::timed(span, self.inner.prepare_run(input, state, model, cancel)).await
    }

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError> {
        let span = info_span!(
            "agent.context.prepare_request",
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.prepare_request(state, model, cancel)).await
    }

    async fn compact(
        &self,
        state: &ContextView<'_>,
        request: CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<CompactionOutcome, ContextError> {
        let span = info_span!("agent.context.compact", duration_ms = tracing::field::Empty);
        super::timed(span, self.inner.compact(state, request, model, cancel)).await
    }

    async fn compaction_committed(
        &self,
        state: &ContextView<'_>,
        request: &CompactionRequest,
    ) -> Result<(), ContextError> {
        let span = info_span!(
            "agent.context.compaction_committed",
            duration_ms = tracing::field::Empty
        );
        super::timed(span, self.inner.compaction_committed(state, request)).await
    }
}
