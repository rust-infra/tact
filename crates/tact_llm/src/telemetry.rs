//! Body-free tracing for stable provider calls.

use tact_agent_api::{ModelError, ModelEvent, ModelResponse};

use crate::ProviderMetadata;

#[cfg(feature = "telemetry")]
use std::future::Future;
#[cfg(feature = "telemetry")]
use std::time::Instant;
#[cfg(feature = "telemetry")]
use tracing::Instrument;

#[cfg(feature = "telemetry")]
#[derive(Clone)]
pub(crate) struct SpanHandle(tracing::Span);

#[cfg(not(feature = "telemetry"))]
#[derive(Clone, Copy, Default)]
pub(crate) struct SpanHandle;

#[cfg(feature = "telemetry")]
impl Default for SpanHandle {
    fn default() -> Self {
        Self(tracing::Span::none())
    }
}

impl SpanHandle {
    pub(crate) fn none() -> Self {
        #[cfg(feature = "telemetry")]
        {
            Self::default()
        }
        #[cfg(not(feature = "telemetry"))]
        {
            Self
        }
    }
}

pub(crate) fn clone_handle(handle: &SpanHandle) -> SpanHandle {
    #[cfg(feature = "telemetry")]
    {
        handle.clone()
    }
    #[cfg(not(feature = "telemetry"))]
    {
        *handle
    }
}

#[cfg(feature = "telemetry")]
pub(crate) struct CallSpan {
    span: tracing::Span,
    started: Instant,
    first_event: bool,
}

#[cfg(not(feature = "telemetry"))]
pub(crate) struct CallSpan;

#[cfg(feature = "telemetry")]
pub(crate) fn call_span(kind: &'static str, metadata: &ProviderMetadata) -> CallSpan {
    let endpoint_host = url::Url::parse(metadata.endpoint().as_str())
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .unwrap_or_default();
    CallSpan {
        span: tracing::info_span!(
            "tact_llm.call",
            request_kind = kind,
            provider = %metadata.provider(),
            protocol = metadata.protocol().as_str(),
            model = metadata.model(),
            endpoint_host = %endpoint_host
        ),
        started: Instant::now(),
        first_event: false,
    }
}

#[cfg(not(feature = "telemetry"))]
pub(crate) fn call_span(_kind: &'static str, _metadata: &ProviderMetadata) -> CallSpan {
    CallSpan
}

impl CallSpan {
    pub(crate) fn handle(&self) -> SpanHandle {
        #[cfg(feature = "telemetry")]
        {
            SpanHandle(self.span.clone())
        }
        #[cfg(not(feature = "telemetry"))]
        {
            SpanHandle
        }
    }
}

#[cfg(feature = "telemetry")]
pub(crate) fn instrument<F>(handle: SpanHandle, future: F) -> tracing::instrument::Instrumented<F>
where
    F: Future,
{
    future.instrument(handle.0)
}

#[cfg(not(feature = "telemetry"))]
pub(crate) fn instrument<F>(_handle: SpanHandle, future: F) -> F {
    future
}

pub(crate) fn record_event(span: &mut CallSpan, event: &Result<ModelEvent, ModelError>) {
    #[cfg(feature = "telemetry")]
    {
        if !span.first_event {
            span.first_event = true;
            tracing::event!(
                parent: &span.span,
                tracing::Level::INFO,
                time_to_first_event_ms = elapsed_ms(span.started),
                "stream first event"
            );
        }
        match event {
            Ok(ModelEvent::Usage { usage }) => record_usage(&span.span, *usage),
            Ok(ModelEvent::Finished(response)) => {
                record_response(&span.span, span.started, response)
            }
            Err(error) => record_error(&span.span, error),
            _ => {}
        }
    }
    #[cfg(not(feature = "telemetry"))]
    {
        let _ = (span, event);
    }
}

pub(crate) fn record_result(span: &CallSpan, result: &Result<ModelResponse, ModelError>) {
    #[cfg(feature = "telemetry")]
    {
        match result {
            Ok(response) => record_response(&span.span, span.started, response),
            Err(error) => record_error(&span.span, error),
        }
    }
    #[cfg(not(feature = "telemetry"))]
    {
        let _ = (span, result);
    }
}

#[cfg(feature = "telemetry")]
fn record_response(span: &tracing::Span, started: Instant, response: &ModelResponse) {
    if let Some(usage) = response.usage {
        record_usage(span, usage);
    }
    tracing::event!(
        parent: span,
        tracing::Level::INFO,
        outcome = "finished",
        total_elapsed_ms = elapsed_ms(started),
        "model call finished"
    );
}

