use super::*;

#[tokio::test]
async fn max_tokens_with_tools_executes_pairing_then_continues() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::MaxTokens,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("tool result")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.recovery.continuation_attempts, 1);
    assert_eq!(h.tools.executions(), ["tool"]);
    assert!(
        state
            .transcript
            .iter()
            .any(|message| { message.origin == MessageOrigin::Continuation })
    );
    assert_eq!(h.model.stream_calls(), 2);
    let second_request = &h.model.requests()[1];
    let assistant_index = second_request
        .messages
        .iter()
        .position(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, AgentContentBlock::ToolUse { .. }))
        })
        .expect("assistant tool use");
    assert!(
        second_request.messages[assistant_index + 1]
            .content
            .iter()
            .any(|block| matches!(
                block,
                AgentContentBlock::ToolResult { tool_use_id, .. } if tool_use_id == "call-1"
            ))
    );
}

#[tokio::test]
async fn max_tokens_with_tools_runs_manual_compaction_before_continuation() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "compact")],
            StopReason::MaxTokens,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "compact",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::RequestCompaction {
                focus: Some("manual".into()),
            },
        )),
    )]);
    let context = ScriptedContext::new();
    context.push_compaction(CompactionOutcome::Replaced(ContextReplacement {
        transcript: None,
        cursor: CursorUpdate::Unchanged,
    }));
    let h = harness(model, tools, ScriptedGate::new(), context.clone());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(
        context.compact_requests(),
        [CompactionRequest::Manual {
            focus: Some("manual".into())
        }]
    );
    assert_eq!(h.model.stream_calls(), 2);
    assert!(
        state
            .transcript
            .iter()
            .any(|message| message.origin == MessageOrigin::Continuation)
    );
}

#[tokio::test]
async fn exhausted_max_tokens_synthesizes_truncated_results() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::MaxTokens,
        )),
        events(tool_response(
            "a2",
            &[tool_call("call-2", "tool")],
            StopReason::MaxTokens,
        )),
        events(tool_response(
            "a3",
            &[tool_call("call-3", "tool")],
            StopReason::MaxTokens,
        )),
        events(tool_response(
            "a4",
            &[tool_call("call-4", "tool")],
            StopReason::MaxTokens,
        )),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("tool result")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTokens);
    assert_eq!(outcome.recovery.continuation_attempts, 3);
    assert_eq!(h.tools.executions(), ["tool", "tool", "tool"]);
    assert_eq!(h.model.stream_calls(), 4);
    assert!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin == MessageOrigin::ToolResult)
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                AgentContentBlock::ToolResult {
                    tool_use_id,
                    content,
                } if tool_use_id == "call-4" && content.contains("truncated")
            ))
    );
}

#[tokio::test]
async fn a_truncated_final_turn_cannot_complete_a_goal() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "goal")],
        StopReason::MaxTokens,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "goal",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::CompleteRun,
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let mut input = input("finish");
    input.max_total_turns = Some(1);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::MaxTurns);
    assert_eq!(h.tools.executions(), ["goal"]);
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn goal_completion_only_ends_on_the_final_permitted_turn() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "goal")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "goal",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::CompleteRun,
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let mut input = input("finish");
    input.max_total_turns = Some(1);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn non_final_goal_completion_continues_the_activation() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "goal")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continue", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "goal",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Succeeded,
            ToolDirective::CompleteRun,
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let mut input = input("finish");
    input.max_total_turns = Some(2);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.model.stream_calls(), 2);
}

#[tokio::test]
async fn yield_to_host_suppresses_later_waves() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "yield"), tool_call("call-2", "later")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        ToolDefinition::new(
            "yield",
            ResourceAccess::barrier(),
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::YieldToHost {
                    reason: HostYieldReason::NeedsUser,
                },
            )),
        ),
        tool("later", ToolBehavior::Immediate(success("later"))),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(
        outcome.stop,
        RunStop::YieldToHost {
            reason: HostYieldReason::NeedsUser
        }
    );
    assert_eq!(h.tools.executions(), ["yield"]);
}

