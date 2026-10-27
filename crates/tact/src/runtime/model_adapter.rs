//! Tact's `ModelPort` adapter.

use async_trait::async_trait;
use tact_agent_api::{
    CancelToken, ModelCapabilities, ModelCursor, ModelError, ModelPort, ModelRequest,
    ModelResponse, ModelStream,
};
use tact_llm::{LlmProvider, ProviderModel};

/// Product wrapper around the provider-neutral `tact_llm` model implementation.
pub(crate) struct TactModelPort {
    inner: ProviderModel,
}

impl TactModelPort {
    pub(crate) fn from_provider(provider: LlmProvider, capabilities: ModelCapabilities) -> Self {
        Self {
            inner: ProviderModel::from_provider(provider, capabilities),
        }
    }
}

#[async_trait]
impl ModelPort for TactModelPort {
    fn capabilities(&self) -> ModelCapabilities {
        self.inner.capabilities()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        self.inner.stream(request, cursor, cancel).await
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.inner.complete(request, cursor, cancel).await
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.inner.compact(request, cursor, cancel).await
    }
}

#[cfg(test)]
mod tests {
    use tact_llm::{LlmProvider, MockClient};

    use super::*;

    #[test]
    fn capabilities_are_preserved_through_the_product_wrapper() {
        let client = MockClient::new(Vec::new());
        let expected = ModelCapabilities {
            model: "mock".into(),
            context_window: 1_000,
            max_output_tokens: 100,
            supports_streaming: true,
            supports_native_compaction: false,
        };
        let model = TactModelPort::from_provider(LlmProvider::Mock(client), expected.clone());
        assert_eq!(model.capabilities(), expected);
    }
}
