use async_trait::async_trait;
use tact_agent_api::{UsageError, UsageRecord, UsageSink};

/// Accept usage without retaining it.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoopUsageSink;

#[async_trait]
impl UsageSink for NoopUsageSink {
    async fn record(&self, _record: UsageRecord) -> Result<(), UsageError> {
        Ok(())
    }

    async fn flush(&self) -> Result<(), UsageError> {
        Ok(())
    }
}
