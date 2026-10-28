/// Telemetry configuration retained when the `telemetry` feature is disabled.
#[derive(Debug, Clone)]
pub struct TelemetryConfig {
    #[allow(dead_code)]
    record_content: bool,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self::enabled()
    }
}

impl TelemetryConfig {
    pub fn enabled() -> Self {
        Self {
            record_content: false,
        }
    }

    pub fn disabled() -> Self {
        Self {
            record_content: false,
        }
    }

    pub fn record_content(self, enabled: bool) -> Self {
        Self {
            record_content: enabled,
        }
    }

    #[allow(dead_code)]
    pub(crate) fn is_enabled(&self) -> bool {
        false
    }

    #[allow(dead_code)]
    pub(crate) fn record_content_enabled(&self) -> bool {
        self.record_content
    }
}

#[cfg(test)]
mod tests {
    use super::TelemetryConfig;
    use crate::AgentBuilder;
    use std::sync::Arc;
    use tact_agent_api::{
        CancelToken, ModelCapabilities, ModelCursor, ModelError, ModelPort, ModelRequest,
        ModelResponse, ModelStream,
    };

    struct NoopModel;

    #[async_trait::async_trait]
    impl ModelPort for NoopModel {
        fn capabilities(&self) -> ModelCapabilities {
            ModelCapabilities {
                model: "noop".into(),
                context_window: 1,
                max_output_tokens: 1,
                supports_streaming: true,
                supports_native_compaction: false,
            }
        }

        async fn stream(
            &self,
            _request: ModelRequest,
            _cursor: Option<&ModelCursor>,
            _cancel: CancelToken,
        ) -> Result<ModelStream, ModelError> {
            Err(ModelError::unsupported("stream"))
        }

        async fn complete(
            &self,
            _request: ModelRequest,
            _cursor: Option<&ModelCursor>,
            _cancel: CancelToken,
        ) -> Result<ModelResponse, ModelError> {
            Err(ModelError::unsupported("complete"))
        }
    }

    #[test]
    fn telemetry_is_optional() {
        let config = TelemetryConfig::default().record_content(true);
        assert!(!config.is_enabled());
        assert!(config.record_content_enabled());
        assert!(
            AgentBuilder::new(Arc::new(NoopModel))
                .telemetry(config)
                .build()
                .is_ok()
        );
    }
}
