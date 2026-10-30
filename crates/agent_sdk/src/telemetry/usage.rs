use std::sync::Arc;

use async_trait::async_trait;
use tact_agent_api::{UsageError, UsageRecord, UsageSink};
use tracing::info_span;

pub(crate) struct InstrumentedUsageSink {
    inner: Arc<dyn UsageSink>,
}

impl InstrumentedUsageSink {
    pub(crate) fn new(inner: Arc<dyn UsageSink>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl UsageSink for InstrumentedUsageSink {
    async fn record(&self, record: UsageRecord) -> Result<(), UsageError> {
        let span = info_span!(
            "agent.usage.record",
            call_id = %record.call_id,
            duration_ms = tracing::field::Empty,
            input_tokens = tracing::field::Empty,
            output_tokens = tracing::field::Empty,
            total_tokens = tracing::field::Empty
        );
        if let Some(usage) = record.usage {
            span.record("input_tokens", usage.input_tokens);
            span.record("output_tokens", usage.output_tokens);
            span.record("total_tokens", usage.total_tokens);
        }
        super::timed(span, self.inner.record(record)).await
    }

    async fn flush(&self) -> Result<(), UsageError> {
        let span = info_span!("agent.usage.flush", duration_ms = tracing::field::Empty);
        super::timed(span, self.inner.flush()).await
    }
}
