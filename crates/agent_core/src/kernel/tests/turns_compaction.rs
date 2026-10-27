use super::*;

#[tokio::test]
async fn absolute_turn_cap_is_checked_before_model_dispatch() {
    let model = ScriptedModel::new(Vec::new());
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    state.total_turns = 2;
    let mut input = input("go");
    input.max_total_turns = Some(2);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTurns);
    assert_eq!(outcome.turns, 0);
    assert_eq!(h.model.stream_calls(), 0);
}

#[tokio::test]
async fn transport_retry_counts_as_one_turn() {
    let model = ScriptedModel::new(vec![
        ModelScript::Error(ModelError {
            kind: ModelErrorKind::Transient,
            message: "retry".into(),
            retry_after: Some(Duration::ZERO),
        }),
        events(text_response("a1", "done", StopReason::EndTurn)),
    ]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.total_turns, 1);
    assert_eq!(outcome.recovery.transport_attempts, 1);
    assert_eq!(h.model.stream_calls(), 2);
}

#[tokio::test]
async fn cancellation_during_transport_backoff_keeps_the_retry_count() {
    let model = ScriptedModel::new(vec![ModelScript::Error(ModelError {
        kind: ModelErrorKind::Transient,
        message: "retry".into(),
        retry_after: Some(Duration::from_secs(5)),
    })]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h
        .kernel
        .run(&mut state, input("go"), cancel)
        .await
        .expect("cancellation during backoff returns a typed outcome");

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(outcome.recovery.transport_attempts, 1);
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn context_overflow_compacts_and_retries_as_a_second_dispatch() {
    let model = ScriptedModel::new(vec![
        ModelScript::Error(ModelError::new(ModelErrorKind::ContextOverflow, "overflow")),
        events(text_response("a1", "done", StopReason::EndTurn)),
    ]);
    let context = ScriptedContext::new();
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.turns, 2);
    assert_eq!(outcome.recovery.compact_attempts, 1);
    assert_eq!(
        h.context.compact_requests(),
        [CompactionRequest::Recovery { focus: None }]
    );
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::ContextCompacted {
            kind: tact_agent_api::CompactionKind::Recovery
        }
    )));
}

#[tokio::test]
async fn skipped_recovery_returns_the_original_overflow_error() {
    let model = ScriptedModel::new(vec![ModelScript::Error(ModelError::new(
        ModelErrorKind::ContextOverflow,
        "original overflow",
    ))]);
    let context = ScriptedContext::new();
    context.push_compaction(CompactionOutcome::Skipped {
        reason: "cannot compact".into(),
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context.clone(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("original overflow"));
    assert_eq!(
        context.compact_requests(),
        [CompactionRequest::Recovery { focus: None }]
    );
}

#[tokio::test]
async fn orphaned_tool_use_is_reconciled_before_the_first_request() {
    let mut state = state();
    state.transcript.push(AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        vec![AgentContentBlock::ToolUse {
            id: "orphan".into(),
            name: "tool".into(),
            input: serde_json::json!({}),
        }],
    ));
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "resumed",
        StopReason::EndTurn,
    ))]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );

    let _ = h
        .kernel
        .run(&mut state, input("continue"), CancelToken::new())
        .await
        .unwrap();

    let requests = h.model.requests();
    let first = requests.first().expect("first request");
    assert!(
        first
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| {
                matches!(
                    block,
                    AgentContentBlock::ToolResult { tool_use_id, content }
                        if tool_use_id == "orphan" && content.contains("interrupted")
                )
            })
    );
}

#[tokio::test]
async fn healthy_tool_pairing_is_not_rewritten_during_reconciliation() {
    let store = RecordingTranscriptStore::new();
    let mut state = state();
    state.transcript = vec![
        AgentMessage::new(
            AgentRole::Assistant,
            MessageOrigin::Assistant,
            vec![AgentContentBlock::ToolUse {
                id: "paired".into(),
                name: "tool".into(),
                input: serde_json::json!({}),
            }],
        ),
        AgentMessage::new(
            AgentRole::User,
            MessageOrigin::ToolResult,
            vec![AgentContentBlock::ToolResult {
                tool_use_id: "paired".into(),
                content: "already done".into(),
            }],
        ),
    ];
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "resumed",
        StopReason::EndTurn,
    ))]);
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });

    let _ = kernel
        .run(&mut state, input("continue"), CancelToken::new())
        .await
        .unwrap();

    assert!(
        !store
            .calls()
            .iter()
            .any(|call| matches!(call, StoreCall::Replace { .. }))
    );
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "paired")
    }));
}

#[tokio::test]
async fn injected_context_is_persisted_but_is_not_a_user_turn() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_run(RunPreparation::Continue {
        appended: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::GoalContext,
            "goal context",
        )],
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::GoalContext
            && message.text_content().contains("goal context")
    }));
    assert_eq!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin.is_user_turn())
            .count(),
        1
    );
}

