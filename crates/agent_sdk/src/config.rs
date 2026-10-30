use std::sync::Arc;

use crate::{RunIdSource, TelemetryConfig, UuidRunIdSource};

#[derive(Clone)]
pub(crate) struct AgentConfig {
    pub(crate) default_max_total_turns: Option<u32>,
    pub(crate) run_id_source: Arc<dyn RunIdSource>,
    #[cfg(feature = "telemetry")]
    #[allow(dead_code)]
    pub(crate) telemetry: TelemetryConfig,
}

impl AgentConfig {
    pub(crate) fn new(
        default_max_total_turns: Option<u32>,
        run_id_source: Option<Arc<dyn RunIdSource>>,
        #[cfg(feature = "telemetry")] telemetry: TelemetryConfig,
        #[cfg(not(feature = "telemetry"))] _telemetry: TelemetryConfig,
    ) -> Self {
        Self {
            default_max_total_turns,
            run_id_source: run_id_source.unwrap_or_else(|| Arc::new(UuidRunIdSource)),
            #[cfg(feature = "telemetry")]
            telemetry,
        }
    }
}
