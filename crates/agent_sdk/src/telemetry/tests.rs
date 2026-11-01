use std::{
    collections::HashMap,
    io::Write,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use async_trait::async_trait;
use serde_json::json;
use tact_agent_api::{
    CancelToken, ContextError, ContextPolicy, ContextPreparation, ContextView, ModelCapabilities,
    ModelCursor, ModelError, ModelMessage, ModelPort, ModelRequest, ModelResponse, ModelStream,
    PreparationDecision, PreparedRequest, RequestPlan, RunContext, RunId, RunInput, SessionRef,
    StopReason, TokenUsage,
};
use tact_agent_core::test_support::{ScriptedModel, events, text_response};
use tracing_subscriber::fmt::MakeWriter;

use crate::{AgentBuilder, AllowAllGate, TelemetryConfig, ToolHandler, ToolSet};

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

struct Writer(Captured);

impl Write for Writer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Captured {
    type Writer = Writer;

    fn make_writer(&'a self) -> Self::Writer {
        Writer(self.clone())
    }
}

use tracing_subscriber::prelude::*;

#[derive(Clone, Default)]
struct RecordedDurations {
    spans: Arc<Mutex<HashMap<tracing::span::Id, String>>>,
    recorded: Arc<Mutex<Vec<String>>>,
}

impl RecordedDurations {
    fn names(&self) -> Vec<String> {
        self.recorded.lock().unwrap().clone()
    }
}

struct DurationField(bool);

impl tracing::field::Visit for DurationField {
    fn record_debug(&mut self, field: &tracing::field::Field, _value: &dyn std::fmt::Debug) {
        if field.name() == "duration_ms" {
            self.0 = true;
        }
    }

    fn record_u64(&mut self, field: &tracing::field::Field, _value: u64) {
        if field.name() == "duration_ms" {
            self.0 = true;
        }
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for RecordedDurations {
    fn on_new_span(
        &self,
        attributes: &tracing::span::Attributes<'_>,
        id: &tracing::span::Id,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        self.spans
            .lock()
            .unwrap()
            .insert(id.clone(), attributes.metadata().name().to_owned());
    }

    fn on_record(
        &self,
        id: &tracing::span::Id,
        values: &tracing::span::Record<'_>,
        _context: tracing_subscriber::layer::Context<'_, S>,
    ) {
        let mut duration = DurationField(false);
        values.record(&mut duration);
        if duration.0
            && let Some(name) = self.spans.lock().unwrap().get(id).cloned()
        {
            self.recorded.lock().unwrap().push(name);
        }
    }
}

fn recording_subscriber(
    captured: &Captured,
    durations: &RecordedDurations,
) -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::registry()
        .with(tracing_subscriber::filter::LevelFilter::TRACE)
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(captured.clone())
                .with_filter(tracing_subscriber::filter::LevelFilter::TRACE),
        )
        .with(durations.clone())
}

struct CanaryTool;

#[async_trait]
impl ToolHandler for CanaryTool {
    fn spec(&self) -> tact_agent_api::ToolSpec {
        tact_agent_api::ToolSpec {
            name: "canary".into(),
            description: "canary".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    fn metadata(&self) -> serde_json::Value {
        json!({"secret": "METADATA_CANARY"})
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _context: tact_agent_api::ToolExecutionContext,
    ) -> Result<tact_agent_api::ToolExecutionResult, tact_agent_api::ToolError> {
        Ok(tact_agent_api::ToolExecutionResult::success(
            tact_agent_api::ToolOutcome::text("RESULT_CANARY"),
        ))
    }
}

struct SlowTool;

#[async_trait]
impl ToolHandler for SlowTool {
    fn spec(&self) -> tact_agent_api::ToolSpec {
        tact_agent_api::ToolSpec {
            name: "slow".into(),
            description: "slow".into(),
            input_schema: json!({"type": "object"}),
        }
    }

    async fn execute(
        &self,
        _input: serde_json::Value,
        _context: tact_agent_api::ToolExecutionContext,
    ) -> Result<tact_agent_api::ToolExecutionResult, tact_agent_api::ToolError> {
        tokio::time::sleep(Duration::from_millis(5)).await;
        Ok(tact_agent_api::ToolExecutionResult::success(
            tact_agent_api::ToolOutcome::text("slow result"),
        ))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_default_excludes_tool_payload_canary() {
    let captured = Captured::default();
    let durations = RecordedDurations::default();
    let subscriber = recording_subscriber(&captured, &durations);
    let _guard = tracing::subscriber::set_default(subscriber);

    let model = ScriptedModel::new(vec![events(tact_agent_core::test_support::tool_response(
        "assistant",
        &[("call", "canary", json!({"secret": "INPUT_CANARY"}))],
        StopReason::ToolUse,
    )), events(text_response(
        "assistant",
        "done",
        StopReason::EndTurn,
    ))]);
    let agent = AgentBuilder::new(model)
        .tools(Arc::new(ToolSet::new().add(CanaryTool).unwrap()))
        .gate(Arc::new(AllowAllGate))
        .telemetry(TelemetryConfig::default())
        .build()
        .unwrap();
    let mut session = agent.session("canary-session");
    session.run_text("use it").await.unwrap();

    let output = captured.text();
    assert!(!output.contains("INPUT_CANARY"));
    assert!(!output.contains("METADATA_CANARY"));
    assert!(!output.contains("RESULT_CANARY"));
    assert!(output.contains("agent.run"));
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_records_model_and_tool_durations() {
    let captured = Captured::default();
    let durations = RecordedDurations::default();
    let subscriber = recording_subscriber(&captured, &durations);
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut response = text_response("assistant", "done", StopReason::EndTurn);
    response.usage = Some(TokenUsage {
        input_tokens: 12,
        output_tokens: 3,
        total_tokens: 15,
        ..TokenUsage::default()
    });
    let model = ScriptedModel::new(vec![
        events(tact_agent_core::test_support::tool_response(
            "tool-call",
            &[("call-1", "slow", json!({}))],
            StopReason::ToolUse,
        )),
        events(response),
    ]);
    let agent = AgentBuilder::new(model)
        .tools(Arc::new(ToolSet::new().add(SlowTool).unwrap()))
        .gate(Arc::new(AllowAllGate))
        .telemetry(TelemetryConfig::default())
        .build()
        .unwrap();
    let mut session = agent.session("timed");
    session.run_text("go").await.unwrap();

    let output = captured.text();
    assert!(output.contains("agent.tool.execute"));
    assert!(output.contains("time_to_first_event_ms"));
    assert!(output.contains("duration_ms"));
    let recorded = durations.names();
    for span in [
        "agent.run",
        "agent.model.call",
        "agent.tool.refresh",
        "agent.tool.execute",
        "agent.gate.before",
        "agent.gate.after",
        "agent.context.prepare_run",
        "agent.context.prepare_request",
        "agent.store.load",
        "agent.store.append",
    ] {
        assert!(
            recorded.iter().any(|recorded| recorded == span),
            "missing duration_ms on {span}: {recorded:?}"
        );
    }
    assert!(output.contains("input_tokens=12"));
    assert!(output.contains("total_tokens=15"));
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_records_time_to_first_event() {
    let captured = Captured::default();
    // `with_ansi(false)`: field assertions below match plain `field=value`
    // text, and ANSI styling is on whenever another workspace member unifies
    // tracing-subscriber's default features into this test build.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let model = ScriptedModel::new(vec![events(text_response(
        "assistant",
        "done",
        StopReason::EndTurn,
    ))]);
    let mut session = AgentBuilder::new(model).build_session("first-event").unwrap();
    session.run_text("go").await.unwrap();

    assert!(captured.text().contains("time_to_first_event_ms"));
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_records_usage_tokens() {
    let captured = Captured::default();
    // `with_ansi(false)`: field assertions below match plain `field=value`
    // text, and ANSI styling is on whenever another workspace member unifies
    // tracing-subscriber's default features into this test build.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut response = text_response("assistant", "done", StopReason::EndTurn);
    response.usage = Some(TokenUsage {
        input_tokens: 21,
        output_tokens: 8,
        total_tokens: 29,
        ..TokenUsage::default()
    });
    let mut session = AgentBuilder::new(ScriptedModel::new(vec![events(response)]))
        .build_session("usage")
        .unwrap();
    session.run_text("go").await.unwrap();

    let output = captured.text();
    assert!(output.contains("input_tokens=21"));
    assert!(output.contains("output_tokens=8"));
    assert!(output.contains("total_tokens=29"));
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_does_not_log_content_by_default() {
    let captured = Captured::default();
    // `with_ansi(false)`: field assertions below match plain `field=value`
    // text, and ANSI styling is on whenever another workspace member unifies
    // tracing-subscriber's default features into this test build.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let mut session = AgentBuilder::new(ScriptedModel::new(vec![events(text_response(
        "assistant",
        "OUTPUT_CANARY",
        StopReason::EndTurn,
    ))]))
    .build_session("content-canary")
    .unwrap();
    session.run_text("INPUT_CANARY").await.unwrap();

    let output = captured.text();
    assert!(!output.contains("INPUT_CANARY"));
    assert!(!output.contains("OUTPUT_CANARY"));
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_content_is_bounded_when_explicitly_enabled() {
    let captured = Captured::default();
    // `with_ansi(false)`: field assertions below match plain `field=value`
    // text, and ANSI styling is on whenever another workspace member unifies
    // tracing-subscriber's default features into this test build.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let input = format!("{}{}", "甲".repeat(512), "INPUT_SUFFIX");
    let output = format!("{}{}", "乙".repeat(512), "OUTPUT_SUFFIX");
    let mut session = AgentBuilder::new(ScriptedModel::new(vec![events(text_response(
        "assistant",
        &output,
        StopReason::EndTurn,
    ))]))
    .telemetry(TelemetryConfig::default().record_content(true))
    .build_session("bounded-content")
    .unwrap();
    session.run_text(input.clone()).await.unwrap();

    let logged = captured.text();
    assert!(logged.contains(&"甲".repeat(512)));
    assert!(logged.contains(&"乙".repeat(512)));
    assert!(!logged.contains(&input));
    assert!(!logged.contains(&output));
    assert!(!logged.contains("INPUT_SUFFIX"));
    assert!(!logged.contains("OUTPUT_SUFFIX"));
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_compaction_span_durations_are_recorded() {
    let captured = Captured::default();
    let durations = RecordedDurations::default();
    let subscriber = recording_subscriber(&captured, &durations);
    let _guard = tracing::subscriber::set_default(subscriber);

    let model = ScriptedModel::new(vec![events(text_response(
        "assistant",
        "ok",
        StopReason::EndTurn,
    ))]);
    model.push_native_compaction(events(text_response(
        "compaction",
        "summary",
        StopReason::EndTurn,
    )));
    let mut session = AgentBuilder::new(model)
        .context(Arc::new(CounterContext::default()))
        .build_session("span-durations")
        .unwrap();
    session.run_text("go").await.unwrap();

    let recorded = durations.names();
    for span in [
        "agent.context.compact",
        "agent.context.compaction_committed",
        "agent.store.replace",
        "agent.usage.record",
        "agent.usage.flush",
    ] {
        assert!(
            recorded.iter().any(|recorded| recorded == span),
            "missing duration_ms on {span}: {recorded:?}"
        );
    }
}

#[tokio::test]
async fn telemetry_wrappers_forward_default_methods() {
    let model = ScriptedModel::new(vec![events(text_response(
        "assistant",
        "ok",
        StopReason::EndTurn,
    ))]);
    model.push_native_compaction(events(text_response(
        "compaction",
        "summary",
        StopReason::EndTurn,
    )));
    let context = CounterContext::default();
    let mut session = AgentBuilder::new(model)
        .context(Arc::new(context.clone()))
        .build_session("forward")
        .unwrap();
    session.run_text("go").await.unwrap();
    assert_eq!(context.compact_calls.load(Ordering::SeqCst), 1);
    assert_eq!(context.committed.load(Ordering::SeqCst), 1);
}

#[derive(Default, Clone)]
struct CounterContext {
    request_calls: Arc<AtomicUsize>,
    compact_calls: Arc<AtomicUsize>,
    committed: Arc<AtomicUsize>,
}

#[async_trait]
impl ContextPolicy for CounterContext {
    async fn prepare_run(
        &self,
        input: &RunInput,
        _state: &ContextView<'_>,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<PreparationDecision<ContextPreparation>, ContextError> {
        Ok(PreparationDecision::Continue(ContextPreparation {
            inputs: input.inputs.clone(),
            appended_messages: Vec::new(),
        }))
    }

    async fn prepare_request(
        &self,
        state: &ContextView<'_>,
        _model: &dyn ModelPort,
        _cancel: CancelToken,
    ) -> Result<PreparationDecision<RequestPlan>, ContextError> {
        let plan = RequestPlan {
            request: PreparedRequest {
                system: String::new(),
                messages: state.transcript.iter().map(ModelMessage::from).collect(),
                cursor: None,
                options: tact_agent_api::ModelCallOptions::default(),
            },
            appended_messages: Vec::new(),
        };
        if self.request_calls.fetch_add(1, Ordering::SeqCst) == 0 {
            Ok(PreparationDecision::Compact {
                request: tact_agent_api::CompactionRequest::Command { focus: None },
                if_skipped: plan,
            })
        } else {
            Ok(PreparationDecision::Continue(plan))
        }
    }

    async fn compact(
        &self,
        _state: &ContextView<'_>,
        request: tact_agent_api::CompactionRequest,
        model: &dyn ModelPort,
        cancel: CancelToken,
    ) -> Result<tact_agent_api::CompactionOutcome, ContextError> {
        self.compact_calls.fetch_add(1, Ordering::SeqCst);
        let capabilities = model.capabilities();
        model
            .compact(
                ModelRequest {
                    model: capabilities.model,
                    system: String::new(),
                    messages: Vec::new(),
                    tools: Vec::new(),
                    max_output_tokens: capabilities.max_output_tokens,
                    options: tact_agent_api::ModelCallOptions::default(),
                },
                None,
                cancel,
            )
            .await
            .map_err(|error| ContextError::Compaction(error.to_string()))?;
        let _ = request;
        Ok(tact_agent_api::CompactionOutcome::Replaced(
            tact_agent_api::ContextReplacement {
                transcript: Some(Vec::new()),
                cursor: tact_agent_api::CursorUpdate::Unchanged,
            },
        ))
    }

    async fn compaction_committed(
        &self,
        _state: &ContextView<'_>,
        _request: &tact_agent_api::CompactionRequest,
    ) -> Result<(), ContextError> {
        self.committed
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[tokio::test]
async fn no_subscriber_is_required_or_installed() {
    assert!(!tracing::dispatcher::has_been_set());
    let model = ScriptedModel::new(vec![events(text_response(
        "assistant",
        "ok",
        StopReason::EndTurn,
    ))]);
    let mut session = AgentBuilder::new(model)
        .build_session("no-subscriber")
        .unwrap();
    let outcome = session.run_text("go").await.unwrap();
    assert_eq!(outcome.stop, tact_agent_api::RunStop::Completed);
    assert!(!tracing::dispatcher::has_been_set());
}

struct EmptyStream;

#[async_trait]
impl ModelPort for EmptyStream {
    fn capabilities(&self) -> ModelCapabilities {
        tact_agent_core::test_support::default_capabilities()
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        Ok(Box::pin(futures_util::stream::empty()))
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::unsupported("test"))
    }
}

#[derive(Default)]
struct ContextStream {
    calls: AtomicUsize,
}

#[async_trait]
impl ModelPort for ContextStream {
    fn capabilities(&self) -> ModelCapabilities {
        tact_agent_core::test_support::default_capabilities()
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        Err(ModelError::unsupported("legacy stream"))
    }

    async fn stream_with_context(
        &self,
        _context: &RunContext,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(Box::pin(futures_util::stream::empty()))
    }
}

#[tokio::test]
async fn instrumented_model_forwards_context_aware_streams() {
    let inner = Arc::new(ContextStream::default());
    let port = super::model::InstrumentedModelPort::new(inner.clone(), TelemetryConfig::default());
    let request = ModelRequest {
        model: "test".into(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_output_tokens: 10,
        options: tact_agent_api::ModelCallOptions::default(),
    };
    let _stream = port
        .stream_with_context(
            &RunContext {
                session: SessionRef::new("session"),
                run_id: RunId::new("run"),
            },
            request,
            None,
            CancelToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn instrumented_stream_handles_empty_stream_without_fabricating_terminal() {
    let port = super::model::InstrumentedModelPort::new(
        Arc::new(EmptyStream),
        TelemetryConfig::default(),
    );
    use futures_util::StreamExt;
    let request = ModelRequest {
        model: "test".into(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_output_tokens: 10,
        options: tact_agent_api::ModelCallOptions::default(),
    };
    let mut stream = port
        .stream(request, None, CancelToken::new())
        .await
        .unwrap();
    assert!(stream.next().await.is_none());
}

type StreamScript = Result<Vec<Result<tact_agent_api::ModelEvent, ModelError>>, ModelError>;

struct ScriptedStream {
    script: Mutex<Option<StreamScript>>,
}

impl ScriptedStream {
    fn new(script: StreamScript) -> Self {
        Self {
            script: Mutex::new(Some(script)),
        }
    }
}

#[async_trait]
impl ModelPort for ScriptedStream {
    fn capabilities(&self) -> ModelCapabilities {
        tact_agent_core::test_support::default_capabilities()
    }

    async fn stream(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelStream, ModelError> {
        let script = self
            .script
            .lock()
            .unwrap()
            .take()
            .expect("one stream script");
        match script {
            Ok(events) => Ok(Box::pin(futures_util::stream::iter(events))),
            Err(error) => Err(error),
        }
    }

    async fn complete(
        &self,
        _request: ModelRequest,
        _cursor: Option<&ModelCursor>,
        _cancel: CancelToken,
    ) -> Result<ModelResponse, ModelError> {
        Err(ModelError::unsupported("test"))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn instrumented_stream_handles_empty_error_and_post_terminal_events() {
    use futures_util::StreamExt;

    let request = || ModelRequest {
        model: "test".into(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_output_tokens: 10,
        options: tact_agent_api::ModelCallOptions::default(),
    };

    let empty = super::model::InstrumentedModelPort::new(
        Arc::new(ScriptedStream::new(Ok(Vec::new()))),
        TelemetryConfig::default(),
    );
    assert!(
        empty
            .stream(request(), None, CancelToken::new())
            .await
            .unwrap()
            .next()
            .await
            .is_none()
    );

    let setup = super::model::InstrumentedModelPort::new(
        Arc::new(ScriptedStream::new(Err(ModelError::new(
            tact_agent_api::ModelErrorKind::Transient,
            "SETUP_ERROR",
        )))),
        TelemetryConfig::default(),
    );
    assert!(
        setup
            .stream(request(), None, CancelToken::new())
            .await
            .is_err()
    );

    let response = text_response("terminal", "done", StopReason::EndTurn);
    let trailing_usage = TokenUsage {
        input_tokens: 4,
        output_tokens: 2,
        total_tokens: 6,
        ..TokenUsage::default()
    };
    let post_terminal = super::model::InstrumentedModelPort::new(
        Arc::new(ScriptedStream::new(Ok(vec![
            Ok(tact_agent_api::ModelEvent::MessageStarted {
                message_id: response.message_id.clone(),
                role: tact_agent_api::AgentRole::Assistant,
            }),
            Ok(tact_agent_api::ModelEvent::Finished(response)),
            Ok(tact_agent_api::ModelEvent::Usage {
                usage: trailing_usage,
            }),
        ]))),
        TelemetryConfig::default(),
    );
    let mut stream = post_terminal
        .stream(request(), None, CancelToken::new())
        .await
        .unwrap();
    assert!(matches!(
        stream.next().await,
        Some(Ok(tact_agent_api::ModelEvent::MessageStarted { .. }))
    ));
    assert!(matches!(
        stream.next().await,
        Some(Ok(tact_agent_api::ModelEvent::Finished(_)))
    ));
    assert!(matches!(
        stream.next().await,
        Some(Ok(tact_agent_api::ModelEvent::Usage { usage }))
            if usage == trailing_usage
    ));
    assert!(stream.next().await.is_none());

    let error = ModelError::new(tact_agent_api::ModelErrorKind::Transient, "STREAM_ERROR");
    let failed = super::model::InstrumentedModelPort::new(
        Arc::new(ScriptedStream::new(Ok(vec![
            Err(error.clone()),
            Err(error),
        ]))),
        TelemetryConfig::default(),
    );
    let mut stream = failed
        .stream(request(), None, CancelToken::new())
        .await
        .unwrap();
    assert!(stream.next().await.is_some_and(|item| item.is_err()));
    assert!(stream.next().await.is_some_and(|item| item.is_err()));
    assert!(stream.next().await.is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn telemetry_first_error_does_not_record_time_to_first_event() {
    use futures_util::StreamExt;

    let captured = Captured::default();
    // `with_ansi(false)`: field assertions below match plain `field=value`
    // text, and ANSI styling is on whenever another workspace member unifies
    // tracing-subscriber's default features into this test build.
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_ansi(false)
        .with_writer(captured.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let error = ModelError::new(tact_agent_api::ModelErrorKind::Transient, "FIRST_ERROR");
    let failed = super::model::InstrumentedModelPort::new(
        Arc::new(ScriptedStream::new(Ok(vec![Err(error)]))),
        TelemetryConfig::default(),
    );
    let request = ModelRequest {
        model: "test".into(),
        system: String::new(),
        messages: Vec::new(),
        tools: Vec::new(),
        max_output_tokens: 10,
        options: tact_agent_api::ModelCallOptions::default(),
    };
    let mut stream = failed
        .stream(request, None, CancelToken::new())
        .await
        .unwrap();
    assert!(stream.next().await.is_some_and(|item| item.is_err()));

    let output = captured.text();
    assert!(!output.contains("time_to_first_event_ms"));
    assert!(output.contains("error.kind=Transient"));
}