#[tokio::test]
async fn injected_context_and_hosted_activity_keep_separate_non_user_channels() {
    let response = text_response("a1", "answer", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::ProviderActivity {
            activity: ProviderActivity {
                id: "search-1".into(),
                step: 0,
                kind: ProviderActivityKind::WebSearch {
                    phase: ProviderActivityPhase::Completed,
                    query: Some("tact kernel".into()),
                    failure: None,
                    duration: Some(Duration::from_millis(5)),
                },
            },
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let context = ScriptedContext::new();
    context.push_run(RunPreparation::Continue {
        appended: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "hook".into(),
            },
            "hook context",
        )],
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(h.tools.executions().is_empty());
    assert_eq!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin.is_user_turn())
            .count(),
        1
    );
    assert!(state.transcript.iter().any(|message| {
        message.origin
            == MessageOrigin::InjectedContext {
                source: "hook".into(),
            }
            && message.text_content() == "hook context"
    }));
    assert!(
        state
            .transcript
            .iter()
            .all(|message| message.origin != MessageOrigin::ToolResult)
    );
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::ProviderActivity { activity }
            if activity.id == "search-1"
                && matches!(
                    &activity.kind,
                    ProviderActivityKind::WebSearch {
                        query: Some(query),
                        ..
                    } if query == "tact kernel"
                )
    )));
    assert!(
        !h.events
            .events()
            .iter()
            .any(|event| matches!(event, AgentEvent::ToolStarted { .. }))
    );
}

#[tokio::test]
async fn usage_record_failure_prevents_tool_execution() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool("tool", ToolBehavior::Immediate(success("done")))]);
    let store = Arc::new(InMemoryTranscriptStore::new());
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model: model.clone(),
        tools: tools.clone(),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store,
        usage: FailingUsageSink::new(true, false),
        events: events.clone(),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("usage"));
    assert!(tools.executions().is_empty());
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

#[tokio::test]
async fn assistant_commit_failure_prevents_tool_execution() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("must not run")),
    )]);
    let store = FailingTranscriptStore::new(Some(2), None);
    let events = Arc::new(RecordingEventPort::new());
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: tools.clone(),
        gate: ScriptedGate::new(),
        context: ScriptedContext::new(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: events.clone(),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("forced append failure"));
    assert!(tools.executions().is_empty());
    assert_eq!(store.transcript(&session()).len(), 1);
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

#[tokio::test]
async fn malformed_stream_cardinality_fails_the_run() {
    let response = text_response("a1", "done", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::Finished(response.clone())),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("terminal"));
}

#[tokio::test]
async fn stream_without_a_terminal_response_fails_without_committing() {
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![Ok(
        ModelEvent::MessageStarted {
            message_id: tact_agent_api::MessageId::new("a1"),
            role: AgentRole::Assistant,
        },
    )])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("terminal"));
    assert!(
        state
            .transcript
            .iter()
            .all(|message| message.role != AgentRole::Assistant)
    );
}

#[tokio::test]
async fn terminal_message_id_must_match_started_id() {
    let response = text_response("a2", "done", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: tact_agent_api::MessageId::new("a1"),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let error = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("message id"));
}

#[tokio::test]
async fn auto_compaction_records_model_usage_before_retrying_preparation() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    model.push_complete(events(text_response(
        "compact",
        "summary",
        StopReason::EndTurn,
    )));
    let context = ScriptedContext::new();
    context.set_compaction_model_calls(1);
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    let records = h.usage.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.kind == ModelCallKind::LocalCompact)
    );
}

#[tokio::test]
async fn compaction_usage_assigns_last_token_total_from_total_tokens() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let mut compact_response = text_response("compact", "summary", StopReason::EndTurn);
    compact_response.usage = Some(TokenUsage {
        input_tokens: 12,
        output_tokens: 3,
        total_tokens: 91,
        ..TokenUsage::default()
    });
    model.push_complete(events(compact_response));
    let context = ScriptedContext::new();
    context.set_compaction_model_calls(1);
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    h.kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(state.last_token_total, Some(91));
}

#[tokio::test]
async fn compaction_commit_callback_runs_after_the_replacement_is_persisted() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context.clone(),
    );
    let mut state = state();

    h.kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(
        context.compaction_commits(),
        [CompactionRequest::Auto { focus: None }]
    );
}

#[tokio::test]
async fn failed_replacement_never_acknowledges_compaction_commit() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Auto { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: Some(Vec::new()),
        cursor: CursorUpdate::Unchanged,
    }));
    let store = FailingTranscriptStore::new(None, Some(1));
    let kernel = AgentKernel::new(KernelDeps {
        model,
        tools: ScriptedTools::new(Vec::new()),
        gate: ScriptedGate::new(),
        context: context.clone(),
        transcript: store.clone(),
        usage: Arc::new(RecordingUsageSink::new()),
        events: Arc::new(RecordingEventPort::new()),
    });
    let mut state = state();

    let error = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("forced replace failure"));
    assert!(context.compaction_commits().is_empty());
    assert_eq!(store.transcript(&session()).len(), 1);
}

