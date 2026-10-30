use std::{
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
    time::Instant,
};

use async_trait::async_trait;
use futures_util::Stream;
use tact_agent_api::{
    CancelToken, ModelCapabilities, ModelCursor, ModelError, ModelEvent, ModelPort, ModelRequest,
    ModelResponse, ModelStream, RunContext,
};
use tracing::{Instrument, info_span};

use super::TelemetryConfig;

pub(crate) struct InstrumentedModelPort {
    inner: Arc<dyn ModelPort>,
    config: TelemetryConfig,
}

impl InstrumentedModelPort {
    pub(crate) fn new(inner: Arc<dyn ModelPort>, config: TelemetryConfig) -> Self {
        Self { inner, config }
    }

    async fn stream_with(
        &self,
        context: Option<&RunContext>,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let model = request.model.clone();
        let span = info_span!(
            "agent.model.call",
            model = %model,
            kind = "stream",
            duration_ms = tracing::field::Empty,
            time_to_first_event_ms = tracing::field::Empty,
            input_tokens = tracing::field::Empty,
            output_tokens = tracing::field::Empty,
            total_tokens = tracing::field::Empty,
            "error.kind" = tracing::field::Empty
        );
        let started = Instant::now();
        let result = match context {
            Some(context) => {
                self.inner
                    .stream_with_context(context, request, cursor, cancel)
                    .instrument(span.clone())
                    .await
            }
            None => self.inner.stream(request, cursor, cancel).await,
        };
        match result {
            Ok(stream) => Ok(Box::pin(InstrumentedModelStream::new(
                stream,
                span,
                started,
                self.config.clone(),
            ))),
            Err(error) => {
                span.record("error.kind", tracing::field::debug(&error.kind));
                span.record("duration_ms", elapsed_ms(started));
                tracing::warn!(
                    target: "tact_agent_sdk",
                    metric = "agent.model.setup.error",
                    model = %model,
                    error_kind = ?error.kind,
                    "agent model stream setup failed"
                );
                Err(error)
            }
        }
    }

    async fn complete_with(
        &self,
        context: Option<&RunContext>,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let model = request.model.clone();
        let span = info_span!(
            "agent.model.call",
            model = %model,
            kind = "complete",
            duration_ms = tracing::field::Empty,
            input_tokens = tracing::field::Empty,
            output_tokens = tracing::field::Empty,
            total_tokens = tracing::field::Empty,
            "error.kind" = tracing::field::Empty
        );
        let started = Instant::now();
        let result = match context {
            Some(context) => {
                self.inner
                    .complete_with_context(context, request, cursor, cancel)
                    .instrument(span.clone())
                    .await
            }
            None => self.inner.complete(request, cursor, cancel).await,
        };
        match &result {
            Ok(response) => record_usage(&span, response),
            Err(error) => {
                span.record("error.kind", tracing::field::debug(&error.kind));
            }
        }
        span.record("duration_ms", elapsed_ms(started));
        result
    }

    async fn compact_with(
        &self,
        context: Option<&RunContext>,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        let span = info_span!(
            "agent.model.compact",
            model = %request.model,
            duration_ms = tracing::field::Empty,
            input_tokens = tracing::field::Empty,
            output_tokens = tracing::field::Empty,
            total_tokens = tracing::field::Empty,
            "error.kind" = tracing::field::Empty
        );
        let started = Instant::now();
        let result = match context {
            Some(context) => {
                self.inner
                    .compact_with_context(context, request, cursor, cancel)
                    .instrument(span.clone())
                    .await
            }
            None => self.inner.compact(request, cursor, cancel).await,
        };
        match &result {
            Ok(response) => record_usage(&span, response),
            Err(error) => {
                span.record("error.kind", tracing::field::debug(&error.kind));
            }
        }
        span.record("duration_ms", elapsed_ms(started));
        result
    }
}

#[async_trait]
impl ModelPort for InstrumentedModelPort {
    fn capabilities(&self) -> ModelCapabilities {
        self.inner.capabilities()
    }

    async fn stream(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        self.stream_with(None, request, cursor, cancel).await
    }

    async fn stream_with_context(
        &self,
        context: &RunContext,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        self.stream_with(Some(context), request, cursor, cancel)
            .await
    }

    async fn complete(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_with(None, request, cursor, cancel).await
    }

    async fn complete_with_context(
        &self,
        context: &RunContext,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.complete_with(Some(context), request, cursor, cancel)
            .await
    }