#[tokio::test]
async fn terminal_directive_precedence_is_independent_of_completion_order() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[
            tool_call("call-1", "complete"),
            tool_call("call-2", "yield"),
            tool_call("call-3", "compact"),
        ],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        tool(
            "complete",
            ToolBehavior::Delayed {
                delay: Duration::from_millis(30),
                result: with_directive(ToolExecutionStatus::Succeeded, ToolDirective::CompleteRun),
            },
        ),
        tool(
            "yield",
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::YieldToHost {
                    reason: HostYieldReason::NeedsUser,
                },
            )),
        ),
        tool(
            "compact",
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::RequestCompaction {
                    focus: Some("deferred".into()),
                },
            )),
        ),
    ]);
    let context = ScriptedContext::new();
    let h = harness(model, tools, ScriptedGate::new(), context.clone());
    let mut state = state();
    let mut input = input("go");
    input.max_total_turns = Some(1);

    let outcome = h
        .kernel
        .run(&mut state, input, CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.tools.executions().len(), 3);
    assert!(context.compact_requests().is_empty());
    assert_eq!(h.model.stream_calls(), 1);
}

#[tokio::test]
async fn cancellation_outranks_other_wave_directives() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[
            tool_call("call-1", "cancel"),
            tool_call("call-2", "complete"),
            tool_call("call-3", "yield"),
        ],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        tool(
            "cancel",
            ToolBehavior::CancelAndResult(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::RequestCompaction {
                    focus: Some("cancelled".into()),
                },
            )),
        ),
        tool(
            "complete",
            ToolBehavior::Delayed {
                delay: Duration::from_millis(20),
                result: with_directive(ToolExecutionStatus::Succeeded, ToolDirective::CompleteRun),
            },
        ),
        tool(
            "yield",
            ToolBehavior::Immediate(with_directive(
                ToolExecutionStatus::Succeeded,
                ToolDirective::YieldToHost {
                    reason: HostYieldReason::NeedsUser,
                },
            )),
        ),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions().len(), 3);
}

#[tokio::test]
async fn provider_activity_is_not_treated_as_a_tool_call() {
    let response = text_response("a1", "answer", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![ModelScript::Events(vec![
        Ok(ModelEvent::ProviderActivity {
            activity: ProviderActivity {
                id: "search".into(),
                step: 0,
                kind: ProviderActivityKind::WebSearch {
                    phase: ProviderActivityPhase::Completed,
                    query: Some("rust".into()),
                    failure: None,
                    duration: Some(Duration::from_millis(2)),
                },
            },
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

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(h.tools.executions().is_empty());
}

#[tokio::test]
async fn event_port_runs_before_observers_and_emits_one_terminal_event() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "done",
        StopReason::EndTurn,
    ))]);
    let tools = ScriptedTools::new(Vec::new());
    let gate = ScriptedGate::new();
    let context = ScriptedContext::new();
    let store = Arc::new(InMemoryTranscriptStore::new());
    let usage = Arc::new(RecordingUsageSink::new());
    let events = Arc::new(RecordingEventPort::new());
    let observer = RecordingObserver::new();
    let second_observer = RecordingObserver::new();
    let kernel = AgentKernel::with_extensions(
        KernelDeps {
            model,
            tools,
            gate,
            context,
            transcript: store,
            usage,
            events: events.clone(),
        },
        KernelExtensions {
            observers: vec![observer.clone(), second_observer.clone()],
        },
    );
    let mut state = state();

    let _ = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(events.events(), observer.events());
    assert_eq!(observer.events(), second_observer.events());
    let terminal = observer
        .events()
        .iter()
        .filter(|event| {
            matches!(
                event,
                AgentEvent::RunFinished { .. } | AgentEvent::RunFailed { .. }
            )
        })
        .count();
    assert_eq!(terminal, 1);
}

#[tokio::test]
async fn event_port_precedes_observers_in_registration_order() {
    let trace = Arc::new(Mutex::new(Vec::new()));
    let kernel = AgentKernel::with_extensions(
        KernelDeps {
            model: ScriptedModel::new(vec![events(text_response(
                "a1",
                "done",
                StopReason::EndTurn,
            ))]),
            tools: ScriptedTools::new(Vec::new()),
            gate: ScriptedGate::new(),
            context: ScriptedContext::new(),
            transcript: Arc::new(InMemoryTranscriptStore::new()),
            usage: Arc::new(RecordingUsageSink::new()),
            events: Arc::new(TraceEventPort {
                trace: trace.clone(),
            }),
        },
        KernelExtensions {
            observers: vec![
                Arc::new(TraceObserver {
                    name: "first",
                    trace: trace.clone(),
                }),
                Arc::new(TraceObserver {
                    name: "second",
                    trace: trace.clone(),
                }),
            ],
        },
    );
    let mut state = state();

    let _ = kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let trace = trace.lock().expect("event trace lock");
    assert!(!trace.is_empty());
    assert!(
        trace
            .as_chunks::<3>()
            .0
            .iter()
            .all(|group| *group == ["port", "first", "second"])
    );
}
