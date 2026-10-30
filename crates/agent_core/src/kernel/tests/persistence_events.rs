use super::*;

struct ContextEventPort(Arc<Mutex<Vec<(tact_agent_api::RunContext, AgentEvent)>>>);

impl tact_agent_api::EventPort for ContextEventPort {
    fn emit(&self, _event: AgentEvent) {}

    fn emit_shared_with_context(
        &self,
        context: &tact_agent_api::RunContext,
        event: Arc<AgentEvent>,
        _delivery: tact_agent_api::EventDelivery,
    ) {
        self.0
            .lock()
            .expect("context events lock")
            .push((context.clone(), (*event).clone()));
    }
}

struct HangingTranscriptStore;

#[async_trait::async_trait]
impl TranscriptStore for HangingTranscriptStore {
    async fn load(
        &self,
        _session: &tact_agent_api::SessionRef,
    ) -> Result<tact_agent_api::TranscriptSnapshot, tact_agent_api::StoreError> {
        std::future::pending().await
    }

    async fn append(
        &self,
        _session: &tact_agent_api::SessionRef,
        _message: &AgentMessage,
        _cursor: CursorUpdate,
    ) -> Result<tact_agent_api::MessageReceipt, tact_agent_api::StoreError> {
        Ok(tact_agent_api::MessageReceipt {
            range: tact_agent_api::MessageRange { start: 1, end: 1 },
        })
    }

    async fn replace(
        &self,
        _session: &tact_agent_api::SessionRef,
        _transcript: Option<&[AgentMessage]>,
        _cursor: CursorUpdate,
    ) -> Result<tact_agent_api::ReplaceReceipt, tact_agent_api::StoreError> {
        Ok(tact_agent_api::ReplaceReceipt { range: None })
    }
}

#[tokio::test]
async fn uninitialized_state_loads_session_before_applying_new_input() {
    let store = Arc::new(InMemoryTranscriptStore::new());
    let session = session();
    store
        .append(
            &session,
            &AgentMessage::text(AgentRole::User, MessageOrigin::User, "old"),
            CursorUpdate::Unchanged,
        )
        .await
        .unwrap();
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let usage = Arc::new(RecordingUsageSink::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model: model.clone(),
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage,
        events,
    });
    let mut state = AgentState::default();

    let _ = kernel
        .run(&mut state, input("new"), CancelToken::new())
        .await
        .unwrap();

    assert!(state.initialized);
    let requests = model.requests();
    let texts = requests[0]
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            AgentContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(texts, ["old", "new"]);
}

#[tokio::test]
async fn durable_runtime_state_preserves_the_turn_budget() {
    let store = Arc::new(InMemoryTranscriptStore::new());
    let first_model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "first",
        StopReason::EndTurn,
    ))]);
    let first_kernel = AgentKernel::new(KernelDeps {
        model: first_model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });
    let mut first_state = AgentState::default();
    let mut first_input = input("first");
    first_input.max_total_turns = Some(1);
    first_kernel
        .run(&mut first_state, first_input, CancelToken::new())
        .await
        .unwrap();

    let second_model = ScriptedModel::new(vec![events(text_response(
        "a2",
        "second",
        StopReason::EndTurn,
    ))]);
    let second_kernel = AgentKernel::new(KernelDeps {
        model: second_model.clone(),
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });
    let mut second_state = AgentState::default();
    let mut second_input = input("second");
    second_input.max_total_turns = Some(1);
    let outcome = second_kernel
        .run(&mut second_state, second_input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTurns);
    assert_eq!(second_state.total_turns, 1);
    assert_eq!(second_model.stream_calls(), 0);
}

#[tokio::test]
async fn cancellation_interrupts_a_hanging_transcript_load() {
    let kernel = AgentKernel::new(KernelDeps {
        model: ScriptedModel::new(vec![]),
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: Arc::new(HangingTranscriptStore),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });
    let cancel = CancelToken::new();
    let cancel_from_task = cancel.clone();
    let mut state = AgentState::default();
    let task = tokio::spawn(async move {
        kernel
            .run(&mut state, input("hung"), cancel_from_task)
            .await
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    cancel.cancel();
    let outcome = tokio::time::timeout(Duration::from_millis(200), task)
        .await
        .expect("cancellation must bound a hung store load")
        .expect("run task")
        .expect("cancelled run");

    assert_eq!(outcome.stop, RunStop::Cancelled);
}

#[tokio::test]
async fn every_event_is_tagged_with_the_activation_context() {
    let events = Arc::new(ContextEventPort(Arc::new(Mutex::new(Vec::new()))));
    let kernel = AgentKernel::new(KernelDeps {
        model: ScriptedModel::new(vec![events_with_deltas(
            text_response("a1", "hello", StopReason::EndTurn),
            vec![MessageDelta::Text {
                delta: "hello".into(),
            }],
        )]),
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: Arc::new(InMemoryTranscriptStore::new()),
        usage: Arc::new(RecordingUsageSink::new()),
        events: events.clone(),
    });
    let mut state = AgentState::default();
    kernel
        .run(&mut state, input("context"), CancelToken::new())
        .await
        .unwrap();

    let recorded = events.0.lock().expect("context events lock");
    assert!(!recorded.is_empty());
    assert!(recorded.iter().all(|(context, _)| {
        context.session == session() && context.run_id == RunId::new("test-run")
    }));
    assert!(recorded.iter().any(|(_, event)| {
        matches!(event, AgentEvent::MessageDelta { delta, .. } if matches!(delta, MessageDelta::Text { .. }))
    }));
}

#[tokio::test]
async fn usage_flush_failure_emits_only_run_failed() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let store = Arc::new(InMemoryTranscriptStore::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage: FailingUsageSink::new(false, true),
        events: events.clone(),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("flush"));
    assert!(
        events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFailed { .. }))
    );
    assert!(
        !events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::RunFinished { .. }))
    );
}
