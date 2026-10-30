use super::*;

#[tokio::test]
async fn cancellation_before_stream_consumes_no_turn() {
    let model = ScriptedModel::new(Vec::new());
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();
    let cancel = CancelToken::new();
    cancel.cancel();

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(outcome.turns, 0);
    assert_eq!(h.model.stream_calls(), 0);
}

#[tokio::test]
async fn cancellation_during_incomplete_stream_commits_nothing() {
    let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(vec![Ok(
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
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(
        state
            .transcript
            .iter()
            .all(|message| message.role != AgentRole::Assistant)
    );
}

#[tokio::test]
async fn cancellation_after_complete_response_commits_assistant() {
    let response = text_response("a1", "complete", StopReason::EndTurn);
    let events = vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ];
    let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(events)]);
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

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(
        state
            .transcript
            .iter()
            .any(|message| message.role == AgentRole::Assistant)
    );
}

#[tokio::test]
async fn cancellation_after_complete_tool_response_pairs_every_call() {
    let response = tool_response(
        "a1",
        &[tool_call("call-1", "tool"), tool_call("call-2", "tool")],
        StopReason::ToolUse,
    );
    let model = ScriptedModel::new(vec![ModelScript::WaitForCancel(vec![
        Ok(ModelEvent::MessageStarted {
            message_id: response.message_id.clone(),
            role: AgentRole::Assistant,
        }),
        Ok(ModelEvent::Finished(response)),
    ])]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("must not run")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(h.tools.executions().is_empty());
    let grouped = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped cancellation result");
    assert_eq!(
        grouped.tool_result_ids().collect::<Vec<_>>(),
        ["call-1", "call-2"]
    );
}

#[tokio::test]
async fn cancellation_during_gate_approval_never_executes_the_tool() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("should not run")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::WaitForCancel);
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert!(h.tools.executions().is_empty());
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-1")
    }));
}

#[tokio::test]
async fn cancellation_waits_for_in_flight_tool_and_pairs_results() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "tool")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::WaitForCancel(success("stopped")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions(), ["tool"]);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-1")
    }));
}

#[tokio::test]
async fn cancellation_between_waves_suppresses_later_calls() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "slow"), tool_call("call-2", "later")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![
        ToolDefinition::new(
            "slow",
            ResourceAccess::barrier(),
            ToolBehavior::Delayed {
                delay: Duration::from_millis(40),
                result: success("slow"),
            },
        ),
        ToolDefinition::new(
            "later",
            ResourceAccess::barrier(),
            ToolBehavior::Immediate(success("later")),
        ),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        cancel_task.cancel();
    });

    let outcome = h.kernel.run(&mut state, input("go"), cancel).await.unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions(), ["slow"]);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-2")
    }));
}

#[tokio::test]
async fn repeated_cancellation_has_one_terminal_event_and_bounded_drain() {
    let model = ScriptedModel::new(vec![events(tool_response(
        "a1",
        &[tool_call("call-1", "slow")],
        StopReason::ToolUse,
    ))]);
    let tools = ScriptedTools::new(vec![tool(
        "slow",
        ToolBehavior::WaitForCancel(success("stopped")),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();
    let cancel = CancelToken::new();
    let cancel_task = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(10)).await;
        for _ in 0..32 {
            cancel_task.cancel();
            tokio::task::yield_now().await;
        }
    });

    let outcome = tokio::time::timeout(
        Duration::from_secs(1),
        h.kernel.run(&mut state, input("go"), cancel),
    )
    .await
    .expect("cancel drain must be bounded")
    .unwrap();

    assert_eq!(outcome.stop, RunStop::Cancelled);
    assert_eq!(h.tools.executions(), ["slow"]);
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|id| id == "call-1")
    }));

    let events = h.events.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(
                event,
                AgentEvent::RunFinished { .. } | AgentEvent::RunFailed { .. }
            ))
            .count(),
        1
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::RunFinished { .. })
    ));
    assert!(h.usage.flush_count() >= 1);
}