    async fn compact(
        &self,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.compact_with(None, request, cursor, cancel).await
    }

    async fn compact_with_context(
        &self,
        context: &RunContext,
        request: ModelRequest,
        cursor: Option<&ModelCursor>,
        cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        self.compact_with(Some(context), request, cursor, cancel)
            .await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamPhase {
    AwaitingFirst,
    Streaming,
    Terminal,
    Errored,
}

struct InstrumentedModelStream {
    inner: ModelStream,
    span: tracing::Span,
    started: Instant,
    phase: StreamPhase,
    config: TelemetryConfig,
}

impl InstrumentedModelStream {
    fn new(
        inner: ModelStream,
        span: tracing::Span,
        started: Instant,
        config: TelemetryConfig,
    ) -> Self {
        Self {
            inner,
            span,
            started,
            phase: StreamPhase::AwaitingFirst,
            config,
        }
    }

    fn observe(&mut self, item: &Result<ModelEvent, ModelError>) {
        if matches!(self.phase, StreamPhase::Terminal | StreamPhase::Errored) {
            return;
        }
        match item {
            Ok(event) => {
                if self.phase == StreamPhase::AwaitingFirst {
                    self.phase = StreamPhase::Streaming;
                    let elapsed = self.started.elapsed().as_millis();
                    self.span.record("time_to_first_event_ms", elapsed);
                    self.span.in_scope(|| {
                        tracing::debug!(
                            target: "tact_agent_sdk",
                            metric = "agent.model.time_to_first_event_ms",
                            time_to_first_event_ms = elapsed,
                            "agent model first event"
                        );
                    });
                }
                if let ModelEvent::Finished(response) = event {
                    self.phase = StreamPhase::Terminal;
                    record_usage(&self.span, response);
                    if self.config.record_content_enabled() {
                        super::debug_content(
                            "message",
                            "assistant_text",
                            &response.message.text_content(),
                        );
                    }
                    let elapsed = self.started.elapsed().as_millis();
                    self.span.record("duration_ms", elapsed);
                    let usage = response.usage.unwrap_or_default();
                    self.span.in_scope(|| {
                        tracing::debug!(
                            target: "tact_agent_sdk",
                            metric = "agent.model.finished",
                            duration_ms = elapsed,
                            input_tokens = usage.input_tokens,
                            output_tokens = usage.output_tokens,
                            total_tokens = usage.total_tokens,
                            "agent model finished"
                        );
                    });
                }
            }
            Err(error) => {
                self.phase = StreamPhase::Errored;
                self.span
                    .record("error.kind", tracing::field::debug(&error.kind));
                let elapsed = self.started.elapsed().as_millis();
                self.span.record("duration_ms", elapsed);
                self.span.in_scope(|| {
                    tracing::warn!(
                        target: "tact_agent_sdk",
                        metric = "agent.model.stream.error",
                        duration_ms = elapsed,
                        error_kind = ?error.kind,
                        "agent model stream failed"
                    );
                });
            }
        }
    }
}

impl Stream for InstrumentedModelStream {
    type Item = Result<ModelEvent, ModelError>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.as_mut().get_mut();
        let poll = {
            let _entered = this.span.enter();
            this.inner.as_mut().poll_next(cx)
        };
        match poll {
            Poll::Ready(Some(item)) => {
                this.observe(&item);
                Poll::Ready(Some(item))
            }
            Poll::Ready(None) => {
                if matches!(
                    this.phase,
                    StreamPhase::AwaitingFirst | StreamPhase::Streaming
                ) {
                    this.phase = StreamPhase::Errored;
                    let elapsed = this.started.elapsed().as_millis();
                    this.span.record("duration_ms", elapsed);
                    this.span.in_scope(|| {
                        tracing::debug!(
                            target: "tact_agent_sdk",
                            metric = "agent.model.ended_without_terminal",
                            duration_ms = elapsed,
                            "agent model stream ended"
                        );
                    });
                }
                Poll::Ready(None)
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

fn record_usage(span: &tracing::Span, response: &ModelResponse) {
    if let Some(usage) = response.usage {
        span.record("input_tokens", usage.input_tokens);
        span.record("output_tokens", usage.output_tokens);
        span.record("total_tokens", usage.total_tokens);
    }
}

fn elapsed_ms(started: Instant) -> u128 {
    started.elapsed().as_millis()
}
