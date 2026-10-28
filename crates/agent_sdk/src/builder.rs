use std::sync::Arc;

use tact_agent_api::{
    AgentState, ContextPolicy, EventObserver, EventPort, ModelCallOptions, ModelPort, ToolGate,
    ToolPort, TranscriptStore, UsageSink,
};
use tact_agent_core::{AgentKernel, KernelDeps, KernelExtensions};

use crate::{
    Agent, AgentBuildError, AgentConfig, AgentSession, BasicContextPolicy, DenyAllGate, EmptyTools,
    NoopEventPort, NoopTranscriptStore, NoopUsageSink, RunIdSource, SessionControl,
    TelemetryConfig,
};

/// Composes a reusable agent or a ready-to-run session.
#[derive(Clone)]
pub struct AgentBuilder {
    model: Arc<dyn ModelPort>,
    tools: Option<Arc<dyn ToolPort>>,
    gate: Option<Arc<dyn ToolGate>>,
    context: Option<Arc<dyn ContextPolicy>>,
    system_prompt: Option<String>,
    model_options: Option<ModelCallOptions>,
    transcript: Option<Arc<dyn TranscriptStore>>,
    usage: Option<Arc<dyn UsageSink>>,
    events: Option<Arc<dyn EventPort>>,
    observers: Vec<Arc<dyn EventObserver>>,
    max_total_turns: Option<u32>,
    run_id_source: Option<Arc<dyn RunIdSource>>,
    telemetry: TelemetryConfig,
}

