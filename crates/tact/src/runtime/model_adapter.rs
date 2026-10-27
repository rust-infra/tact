//! Tact's `ModelPort` adapter.

use std::{
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
};

use async_trait::async_trait;
use futures_util::Stream;
use tact_agent_api::{
    CancelToken, ModelCapabilities, ModelCursor, ModelError, ModelErrorKind, ModelEvent, ModelPort,
    ModelRequest, ModelResponse, ModelStream,
};
use tact_llm::{LlmProvider, ProviderModel};

/// Product wrapper around the provider-neutral `tact_llm` model implementation.
pub(crate) struct TactModelPort {
    inner: ProviderModel,
    cancel_flag: Option<Arc<AtomicBool>>,
}

struct CancelAwareStream {
    inner: ModelStream,
    cancel_flag: Arc<AtomicBool>,
    cancel: CancelToken,
    emitted_cancel: bool,
}

impl Stream for CancelAwareStream {
    type Item = Result<ModelEvent, ModelError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if self.emitted_cancel {
            return Poll::Ready(None);
        }
        let this = self.get_mut();
        let polled = Pin::new(&mut this.inner).poll_next(cx);
        let cancelled = this.cancel_flag.load(Ordering::SeqCst);
        if cancelled {
            this.cancel.cancel();
        }
        if !polled.is_pending() {
            return polled;
        }
        if cancelled {
            this.emitted_cancel = true;
            return Poll::Ready(Some(Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model call cancelled",
            ))));
        }
        polled
    }
}

impl TactModelPort {
    pub(crate) fn from_provider(provider: LlmProvider, capabilities: ModelCapabilities) -> Self {
        Self {
            inner: ProviderModel::from_provider(provider, capabilities),
            cancel_flag: None,
        }
    }

    pub(crate) fn with_cancel_flag(mut self, cancel_flag: Arc<AtomicBool>) -> Self {
        self.cancel_flag = Some(cancel_flag);
        self
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
        let stream = self.inner.stream(request, cursor, cancel.clone()).await?;
        Ok(match &self.cancel_flag {
            Some(flag) => Box::pin(CancelAwareStream {
                inner: stream,
                cancel_flag: flag.clone(),
                cancel,
                emitted_cancel: false,
            }),
            None => stream,
        })
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.inner.complete(request, cursor, cancel).await?;
        if self
            .cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            return Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model call cancelled",
            ));
        }
        Ok(response)
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let response = self.inner.compact(request, cursor, cancel).await?;
        if self
            .cancel_flag
            .as_ref()
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
        {
            return Err(ModelError::new(
                ModelErrorKind::Cancelled,
                "model call cancelled",
            ));
        }
        Ok(response)
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