#[tokio::test]
async fn native_compaction_records_native_usage_and_diagnostics() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    model.push_native_compaction(events(text_response(
        "compact",
        "native summary",
        StopReason::EndTurn,
    )));
    let context = ScriptedContext::new();
    context.set_native_compaction_calls(1);
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Command { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Replace { cursor: None },
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let records = h.usage.records();
    assert_eq!(records.len(), 2);
    assert!(
        records
            .iter()
            .any(|record| record.kind == ModelCallKind::NativeCompact)
    );
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::Usage {
            kind: ModelCallKind::NativeCompact,
            ..
        }
    )));
}

#[tokio::test]
async fn every_preparation_compaction_request_uses_the_typed_protocol() {
    for request in [
        CompactionRequest::Auto { focus: None },
        CompactionRequest::Manual {
            focus: Some("manual".into()),
        },
        CompactionRequest::Command {
            focus: Some("command".into()),
        },
        CompactionRequest::Micro,
    ] {
        let model = ScriptedModel::new(vec![events(text_response(
            "a1",
            "done",
            StopReason::EndTurn,
        ))]);
        let context = ScriptedContext::new();
        context.push_request(RequestPreparation::Compact {
            request: request.clone(),
            if_skipped: Vec::new(),
        });
        context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
            transcript: None,
            cursor: CursorUpdate::Unchanged,
        }));
        let kind = request.kind();
        let h = harness(
            model,
            ScriptedTools::new(Vec::new()),
            ScriptedGate::new(),
            context.clone(),
        );
        let mut state = state();

        let _ = h
            .kernel
            .run(&mut state, input("go"), CancelToken::new())
            .await
            .unwrap();

        assert_eq!(context.compact_requests(), [request]);
        assert_eq!(context.prepare_request_calls(), 2);
        assert!(h.events.events().iter().any(|event| matches!(
            event,
            AgentEvent::ContextCompacted { kind: observed } if *observed == kind
        )));
    }
}

#[tokio::test]
async fn skipped_compaction_uses_fallback_without_looping() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Micro,
        if_skipped: vec![AgentMessage::text(
            AgentRole::User,
            MessageOrigin::InjectedContext {
                source: "fallback".into(),
            },
            "fallback context",
        )],
    });
    context.push_compaction(CompactionOutcome::Skipped {
        reason: "not needed".into(),
    });
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.context.compact_requests(), [CompactionRequest::Micro]);
    assert!(
        h.model.requests()[0]
            .messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                AgentContentBlock::Text { text } if text == "fallback context"
            ))
    );
}

#[tokio::test]
async fn native_cursor_only_replacement_retains_the_transcript() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let cursor = ModelCursor {
        provider: "fake".into(),
        version: 1,
        payload: serde_json::json!({"compacted": true}),
    };
    let context = ScriptedContext::new();
    context.push_request(RequestPreparation::Compact {
        request: CompactionRequest::Command { focus: None },
        if_skipped: Vec::new(),
    });
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Replace {
            cursor: Some(cursor.clone()),
        },
    }));
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        context,
    );
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(state.cursor, Some(cursor));
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::User && message.text_content() == "go"
    }));
}

#[tokio::test]
async fn replacement_retain_clear_and_unchanged_are_distinguishable() {
    let (retain_calls, retain_state) =
        replacement_case(None, CursorUpdate::Replace { cursor: None }).await;
    let (clear_calls, clear_state) =
        replacement_case(Some(Vec::new()), CursorUpdate::Replace { cursor: None }).await;
    let (unchanged_calls, unchanged_state) = replacement_case(None, CursorUpdate::Unchanged).await;

    assert!(retain_calls.iter().any(|call| matches!(
        call,
        StoreCall::Replace {
            transcript: None,
            cursor: CursorUpdate::Replace { cursor: None },
        }
    )));
    assert!(clear_calls.iter().any(|call| matches!(
        call,
        StoreCall::Replace {
            transcript: Some(messages),
            cursor: CursorUpdate::Replace { cursor: None },
        } if messages.is_empty()
    )));
    assert!(unchanged_calls.iter().any(|call| matches!(
        call,
        StoreCall::Replace {
            transcript: None,
            cursor: CursorUpdate::Unchanged,
        }
    )));
    assert!(
        retain_state
            .transcript
            .iter()
            .any(|message| message.text_content() == "go")
    );
    assert!(
        !clear_state
            .transcript
            .iter()
            .any(|message| message.text_content() == "go")
    );
    assert!(
        unchanged_state
            .transcript
            .iter()
            .any(|message| message.text_content() == "go")
    );
}
