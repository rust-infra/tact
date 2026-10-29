use super::*;

#[tokio::test]
async fn end_turn_text_stream_completes() {
    let model = ScriptedModel::new(vec![events(text_response(
        "a1",
        "hello",
        StopReason::EndTurn,
    ))]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("hi"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.total_turns, 1);
    assert_eq!(state.transcript.len(), 2);
    assert_eq!(state.transcript[1].origin, MessageOrigin::Assistant);
    assert_eq!(h.store.transcript(&session()), state.transcript);
    assert_eq!(h.usage.records().len(), 1);
    assert!(h.usage.flush_count() >= 1);
}

#[tokio::test]
async fn ordinary_usage_assigns_last_token_total_from_total_tokens() {
    let mut response = text_response("a1", "hello", StopReason::EndTurn);
    response.usage = Some(TokenUsage {
        input_tokens: 9,
        output_tokens: 4,
        total_tokens: 37,
        ..TokenUsage::default()
    });
    let model = ScriptedModel::new(vec![events(response)]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    h.kernel
        .run(&mut state, input("hi"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(state.last_token_total, Some(37));
    let record = &h.usage.records()[0];
    let run = record.run.as_ref().expect("usage keeps the run context");
    assert_eq!(run.session.as_str(), "test-session");
    assert_eq!(run.run_id.as_str(), "test-run");
}

#[tokio::test]
async fn parallel_tool_results_preserve_assistant_source_order() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("slow", "slow"), tool_call("fast", "fast")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![
        tool(
            "slow",
            ToolBehavior::Delayed {
                delay: Duration::from_millis(30),
                result: success("slow result"),
            },
        ),
        tool("fast", ToolBehavior::Immediate(success("fast result"))),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(outcome.tool_calls, 2);
    let result_message = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped result");
    assert_eq!(
        result_message.tool_result_ids().collect::<Vec<_>>(),
        ["slow", "fast"]
    );
    let results = result_message
        .content
        .iter()
        .filter_map(|block| match block {
            AgentContentBlock::ToolResult { content, .. } => Some(content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(results, ["slow result", "fast result"]);

    let requests = h.model.requests();
    let second = requests.get(1).expect("second request");
    let model_results = second
        .messages
        .iter()
        .flat_map(|message| &message.content)
        .filter_map(|block| match block {
            AgentContentBlock::ToolResult {
                tool_use_id,
                content,
            } => Some((tool_use_id.as_str(), content.as_str())),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        model_results,
        [("slow", "slow result"), ("fast", "fast result")]
    );
}

#[tokio::test]
async fn image_bearing_tool_result_keeps_companion_image_after_its_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-image", "screenshot")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let image = AgentContentBlock::Image {
        source: AgentImageSource {
            type_: "base64".into(),
            media_type: "image/png".into(),
            data: "AAAA".into(),
        },
    };
    let result = ToolExecutionResult::success(ToolOutcome {
        content: vec![
            AgentContentBlock::Text {
                text: "preview".into(),
            },
            image.clone(),
        ],
        directives: Vec::new(),
        metadata: None,
    });
    let tools = ScriptedTools::new(vec![tool(
        "screenshot",
        ToolBehavior::Immediate(result.clone()),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    h.kernel
        .run(&mut state, input("capture"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(
        result.outcome.content,
        [result.outcome.content[0].clone(), image.clone()]
    );
    let finished = tool_finished(&h, "call-image");
    assert_eq!(finished.outcome.content, result.outcome.content);

    let grouped = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped image result");
    assert_eq!(
        grouped.content,
        [
            AgentContentBlock::ToolResult {
                tool_use_id: "call-image".into(),
                content: "preview".into(),
            },
            image,
        ]
    );
}

#[tokio::test]
async fn mixed_tool_batch_keeps_each_image_next_to_its_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[
                tool_call("call-image", "screenshot"),
                tool_call("call-text", "lookup"),
            ],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let image = AgentContentBlock::Image {
        source: AgentImageSource {
            type_: "base64".into(),
            media_type: "image/png".into(),
            data: "BBBB".into(),
        },
    };
    let tools = ScriptedTools::new(vec![
        tool(
            "screenshot",
            ToolBehavior::Immediate(ToolExecutionResult::success(ToolOutcome {
                content: vec![
                    AgentContentBlock::Text {
                        text: "preview".into(),
                    },
                    image.clone(),
                ],
                directives: Vec::new(),
                metadata: None,
            })),
        ),
        tool("lookup", ToolBehavior::Immediate(success("lookup result"))),
    ]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    h.kernel
        .run(&mut state, input("capture both"), CancelToken::new())
        .await
        .unwrap();

    let expected = vec![
        AgentContentBlock::ToolResult {
            tool_use_id: "call-image".into(),
            content: "preview".into(),
        },
        image,
        AgentContentBlock::ToolResult {
            tool_use_id: "call-text".into(),
            content: "lookup result".into(),
        },
    ];
    let grouped = state
        .transcript
        .iter()
        .find(|message| message.origin == MessageOrigin::ToolResult)
        .expect("grouped tool result");
    assert_eq!(grouped.content, expected);

    let second_request = &h.model.requests()[1];
    let request_result = second_request
        .messages
        .iter()
        .find(|message| {
            message
                .content
                .iter()
                .any(|block| matches!(block, AgentContentBlock::ToolResult { .. }))
        })
        .expect("the next model request keeps the grouped result");
    assert_eq!(request_result.content, expected);
}

#[tokio::test]
async fn gate_rewrite_is_re_resolved_before_execution() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "benign")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "credential",
        ToolBehavior::Immediate(success("secret")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::ResolveAs {
        name: "credential".into(),
        label: Some("high risk".into()),
    });
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.tools.executions(), ["credential"]);
    assert_eq!(h.gate.after_calls()[0].name, "credential");
}

#[tokio::test]
async fn gate_rewrite_uses_the_resolved_access_for_wave_scheduling() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "benign"), tool_call("call-2", "read")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![
        ToolDefinition::new(
            "benign",
            ResourceAccess::independent(),
            ToolBehavior::Immediate(success("benign")),
        ),
        ToolDefinition::new(
            "barrier",
            ResourceAccess::barrier(),
            ToolBehavior::Immediate(success("barrier")),
        ),
        tool("read", ToolBehavior::Immediate(success("read"))),
    ]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::ResolveAs {
        name: "barrier".into(),
        label: Some("rewritten".into()),
    });
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert_eq!(h.tools.executions(), ["barrier", "read"]);
    assert_eq!(h.gate.after_calls()[0].name, "barrier");
    assert!(h.events.events().iter().any(|event| matches!(
        event,
        AgentEvent::ToolFinished {
            call_id,
            permission_label: Some(label),
            ..
        } if call_id == "call-1" && label == "rewritten"
    )));
}

#[tokio::test]
async fn denied_tool_never_executes_but_gets_a_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("should not run")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_before(BeforeAction::Deny("no".into()));
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    assert!(h.tools.executions().is_empty());
    assert!(state.transcript.iter().any(|message| {
        message.origin == MessageOrigin::ToolResult
            && message.tool_result_ids().any(|tool_id| tool_id == "call-1")
    }));
}

#[tokio::test]
async fn ordinary_failure_preserves_status_and_operational_error() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool("tool", ToolBehavior::Immediate(failure("boom")))]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    assert_eq!(
        finished.error.as_ref().map(|error| error.message.as_str()),
        Some("boom")
    );
    assert!(
        state
            .transcript
            .iter()
            .filter(|message| message.origin == MessageOrigin::ToolResult)
            .flat_map(|message| &message.content)
            .any(|block| matches!(
                block,
                AgentContentBlock::ToolResult { content, .. } if content == "boom"
            ))
    );
}

#[tokio::test]
async fn adapter_error_becomes_a_typed_failed_result() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Error(tact_agent_api::ToolError::new(
            tact_agent_api::ToolErrorKind::Adapter,
            "adapter exploded",
        )),
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    assert_eq!(
        finished.error.as_ref().map(|error| error.kind),
        Some(ToolExecutionErrorKind::Adapter)
    );
}

#[tokio::test]
async fn post_tool_rewrite_cannot_turn_failure_into_success() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(with_directive(
            ToolExecutionStatus::Failed,
            ToolDirective::YieldToHost {
                reason: HostYieldReason::NeedsUser,
            },
        )),
    )]);
    let gate = ScriptedGate::new();
    let mut rewritten = success("rewritten while failed");
    rewritten
        .outcome
        .directives
        .push(ToolDirective::CompleteRun);
    gate.push_after(AfterAction::Replace(rewritten));
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let outcome = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    assert_eq!(outcome.stop, RunStop::Completed);
    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    assert_eq!(
        finished.error.as_ref().map(|error| error.message.as_str()),
        Some("failed")
    );
    assert!(finished.outcome.directives.is_empty());
    assert_eq!(h.model.stream_calls(), 2);
}

