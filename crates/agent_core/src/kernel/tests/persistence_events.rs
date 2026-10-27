use super::*;

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