impl AgentBuilder {
    pub fn new(model: Arc<dyn ModelPort>) -> Self {
        Self {
            model,
            tools: None,
            gate: None,
            context: None,
            system_prompt: None,
            model_options: None,
            transcript: None,
            usage: None,
            events: None,
            observers: Vec::new(),
            max_total_turns: Some(32),
            run_id_source: None,
            telemetry: TelemetryConfig::default(),
        }
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    pub fn model_options(mut self, options: ModelCallOptions) -> Self {
        self.model_options = Some(options);
        self
    }

    pub fn tools(mut self, tools: Arc<dyn ToolPort>) -> Self {
        self.tools = Some(tools);
        self
    }

    pub fn gate(mut self, gate: Arc<dyn ToolGate>) -> Self {
        self.gate = Some(gate);
        self
    }

    pub fn context(mut self, context: Arc<dyn ContextPolicy>) -> Self {
        self.context = Some(context);
        self
    }

    pub fn transcript(mut self, store: Arc<dyn TranscriptStore>) -> Self {
        self.transcript = Some(store);
        self
    }

    pub fn usage(mut self, sink: Arc<dyn UsageSink>) -> Self {
        self.usage = Some(sink);
        self
    }

    pub fn events(mut self, port: Arc<dyn EventPort>) -> Self {
        self.events = Some(port);
        self
    }

    pub fn observer(mut self, observer: Arc<dyn EventObserver>) -> Self {
        self.observers.push(observer);
        self
    }

    pub fn max_total_turns(mut self, max: impl Into<Option<u32>>) -> Self {
        self.max_total_turns = max.into();
        self
    }

    pub fn run_id_source(mut self, source: Arc<dyn RunIdSource>) -> Self {
        self.run_id_source = Some(source);
        self
    }

    pub fn telemetry(mut self, config: TelemetryConfig) -> Self {
        self.telemetry = config;
        self
    }

    pub fn build(self) -> Result<Agent, AgentBuildError> {
        let (kernel, config) = self.compose()?;
        Ok(Agent {
            kernel: Arc::new(kernel),
            config: Arc::new(config),
        })
    }

    pub fn build_session(
        self,
        session_id: impl Into<String>,
    ) -> Result<AgentSession, AgentBuildError> {
        self.build_session_with_state(session_id.into(), None)
    }

    pub fn build_session_from_state(
        self,
        session_id: impl Into<String>,
        state: AgentState,
    ) -> Result<AgentSession, AgentBuildError> {
        if !state.initialized {
            return Err(AgentBuildError::UninitializedState);
        }
        self.build_session_with_state(session_id.into(), Some(state))
    }

    fn build_session_with_state(
        self,
        session_id: String,
        state: Option<AgentState>,
    ) -> Result<AgentSession, AgentBuildError> {
        if state.as_ref().is_some_and(|state| !state.initialized) {
            return Err(AgentBuildError::UninitializedState);
        }
        let (kernel, config) = self.compose()?;
        Ok(AgentSession::new(
            Arc::new(kernel),
            session_id,
            state.unwrap_or_default(),
            Arc::new(config),
            SessionControl::new(),
        ))
    }

    fn compose(self) -> Result<(AgentKernel, AgentConfig), AgentBuildError> {
        if self.context.is_some() && (self.system_prompt.is_some() || self.model_options.is_some())
        {
            return Err(AgentBuildError::ConflictingContextConfig);
        }

        let context: Arc<dyn ContextPolicy> = self.context.unwrap_or_else(|| {
            Arc::new(BasicContextPolicy::new(
                self.system_prompt.unwrap_or_default(),
                self.model_options.unwrap_or_default(),
            ))
        });

        #[allow(unused_mut)]
        let mut deps = KernelDeps {
            model: self.model,
            tools: self.tools.unwrap_or_else(|| Arc::new(EmptyTools)),
            gate: self.gate.unwrap_or_else(|| Arc::new(DenyAllGate)),
            context,
            transcript: self
                .transcript
                .unwrap_or_else(|| Arc::new(NoopTranscriptStore::default())),
            usage: self.usage.unwrap_or_else(|| Arc::new(NoopUsageSink)),
            events: self.events.unwrap_or_else(|| Arc::new(NoopEventPort)),
        };
        #[allow(unused_mut)]
        let mut observers = self.observers;

        #[cfg(feature = "telemetry")]
        if self.telemetry.is_enabled() {
            (deps, observers) =
                crate::telemetry::instrument_graph(deps, observers, &self.telemetry);
        }

        let kernel = AgentKernel::with_extensions(deps, KernelExtensions { observers });
        let config = AgentConfig::new(self.max_total_turns, self.run_id_source, self.telemetry);
        Ok((kernel, config))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use async_trait::async_trait;
    use tact_agent_api::{
        AgentMessage, AgentRole, AgentState, CancelToken, ContextError, ContextPolicy,
        ContextPreparation, ContextView, CursorUpdate, MessageOrigin, ModelCallOptions,
        ModelMessage, ModelPort, PreparationDecision, ReasoningEffort, RunId, RunInput, RunStop,
        SessionRef, StoreError, ToolError, ToolExecutionContext, ToolExecutionResult, ToolOutcome,
        ToolSpec, TranscriptSnapshot, TranscriptStore,
    };
    use tact_agent_core::test_support::{
        ModelScript, ScriptedContext, ScriptedModel, events, text_response,
    };

    use crate::{AgentBuildError, BasicContextPolicy, ToolHandler, ToolSet};

    use super::AgentBuilder;

    fn model(text: &str) -> Arc<ScriptedModel> {
        ScriptedModel::new(vec![events(text_response(
            "assistant-1",
            text,
            tact_agent_api::StopReason::EndTurn,
        ))])
    }

    struct PanicLoadStore {
        loaded: AtomicBool,
    }

    struct TrackingTool {
        executions: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl ToolHandler for TrackingTool {
        fn spec(&self) -> ToolSpec {
            ToolSpec {
                name: "tracked".into(),
                description: "tracked tool".into(),
                input_schema: serde_json::json!({"type": "object"}),
            }
        }

        async fn execute(
            &self,
            _input: serde_json::Value,
            _context: ToolExecutionContext,
        ) -> Result<ToolExecutionResult, ToolError> {
            self.executions.fetch_add(1, Ordering::SeqCst);
            Ok(ToolExecutionResult::success(ToolOutcome::text("ok")))
        }
    }

    #[derive(Clone, Default)]
    struct CapturingContext {
        turns: Arc<Mutex<Vec<Option<u32>>>>,
    }

    #[async_trait]
    impl ContextPolicy for CapturingContext {
        async fn prepare_run(
            &self,
            input: &RunInput,
            _state: &ContextView<'_>,
            _model: &dyn ModelPort,
            _cancel: CancelToken,
        ) -> Result<PreparationDecision<ContextPreparation>, ContextError> {
            self.turns
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(input.max_total_turns);
            Ok(PreparationDecision::Continue(ContextPreparation {
                inputs: input.inputs.clone(),
                appended_messages: Vec::new(),
            }))
        }

        async fn compact(
            &self,
            _state: &ContextView<'_>,
            _request: tact_agent_api::CompactionRequest,
            _model: &dyn ModelPort,
            _cancel: CancelToken,
        ) -> Result<tact_agent_api::CompactionOutcome, ContextError> {
            Ok(tact_agent_api::CompactionOutcome::Skipped {
                reason: "test".into(),
            })
        }

        async fn prepare_request(
            &self,
            state: &ContextView<'_>,
            _model: &dyn ModelPort,
            _cancel: CancelToken,
        ) -> Result<PreparationDecision<tact_agent_api::RequestPlan>, ContextError> {
            Ok(PreparationDecision::Continue(tact_agent_api::RequestPlan {
                request: tact_agent_api::PreparedRequest {
                    system: String::new(),
                    messages: state.transcript.iter().map(ModelMessage::from).collect(),
                    cursor: None,
                    options: ModelCallOptions::default(),
                },
                appended_messages: Vec::new(),
            }))
        }
    }

    #[async_trait]
    impl TranscriptStore for PanicLoadStore {
        async fn load(
            &self,
            _session: &tact_agent_api::SessionRef,
        ) -> Result<TranscriptSnapshot, StoreError> {
            self.loaded.store(true, Ordering::SeqCst);
            Err(StoreError::Load("load must not run".into()))
        }

        async fn append(
            &self,
            _session: &tact_agent_api::SessionRef,
            _message: &AgentMessage,
            _cursor: CursorUpdate,
        ) -> Result<tact_agent_api::MessageReceipt, StoreError> {
            Ok(tact_agent_api::MessageReceipt {
                range: tact_agent_api::MessageRange { start: 1, end: 1 },
            })
        }

        async fn replace(
            &self,
            _session: &tact_agent_api::SessionRef,
            _transcript: Option<&[AgentMessage]>,
            _cursor: CursorUpdate,
        ) -> Result<tact_agent_api::ReplaceReceipt, StoreError> {
            Ok(tact_agent_api::ReplaceReceipt { range: None })
        }
    }

    #[test]
    fn builder_defaults_are_safe() {
        let agent = AgentBuilder::new(model("ok")).build().unwrap();
        assert_eq!(agent.config.default_max_total_turns, Some(32));
    }

    #[tokio::test]
    async fn deny_all_gate_prevents_execution() {
        let executions = Arc::new(AtomicUsize::new(0));
        let tools = Arc::new(
            ToolSet::new()
                .add(TrackingTool {
                    executions: executions.clone(),
                })
                .unwrap(),
        );
        let scripted = ScriptedModel::new(vec![
            events(tact_agent_core::test_support::tool_response(
                "assistant",
                &[("call-1", "tracked", serde_json::json!({}))],
                tact_agent_api::StopReason::ToolUse,
            )),
            events(text_response(
                "assistant-2",
                "done",
                tact_agent_api::StopReason::EndTurn,
            )),
        ]);
        let mut session = AgentBuilder::new(scripted)
            .tools(tools)
            .build_session("denied")
            .unwrap();
        session.run_text("use it").await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn allow_all_gate_runs_a_tool() {
        let executions = Arc::new(AtomicUsize::new(0));
        let tools = Arc::new(
            ToolSet::new()
                .add(TrackingTool {
                    executions: executions.clone(),
                })
                .unwrap(),
        );
        let scripted = ScriptedModel::new(vec![
            events(tact_agent_core::test_support::tool_response(
                "assistant",
                &[("call-1", "tracked", serde_json::json!({}))],
                tact_agent_api::StopReason::ToolUse,
            )),
            events(text_response(
                "assistant-2",
                "done",
                tact_agent_api::StopReason::EndTurn,
            )),
        ]);
        let mut session = AgentBuilder::new(scripted)
            .tools(tools)
            .gate(Arc::new(crate::AllowAllGate))
            .build_session("allowed")
            .unwrap();
        session.run_text("use it").await.unwrap();
        assert_eq!(executions.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn build_session_runs_text_and_persists_state() {
        let mut session = AgentBuilder::new(model("hello"))
            .build_session("chat")
            .unwrap();
        let outcome = session.run_text("hi").await.unwrap();
        assert_eq!(outcome.stop, RunStop::Completed);
        assert!(session.state().initialized);
        assert_eq!(session.state().total_turns, 1);
        assert_eq!(session.transcript().len(), 2);
    }

    #[tokio::test]
    async fn basic_context_policy_forwards_inputs() {
        let session = SessionRef::new("chat");
        let capabilities = tact_agent_core::test_support::default_capabilities();
        let transcript = Vec::new();
        let input = RunInput {
            run_id: RunId::new("run"),
            session: session.clone(),
            inputs: vec![AgentMessage::text(
                AgentRole::User,
                MessageOrigin::User,
                "hello",
            )],
            max_total_turns: Some(32),
        };
        let view = ContextView {
            session: &session,
            transcript: &transcript,
            cursor: None,
            total_turns: 0,
            max_total_turns: Some(32),
            last_token_total: None,
            pending_inputs: &[],
            capabilities: &capabilities,
            remaining_turns: Some(32),
        };
        let policy = BasicContextPolicy::new("system", ModelCallOptions::default());
        let model = model("unused");
        let decision = policy
            .prepare_run(&input, &view, model.as_ref(), CancelToken::new())
            .await
            .unwrap();
        let PreparationDecision::Continue(preparation) = decision else {
            panic!("basic context must continue");
        };
        assert_eq!(preparation.inputs.len(), 1);
        assert_eq!(preparation.inputs[0].text_content(), "hello");
        assert!(preparation.appended_messages.is_empty());
    }

    #[tokio::test]
    async fn reused_agent_isolates_sessions() {
        let scripts = ScriptedModel::new(vec![
            events(text_response(
                "a",
                "first",
                tact_agent_api::StopReason::EndTurn,
            )),
            events(text_response(
                "b",
                "second",
                tact_agent_api::StopReason::EndTurn,
            )),
        ]);
        let agent = AgentBuilder::new(scripts).build().unwrap();
        let mut first = agent.session("first");
        let mut second = agent.session("second");
        first.run_text("one").await.unwrap();
        second.run_text("two").await.unwrap();
        assert_eq!(
            first.last_assistant_message().unwrap().text_content(),
            "first"
        );
        assert_eq!(
            second.last_assistant_message().unwrap().text_content(),
            "second"
        );
        assert_ne!(first.session_id(), second.session_id());
    }

    #[tokio::test]
    async fn build_session_from_state_skips_reload() {
        let store = Arc::new(PanicLoadStore {
            loaded: AtomicBool::new(false),
        });
        let state = AgentState {
            initialized: true,
            transcript: vec![AgentMessage::text(
                AgentRole::User,
                MessageOrigin::User,
                "prior",
            )],
            ..AgentState::default()
        };
        let mut session = AgentBuilder::new(model("next"))
            .transcript(store.clone())
            .build_session_from_state("chat", state)
            .unwrap();
        session.run_text("now").await.unwrap();
        assert!(!store.loaded.load(Ordering::SeqCst));
    }

    #[test]
    fn restored_uninitialized_state_is_rejected() {
        let state = AgentState::default();
        assert_eq!(
            AgentBuilder::new(model("ok"))
                .build_session_from_state("chat", state.clone())
                .unwrap_err(),
            AgentBuildError::UninitializedState
        );
        let agent = AgentBuilder::new(model("ok")).build().unwrap();
        assert_eq!(
            agent.session_from_state("chat", state).unwrap_err(),
            AgentBuildError::UninitializedState
        );
    }

    #[tokio::test]
    async fn max_total_turns_accepts_value_option_and_none() {
        let value = CapturingContext::default();
        let mut session = AgentBuilder::new(model("ok"))
            .context(Arc::new(value.clone()))
            .max_total_turns(24)
            .build_session("value")
            .unwrap();
        session.run_text("run").await.unwrap();
        assert_eq!(value.turns.lock().unwrap().as_slice(), &[Some(24)]);

        let some = CapturingContext::default();
        let mut session = AgentBuilder::new(model("ok"))
            .context(Arc::new(some.clone()))
            .max_total_turns(Some(7))
            .build_session("some")
            .unwrap();
        session.run_text("run").await.unwrap();
        assert_eq!(some.turns.lock().unwrap().as_slice(), &[Some(7)]);

        let none = CapturingContext::default();
        let mut session = AgentBuilder::new(model("ok"))
            .context(Arc::new(none.clone()))
            .max_total_turns(None)
            .build_session("none")
            .unwrap();
        session.run_text("run").await.unwrap();
        assert_eq!(none.turns.lock().unwrap().as_slice(), &[None]);
    }

    #[tokio::test]
    async fn model_options_reach_prepared_request() {
        let scripted = model("reply");
        let mut session = AgentBuilder::new(scripted.clone())
            .model_options(ModelCallOptions {
                reasoning_effort: Some(ReasoningEffort::High),
                temperature: Some(0.25),
                ..ModelCallOptions::default()
            })
            .build_session("chat")
            .unwrap();
        session.run_text("hello").await.unwrap();
        assert_eq!(
            scripted.requests()[0].options.reasoning_effort,
            Some(ReasoningEffort::High)
        );
        assert_eq!(scripted.requests()[0].options.temperature, Some(0.25));
    }

    #[test]
    fn custom_context_conflicts_with_system_prompt() {
        let result = AgentBuilder::new(model("ok"))
            .context(ScriptedContext::new())
            .system_prompt("x")
            .build();
        assert_eq!(
            result.unwrap_err(),
            AgentBuildError::ConflictingContextConfig
        );
    }

    #[test]
    fn custom_context_without_model_options_succeeds() {
        assert!(
            AgentBuilder::new(model("ok"))
                .context(ScriptedContext::new())
                .build()
                .is_ok()
        );
    }

    #[test]
    fn custom_context_conflicts_with_model_options() {
        let result = AgentBuilder::new(model("ok"))
            .context(ScriptedContext::new())
            .model_options(ModelCallOptions::default())
            .build();
        assert_eq!(
            result.unwrap_err(),
            AgentBuildError::ConflictingContextConfig
        );
    }

    #[tokio::test]
    async fn timeout_bounded_cancellation_returns_cancelled() {
        let scripted = ScriptedModel::new(vec![ModelScript::WaitForCancel(Vec::new())]);
        let agent = AgentBuilder::new(scripted).build().unwrap();
        let mut session = agent.session("chat");
        let control = session.control();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            control.cancel();
        });
        let outcome =
            tokio::time::timeout(std::time::Duration::from_secs(1), session.run_text("wait"))
                .await
                .expect("run should finish")
                .unwrap();
        assert_eq!(outcome.stop, RunStop::Cancelled);
    }

    #[test]
    fn last_assistant_message_returns_latest_assistant() {
        let state = AgentState {
            initialized: true,
            transcript: vec![
                AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "old"),
                AgentMessage::text(AgentRole::User, MessageOrigin::User, "question"),
                AgentMessage::text(AgentRole::Assistant, MessageOrigin::Assistant, "new"),
            ],
            ..AgentState::default()
        };
        let session = AgentBuilder::new(model("unused"))
            .build_session_from_state("chat", state)
            .unwrap();
        assert_eq!(
            session.last_assistant_message().unwrap().text_content(),
            "new"
        );
    }

    #[test]
    fn explicit_default_model_options_still_conflicts() {
        let result = AgentBuilder::new(model("ok"))
            .context(ScriptedContext::new())
            .model_options(ModelCallOptions::default())
            .build();
        assert_eq!(
            result.unwrap_err(),
            AgentBuildError::ConflictingContextConfig
        );
    }
}