#[tokio::test]
async fn post_tool_gate_failure_never_restores_success() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "continued", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Immediate(success("visible success")),
    )]);
    let gate = ScriptedGate::new();
    gate.push_after(AfterAction::Error("post processing failed".into()));
    let h = harness(model, tools, gate, ScriptedContext::new());
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let finished = tool_finished(&h, "call-1");
    assert_eq!(finished.status, ToolExecutionStatus::Failed);
    let error = finished.error.expect("gate failure");
    assert_eq!(error.kind, ToolExecutionErrorKind::Adapter);
    assert!(error.message.contains("post processing failed"));
}

#[tokio::test]
async fn failed_result_clears_terminal_directives() {
    for directive in [
        ToolDirective::CompleteRun,
        ToolDirective::YieldToHost {
            reason: HostYieldReason::NeedsUser,
        },
        ToolDirective::RequestCompaction {
            focus: Some("failed request".into()),
        },
    ] {
        let model = ScriptedModel::new(vec![
            events(tool_response(
                "a1",
                &[tool_call("call-1", "tool")],
                StopReason::ToolUse,
            )),
            events(text_response("a2", "continued", StopReason::EndTurn)),
        ]);
        let tools = ScriptedTools::new(vec![tool(
            "tool",
            ToolBehavior::Immediate(with_directive(ToolExecutionStatus::Failed, directive)),
        )]);
        let context = ScriptedContext::new();
        let h = harness(model, tools, ScriptedGate::new(), context.clone());
        let mut state = state();

        let outcome = h
            .kernel
            .run(&mut state, input("go"), CancelToken::new())
            .await
            .unwrap();

        assert_eq!(outcome.stop, RunStop::Completed);
        assert_eq!(h.model.stream_calls(), 2);
        assert!(context.compact_requests().is_empty());
    }
}

