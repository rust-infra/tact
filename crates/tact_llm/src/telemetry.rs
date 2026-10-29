//! Body-free tracing for stable provider calls.

use tact_agent_api::{ModelError, ModelEvent, ModelResponse};

use crate::ProviderMetadata;

#[cfg(feature = "telemetry")]
use std::time::Instant;

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

#[cfg(all(test, feature = "telemetry"))]
mod tests {
    use std::{
        io::{self, Write},
        sync::{Arc, Mutex},
        time::Duration,
    };

    use tact_agent_api::{
        AgentMessage, AgentRole, CursorUpdate, MessageId, MessageOrigin, ModelCapabilities,
        ModelErrorKind, StopReason, TokenUsage,
    };
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;
    use crate::{Endpoint, ProviderKind, WireProtocol};

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .expect("capture lock")
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

    #[test]
    fn tracing_records_safe_call_fields_and_redacts_payloads() {
        let capture = Capture(Arc::new(Mutex::new(Vec::new())));
        let subscriber = tracing_subscriber::fmt()
            .with_writer(capture.clone())
            .with_ansi(false)
            .without_time()
            .finish();
        let _guard = tracing::subscriber::set_default(subscriber);

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

        let output = String::from_utf8(capture.0.lock().unwrap().clone()).unwrap();
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
}