#[cfg(feature = "telemetry")]
fn record_usage(span: &tracing::Span, usage: tact_agent_api::TokenUsage) {
    tracing::event!(
        parent: span,
        tracing::Level::INFO,
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        total_tokens = usage.total_tokens,
        cache_read_input_tokens = usage.cache_read_input_tokens,
        cache_miss_input_tokens = usage.cache_miss_input_tokens,
        reasoning_tokens = usage.reasoning_tokens,
        "model usage"
    );
}

#[cfg(feature = "telemetry")]
fn record_error(span: &tracing::Span, error: &ModelError) {
    tracing::event!(
        parent: span,
        tracing::Level::WARN,
        error_kind = ?error.kind,
        retry_after_ms = error.retry_after.map(|delay| delay.as_millis() as u64),
        cancelled = error.kind == tact_agent_api::ModelErrorKind::Cancelled,
        outcome = "failed",
        "model call failed"
    );
}

#[cfg(feature = "telemetry")]
fn elapsed_ms(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Test subscriber capture shared with telemetry tests in other modules.
#[cfg(all(test, feature = "telemetry"))]
pub(crate) mod testing {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
    };

    use tracing_subscriber::{Layer, filter::LevelFilter, fmt::MakeWriter, layer::SubscriberExt};

    #[derive(Clone, Default)]
    pub(crate) struct Capture(Arc<Mutex<CaptureBuffer>>);

    #[derive(Default)]
    struct CaptureBuffer {
        bytes: Vec<u8>,
        call_span_ids: Vec<u64>,
    }

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
                .bytes
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Capture {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    impl<S: tracing::Subscriber> Layer<S> for Capture {
        fn on_new_span(
            &self,
            attributes: &tracing::span::Attributes<'_>,
            id: &tracing::span::Id,
            _context: tracing_subscriber::layer::Context<'_, S>,
        ) {
            if attributes.metadata().name() == "tact_llm.call" {
                self.0
                    .lock()
                    .expect("capture lock")
                    .call_span_ids
                    .push(id.into_u64());
            }
        }
    }

    impl Capture {
        /// Install a thread-local subscriber and return the capture handle.
        ///
        /// The guard must stay alive for the duration of the asserted work.
        pub(crate) fn install(
            max_level: tracing::Level,
        ) -> (Self, tracing::subscriber::DefaultGuard) {
            let capture = Self::default();
            let subscriber = tracing_subscriber::registry()
                .with(
                    capture
                        .clone()
                        .with_filter(LevelFilter::from_level(max_level)),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(capture.clone())
                        .with_ansi(false)
                        .without_time()
                        .with_filter(LevelFilter::from_level(max_level)),
                );
            let guard = tracing::subscriber::set_default(subscriber);
            (capture, guard)
        }

        pub(crate) fn text(&self) -> String {
            let buffer = self.0.lock().expect("capture lock");
            String::from_utf8(buffer.bytes.clone()).expect("captured tracing output is utf-8")
        }

        /// Span ids of every `tact_llm.call` span created while capturing.
        pub(crate) fn call_span_ids(&self) -> Vec<u64> {
            self.0.lock().expect("capture lock").call_span_ids.clone()
        }
    }
}

#[cfg(all(test, feature = "telemetry"))]
mod tests {
    use std::time::Duration;

    use tact_agent_api::{
        AgentMessage, AgentRole, CursorUpdate, MessageId, MessageOrigin, ModelCapabilities,
        ModelErrorKind, StopReason, TokenUsage,
    };

    use super::*;
    use crate::telemetry::testing::Capture;
    use crate::{Endpoint, ProviderKind, WireProtocol};

    #[test]
    fn tracing_records_safe_call_fields_and_redacts_payloads() {
        let (capture, _guard) = Capture::install(tracing::Level::TRACE);

        let metadata = ProviderMetadata::new(
            ProviderKind::OpenAi,
            WireProtocol::OpenAiResponses,
            "gpt-test".to_string(),
            Endpoint::parse("https://api.openai.com/v1").unwrap(),
            ModelCapabilities::new("gpt-test", Some(10_000), 1_000, true, true).unwrap(),
            true,
        );
        let mut span = call_span("stream", &metadata);
        let usage = TokenUsage {
            input_tokens: 7,
            output_tokens: 3,
            total_tokens: 10,
            cache_read_input_tokens: 2,
            cache_miss_input_tokens: 5,
            reasoning_tokens: 1,
        };
        record_event(&mut span, &Ok(ModelEvent::Usage { usage }));
        let response = ModelResponse {
            message_id: MessageId::new("message-1"),
            message: AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "secret"),
            stop_reason: StopReason::EndTurn,
            usage: Some(usage),
            request_body: Some(b"SECRET-REQUEST-BODY".to_vec()),
            cursor: CursorUpdate::Unchanged,
        };
        record_result(&span, &Ok(response));
        record_result(
            &span,
            &Err(ModelError {
                kind: ModelErrorKind::Transient,
                message: "SECRET-ERROR-BODY".into(),
                retry_after: Some(Duration::from_secs(2)),
            }),
        );

        let output = capture.text();
        assert!(output.contains("tact_llm.call"), "{output}");
        assert!(output.contains("provider=openai"), "{output}");
        assert!(output.contains("protocol=\"openai_responses\""), "{output}");
        assert!(output.contains("model=\"gpt-test\""), "{output}");
        assert!(output.contains("endpoint_host=api.openai.com"), "{output}");
        assert!(output.contains("time_to_first_event_ms="), "{output}");
        assert!(output.contains("input_tokens=7"), "{output}");
        assert!(output.contains("outcome=\"finished\""), "{output}");
        assert!(output.contains("error_kind=Transient"), "{output}");
        assert!(output.contains("retry_after_ms=2000"), "{output}");
        assert!(!output.contains("SECRET-REQUEST-BODY"), "{output}");
        assert!(!output.contains("SECRET-ERROR-BODY"), "{output}");
        assert!(!output.contains("secret"), "{output}");
    }

    // `set_default` only covers this thread, so the task must stay on it.
    #[tokio::test(flavor = "current_thread")]
    async fn instrument_wraps_spawned_tasks_in_the_call_span() {
        let (capture, _guard) = Capture::install(tracing::Level::TRACE);

        let metadata = ProviderMetadata::new(
            ProviderKind::OpenAi,
            WireProtocol::OpenAiResponses,
            "gpt-test".to_string(),
            Endpoint::parse("https://api.openai.com/v1").unwrap(),
            ModelCapabilities::new("gpt-test", Some(10_000), 1_000, true, true).unwrap(),
            true,
        );
        let span = call_span("complete", &metadata);
        tokio::spawn(instrument(span.handle(), async {
            tracing::info!("inside instrumented task");
        }))
        .await
        .expect("instrumented task joins");

        let output = capture.text();
        assert!(
            output.lines().any(|line| line.contains("tact_llm.call{")
                && line.contains("request_kind=\"complete\"")
                && line.contains("inside instrumented task")),
            "spanned line missing from:\n{output}"
        );
    }

    /// Pins the crate's side of the contract: no feature path may enable the
    /// consumer's `tokio/tracing`. Parses the feature graph instead of
    /// grepping one line, so multi-line arrays and intermediate features
    /// cannot smuggle the edge back in.
    #[test]
    fn no_feature_enables_tokio_tracing() {
        let manifest: toml::Value = include_str!("../Cargo.toml")
            .parse()
            .expect("parse manifest");
        let features = manifest
            .get("features")
            .and_then(toml::Value::as_table)
            .expect("[features] table");

        let mut edges = Vec::new();
        for (name, values) in features {
            for value in values.as_array().expect("feature list") {
                edges.push((
                    name.clone(),
                    value.as_str().expect("feature string").to_owned(),
                ));
            }
        }
        let deps = manifest
            .get("dependencies")
            .and_then(toml::Value::as_table)
            .expect("[dependencies] table");
        let tokio_features: Vec<String> = deps
            .get("tokio")
            .and_then(|tokio| tokio.get("features"))
            .and_then(toml::Value::as_array)
            .into_iter()
            .flatten()
            .map(|feature| feature.as_str().expect("feature string").to_owned())
            .collect();
        // `tact_llm` always depends on tokio, so a feature listed here is on
        // for every consumer, not only hosts that opt into telemetry.
        assert!(
            !tokio_features.iter().any(|feature| feature == "tracing"),
            "the tokio dependency must not enable tracing: {tokio_features:?}"
        );
        for feature in &tokio_features {
            edges.push(("tokio".to_owned(), feature.clone()));
        }

        // Both entry points matter: `default` is always on, and every feature
        // that implies `telemetry` ships the same behaviour.
        let mut reachable = vec!["telemetry".to_owned(), "default".to_owned()];
        for (name, values) in features {
            let implied = values
                .as_array()
                .expect("feature list")
                .iter()
                .any(|value| value.as_str() == Some("telemetry"));
            if implied {
                reachable.push(name.clone());
            }
        }
        let mut index = 0;
        while index < reachable.len() {
            let current = reachable[index].clone();
            index += 1;
            assert_ne!(
                current, "tokio/tracing",
                "feature path enables tokio/tracing: {reachable:?}"
            );
            for (from, to) in &edges {
                if *from == current && !reachable.contains(to) {
                    reachable.push(to.clone());
                }
            }
        }
    }
}