#[tokio::test]
async fn tool_progress_is_forwarded_in_order() {
    let model = ScriptedModel::new(vec![
        events(tool_response(
            "a1",
            &[tool_call("call-1", "tool")],
            StopReason::ToolUse,
        )),
        events(text_response("a2", "done", StopReason::EndTurn)),
    ]);
    let tools = ScriptedTools::new(vec![tool(
        "tool",
        ToolBehavior::Progress {
            chunks: vec![
                ToolOutputChunk {
                    stream: ToolOutputStream::Stdout,
                    text: "one".into(),
                },
                ToolOutputChunk {
                    stream: ToolOutputStream::Stderr,
                    text: "two".into(),
                },
            ],
            result: success("done"),
        },
    )]);
    let h = harness(model, tools, ScriptedGate::new(), ScriptedContext::new());
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let progress = h
        .events
        .events()
        .into_iter()
        .find_map(|event| match event {
            AgentEvent::ToolProgress { call_id, chunks } => Some((call_id, chunks)),
            _ => None,
        })
        .expect("progress event");
    assert_eq!(progress.0, "call-1");
    assert_eq!(progress.1[0].text, "one");
    assert_eq!(progress.1[1].text, "two");
}

#[tokio::test]
async fn thinking_lifecycle_is_forwarded_as_deltas() {
    let response = text_response("a1", "answer", StopReason::EndTurn);
    let model = ScriptedModel::new(vec![events_with_deltas(
        response,
        vec![
            MessageDelta::Text {
                delta: r#"{"tool":"not-a-call"}"#.into(),
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Started,
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Delta {
                    delta: "reason".into(),
                },
            },
            MessageDelta::Thinking {
                phase: ThinkingDelta::Finished,
            },
        ],
    )]);
    let h = harness(
        model,
        ScriptedTools::new(Vec::new()),
        ScriptedGate::new(),
        ScriptedContext::new(),
    );
    let mut state = state();

    let _ = h
        .kernel
        .run(&mut state, input("go"), CancelToken::new())
        .await
        .unwrap();

    let deltas = h
        .events
        .events()
        .into_iter()
        .filter_map(|event| match event {
            AgentEvent::MessageDelta { message_id, delta } => Some((message_id, delta)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(deltas.len(), 4);
    assert!(
        deltas
            .iter()
            .all(|(message_id, _)| message_id.as_str() == "a1")
    );
    assert!(h.tools.executions().is_empty());
}
