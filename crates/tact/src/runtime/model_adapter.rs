//! Tact's `ModelPort` adapters.
//!
//! [`TactModelPort`] is the provider-backed port. [`StatsModelPort`] wraps any
//! port with the session's `/stats` accounting, because the numbers it reports
//! (prompt/response characters, thinking blocks, per-call duration) only exist
//! where the request and response bodies meet — kernel events carry neither.

use std::{
    sync::{Arc, RwLock},
    time::Instant,
};

use async_trait::async_trait;
use futures_util::StreamExt;
use tact_agent_api::{
    AgentContentBlock, CancelToken, ModelCapabilities, ModelCursor, ModelError, ModelEvent,
    ModelPort, ModelRequest, ModelResponse, ModelStream,
};
use tact_llm::{LlmProvider, ProviderMetadata, ProviderModel};

use super::host_neutral_token_usage;
use crate::stats::SessionStats;

/// One immutable provider activation used by the product kernel.
#[derive(Clone)]
pub(crate) struct ActiveModelSnapshot {
    pub model: Arc<dyn ModelPort>,
    pub metadata: ProviderMetadata,
}

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

/// Wraps a model port with the `/stats` accounting the kernel events cannot do.
///
/// The kernel reports usage through [`AgentEvent::Usage`], but not the request
/// or response text, thinking blocks, or wall-clock time — those live only at
/// the port, which sees both bodies. One wrapper therefore owns every field in
/// `SessionStats` that describes a model call.
///
/// [`AgentEvent::Usage`]: tact_agent_api::AgentEvent::Usage
pub(crate) struct StatsModelPort {
    inner: Arc<dyn ModelPort>,
    stats: Arc<RwLock<SessionStats>>,
}

impl StatsModelPort {
    pub(crate) fn new(inner: Arc<dyn ModelPort>, stats: Arc<RwLock<SessionStats>>) -> Self {
        Self { inner, stats }
    }

    fn record_request(&self, request: &ModelRequest) {
        let chars = serde_json::to_string(request)
            .map(|serialized| serialized.chars().count() as u64)
            .unwrap_or(0);
        let mut stats = self
            .stats
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        stats.prompt_count = stats.prompt_count.saturating_add(1);
        stats.total_prompt_chars = stats.total_prompt_chars.saturating_add(chars);
    }

    fn record_response(&self, started: Instant, response: &ModelResponse) {
        record_response(&self.stats, started, response);
    }

    /// Forward a streaming call, accounting when the stream finishes.
    ///
    /// A stream that errors or is dropped never produced a response, so it
    /// records the request but no duration — the same shape the pre-kernel
    /// loop had, where the counter moved only after a response arrived.
    fn forward_stream(&self, started: Instant, stream: ModelStream) -> ModelStream {
        let stats = self.stats.clone();
        Box::pin(stream.inspect(move |item| {
            if let Ok(ModelEvent::Finished(response)) = item {
                record_response(&stats, started, response);
            }
        }))
    }
}

fn record_response(stats: &RwLock<SessionStats>, started: Instant, response: &ModelResponse) {
    let response_chars = serde_json::to_string(&response.message.content)
        .map(|serialized| serialized.chars().count() as u64)
        .unwrap_or(0);
    let mut stats = stats
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    stats.llm_call_durations.push(started.elapsed());
    stats.total_response_chars = stats.total_response_chars.saturating_add(response_chars);
    for block in &response.message.content {
        if let AgentContentBlock::Thinking { thinking, .. } = block {
            stats.thinking_blocks = stats.thinking_blocks.saturating_add(1);
            stats.total_thinking_chars = stats
                .total_thinking_chars
                .saturating_add(thinking.chars().count() as u64);
        }
    }
    if let Some(usage) = response.usage {
        stats.record_token_usage(&host_neutral_token_usage(&usage));
    }
}

#[async_trait]
impl ModelPort for StatsModelPort {
    fn capabilities(&self) -> ModelCapabilities {
        self.inner.capabilities()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        self.record_request(&request);
        let started = Instant::now();
        let stream = self.inner.stream(request, cursor, cancel).await?;
        Ok(self.forward_stream(started, stream))
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.record_request(&request);
        let started = Instant::now();
        let response = self.inner.complete(request, cursor, cancel).await?;
        self.record_response(started, &response);
        Ok(response)
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.record_request(&request);
        let started = Instant::now();
        let response = self.inner.compact(request, cursor, cancel).await?;
        self.record_response(started, &response);
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use tact_agent_api::ModelCallOptions;
    use tact_llm::{ContentBlock, LlmProvider, MockClient, StopReason as LlmStopReason};

    use super::*;

    fn capabilities() -> ModelCapabilities {
        ModelCapabilities {
            model: "mock".into(),
            context_window: Some(1_000),
            max_output_tokens: 100,
            supports_streaming: true,
            supports_native_compaction: false,
        }
    }

    #[test]
    fn capabilities_are_preserved_through_the_product_wrapper() {
        let client = MockClient::new(Vec::new());
        let expected = capabilities();
        let model = TactModelPort::from_provider(LlmProvider::Mock(client), expected.clone());
        assert_eq!(model.capabilities(), expected);
    }

    fn request() -> ModelRequest {
        ModelRequest {
            model: "mock".into(),
            system: "system".into(),
            messages: Vec::new(),
            tools: Vec::new(),
            max_output_tokens: 100,
            options: ModelCallOptions::default(),
        }
    }

    /// The `/stats` counters that only the model port can see: a completed
    /// call must move the call count, both character totals, the thinking
    /// totals, and the token totals — and exactly once.
    #[tokio::test]
    async fn a_completed_call_records_stats_once() {
        let client = MockClient::with_usage(vec![(
            vec![
                ContentBlock::Thinking {
                    thinking: "plan".into(),
                    signature: "sig".into(),
                },
                ContentBlock::Text {
                    text: "answer".into(),
                },
            ],
            Some(LlmStopReason::EndTurn),
            tact_llm::TokenUsageInfo {
                prompt: 120,
                completion: 30,
                total: 150,
                ..tact_llm::TokenUsageInfo::default()
            },
        )]);
        let stats = Arc::new(RwLock::new(SessionStats::default()));
        let model = StatsModelPort::new(
            Arc::new(TactModelPort::from_provider(
                LlmProvider::Mock(client),
                capabilities(),
            )),
            stats.clone(),
        );
        let response = model
            .complete(request(), None, CancelToken::new())
            .await
            .expect("mock completion");
        assert!(response.usage.is_some(), "the mock must report usage");

        let stats = stats.read().expect("stats lock");
        assert_eq!(stats.prompt_count, 1, "one call, counted once");
        assert!(stats.total_prompt_chars > 0, "the request body is counted");
        assert!(
            stats.total_response_chars > 0,
            "the response body is counted"
        );
        assert_eq!(stats.thinking_blocks, 1);
        assert!(stats.total_thinking_chars > 0);
        assert_eq!(stats.llm_call_durations.len(), 1);
        assert!(stats.total_tokens > 0, "usage reaches the counters");
    }
}
