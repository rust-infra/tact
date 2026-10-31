#![allow(clippy::result_large_err)]

use super::*;

#[tokio::test]
async fn agent_loop_completes_with_end_turn_response() {
    ensure_config();
    let context = test_context("agent_loop_end_turn");
    let router = crate::tool::toolset();
    let mcp = crate::mcp::MCPToolRouter::new();
    let perm =
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap();

    let mock = MockClient::new(vec![(
        vec![make_text_block("Hello, I am a coding agent.")],
        Some(StopReason::EndTurn),
    )]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        router,
        mcp,
        perm,
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    );

    let result = agent
        .agent_loop(Some(Message::new_text(Role::User, "Say hello")))
        .await;

    assert!(
        result.is_ok(),
        "agent_loop should complete: {:?}",
        result.err()
    );
    assert!(
        agent.runtime.context.len() >= 2,
        "context should have at least user + assistant messages"
    );
}

#[tokio::test]
async fn kernel_activation_uses_direct_facade_context_mutations() {
    ensure_config();
    const DIRECT: &str = "DIRECT FACADE CONTEXT";
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_for_responder = seen.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        *seen_for_responder.lock().unwrap() = request
            .messages
            .iter()
            .map(|message| crate::extract_text(&message.content))
            .collect();
        Ok((
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("direct_context_mutation"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    agent.runtime.context = vec![Message::new_text(Role::User, DIRECT)];

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "next turn")))
        .await
        .expect("kernel activation completes");
    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(outcome.turns, 1);

    let seen = seen.lock().unwrap();
    assert_eq!(seen.first().map(String::as_str), Some(DIRECT));
    assert!(seen.iter().any(|message| message == "next turn"));
    drop(seen);
    assert!(
        agent
            .runtime
            .context
            .iter()
            .any(|message| crate::extract_text(&message.content) == DIRECT),
        "the facade context must survive activation restore"
    );
}

#[tokio::test]
async fn kernel_composition_uses_current_facade_model_and_sink_each_activation() {
    ensure_config();
    let first_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let first_calls_for_responder = first_calls.clone();
    let first = MockClient::with_responder(move |_request, _idx| {
        first_calls_for_responder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((
            vec![make_text_block("first model")],
            Some(StopReason::EndTurn),
            None,
        ))
    });
    let second_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let second_calls_for_responder = second_calls.clone();
    let second = MockClient::with_responder(move |_request, _idx| {
        second_calls_for_responder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok((
            vec![make_text_block("second model")],
            Some(StopReason::EndTurn),
            None,
        ))
    });

    let (first_tx, mut first_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut agent = Agent::new(
        LlmProvider::Mock(first),
        test_context("current_composition"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(first_tx);
    agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "one")))
        .await
        .expect("first activation");
    assert_eq!(first_calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    let first_updates = std::iter::from_fn(|| first_rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(matches!(
        first_updates.first(),
        Some(AgentUpdate::RunStarted { .. })
    ));
    assert!(matches!(
        first_updates
            .iter()
            .find(|update| matches!(update, AgentUpdate::TurnStats { .. })),
        Some(AgentUpdate::TurnStats { .. })
    ));

    let (second_tx, mut second_rx) = tokio::sync::mpsc::unbounded_channel();
    agent.runtime.client = LlmProvider::Mock(second);
    agent = agent.with_ui_channel(second_tx);
    agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "two")))
        .await
        .expect("second activation");

    assert_eq!(
        second_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the next activation must use the replacement facade model"
    );
    assert!(
        agent
            .runtime
            .context
            .iter()
            .any(|message| crate::extract_text(&message.content) == "second model")
    );
    let second_updates = std::iter::from_fn(|| second_rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        second_updates
            .iter()
            .any(|update| matches!(update, AgentUpdate::TurnStats { .. }))
    );
    assert!(
        agent.runtime.events.has_ui(),
        "the replacement answering sink must remain available"
    );
}

#[tokio::test]
async fn empty_facade_context_loads_sqlite_history_before_kernel_transfer() {
    ensure_config();
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();
    let history = vec![
        Message::new_text(Role::User, "PERSISTED USER"),
        Message::new_text(Role::Assistant, "PERSISTED ASSISTANT"),
    ];
    store
        .replace_session_messages("session-1", &history)
        .await
        .unwrap();

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen_for_responder = seen.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        *seen_for_responder.lock().unwrap() = request
            .messages
            .iter()
            .map(|message| crate::extract_text(&message.content))
            .collect();
        Ok((
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("resume_before_transfer"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_session("session-1".to_string(), store);
    assert!(agent.runtime.context.is_empty());

    agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "new turn")))
        .await
        .expect("resumed kernel activation completes");
    assert!(agent.kernel_activations_for_test() >= 1);

    let seen = seen.lock().unwrap();
    assert!(seen.iter().any(|message| message == "PERSISTED USER"));
    assert!(seen.iter().any(|message| message == "PERSISTED ASSISTANT"));
    assert!(seen.iter().any(|message| message == "new turn"));
}

#[tokio::test]
async fn agent_loop_goal_request_input_yields_without_second_model_call() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("agent_loop_goal_request_input_yield");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Need user input".to_string(),
            objective: "Pause for a user answer".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "answer received",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id.clone());

    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_responder = request_count.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        request_count_for_responder.fetch_add(1, Ordering::SeqCst);
        if idx == 0 {
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "goal-input-1".to_string(),
                    name: "goal_request_input".to_string(),
                    input: serde_json::json!({"question": "Which option should I use?"}),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        } else {
            panic!("unexpected second request at responder index {idx}");
        }
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "continue the goal")))
        .await
        .expect("agent_loop_detailed should yield cleanly for user input");

    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(
        outcome.stop,
        AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
    );
    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.runtime.goal_yield_reason,
        Some(GoalYieldReason::NeedsUser)
    );

    let events = agent
        .tool_context
        .goal_manager
        .list_events(&goal.id)
        .await
        .unwrap();
    assert!(
        events
            .iter()
            .any(|event| event.kind == crate::goal::GoalEventKind::NeedsUser),
        "needs_user event should be persisted: {events:?}"
    );
    assert_eq!(
        tool_result_for(&agent.runtime.context, "goal-input-1").as_deref(),
        Some("User input requested; yield to Runner.")
    );
}

/// The hard cap is checked before the model call, so the model's final
/// permitted turn is the only one that can still persist a checkpoint.
/// The reminder must appear there (and only there), scoped to a Goal
/// activation — a subagent also runs under `max_turns` but has no Goal.
#[tokio::test]
async fn agent_loop_reminds_a_goal_activation_to_checkpoint_on_its_last_turn() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("agent_loop_goal_final_turn_reminder");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Reminder".to_string(),
            objective: "Checkpoint before the cap".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "checkpoint written",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id.clone());

    let systems = Arc::new(std::sync::Mutex::new(Vec::<Option<String>>::new()));
    let systems_for_responder = systems.clone();
    let index = Arc::new(AtomicUsize::new(0));
    let index_for_responder = index.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        systems_for_responder
            .lock()
            .unwrap()
            .push(request.system.clone());
        let call = index_for_responder.fetch_add(1, Ordering::SeqCst);
        if call == 0 {
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "checkpoint-1".to_string(),
                    name: "goal_checkpoint".to_string(),
                    input: serde_json::json!({"summary": "half done"}),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        } else {
            Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ))
        }
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_max_turns(Some(2));

    agent
        .agent_loop(Some(Message::new_text(Role::User, "work the goal")))
        .await
        .expect("agent_loop should finish");

    assert!(agent.kernel_activations_for_test() >= 1);
    let systems = systems.lock().unwrap();
    assert_eq!(systems.len(), 2, "expected exactly two model calls");
    let first = systems[0].as_deref().unwrap_or_default();
    assert!(
        !first.contains("goal-budget-reminder"),
        "the reminder must not appear before the final turn"
    );
    let last = systems[1].as_deref().unwrap_or_default();
    assert!(
        last.contains("goal-budget-reminder"),
        "the final Goal turn must carry the reminder:\n{last}"
    );
    assert!(
        last.contains("goal_checkpoint"),
        "the reminder must name the checkpoint tool:\n{last}"
    );
}

/// A `goal_request_input` that arrives alongside `stop_reason=max_tokens`
/// is still executed before the continuation prompt: the yield wins, the
/// tool result is persisted, and no second model request is made.
#[tokio::test]
async fn agent_loop_max_tokens_goal_request_input_yields_without_continuation() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("agent_loop_max_tokens_goal_request_input_yield");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Need user input".to_string(),
            objective: "Pause for a user answer".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "answer received",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id.clone());

    let request_count = Arc::new(AtomicUsize::new(0));
    let request_count_for_responder = request_count.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        request_count_for_responder.fetch_add(1, Ordering::SeqCst);
        if idx == 0 {
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "goal-max-1".to_string(),
                    name: "goal_request_input".to_string(),
                    input: serde_json::json!({"question": "Which option should I use?"}),
                }],
                Some(StopReason::MaxTokens),
                None,
            ))
        } else {
            panic!("unexpected second request at responder index {idx}");
        }
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    agent
        .agent_loop(Some(Message::new_text(Role::User, "continue the goal")))
        .await
        .expect("agent_loop should yield cleanly for user input");

    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(request_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        agent.runtime.goal_yield_reason,
        Some(GoalYieldReason::NeedsUser)
    );
    assert_eq!(
        tool_result_for(&agent.runtime.context, "goal-max-1").as_deref(),
        Some("User input requested; yield to Runner.")
    );
}

#[tokio::test]
async fn max_tokens_yield_suppresses_later_tool_waves_before_continuation() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("agent_loop_max_tokens_goal_yield_waves");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Need user input".to_string(),
            objective: "Pause for a user answer".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "answer received",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id);
    let marker = context.work_dir.join("later-wave-marker");
    let marker_for_response = marker.clone();

    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_mock = requests.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        requests_for_mock.fetch_add(1, Ordering::SeqCst);
        assert_eq!(idx, 0, "a host yield must not buy a continuation request");
        Ok((
            vec![
                ContentBlock::ToolUse {
                    id: "goal-yield".to_string(),
                    name: "goal_request_input".to_string(),
                    input: serde_json::json!({"question": "Which path?"}),
                },
                ContentBlock::ToolUse {
                    id: "later-wave".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({
                        "command": format!(
                            "printf should-not-run > {}",
                            marker_for_response.display()
                        )
                    }),
                },
            ],
            Some(StopReason::MaxTokens),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "continue the goal")))
        .await
        .expect("the truncated response should yield before continuing");

    assert_eq!(
        outcome.stop,
        AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
    );
    assert_eq!(requests.load(Ordering::SeqCst), 1);
    assert!(
        !marker.exists(),
        "a call from a later wave must not execute after YieldToHost"
    );
    assert_eq!(
        tool_result_for(&agent.runtime.context, "later-wave").as_deref(),
        Some("Not executed: the run yielded to the host")
    );
}

#[tokio::test]
async fn goal_completion_on_the_final_turn_returns_completed() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("kernel_goal_final_completion");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Finish".to_string(),
            objective: "Finish on the final permitted turn".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "complete",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id);

    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_mock = requests.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        requests_for_mock.fetch_add(1, Ordering::SeqCst);
        assert_eq!(idx, 0, "final completion must not buy another turn");
        Ok((
            vec![ContentBlock::ToolUse {
                id: "completion-1".to_string(),
                name: "goal_propose_completion".to_string(),
                input: serde_json::json!({
                    "summary": "done",
                    "claims": [{"criterion_id": "criterion-1", "claim": "complete"}]
                }),
            }],
            Some(StopReason::ToolUse),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(1));

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "finish the goal")))
        .await
        .expect("final Goal completion should complete the activation");

    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn non_final_goal_completion_continues_the_activation() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("kernel_goal_non_final_completion");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Keep going".to_string(),
            objective: "Propose completion before the final turn".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "complete",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id);

    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_mock = requests.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        requests_for_mock.fetch_add(1, Ordering::SeqCst);
        if idx == 0 {
            Ok((
                vec![ContentBlock::ToolUse {
                    id: "completion-early".to_string(),
                    name: "goal_propose_completion".to_string(),
                    input: serde_json::json!({
                        "summary": "proposed early",
                        "claims": [{"criterion_id": "criterion-1", "claim": "complete"}]
                    }),
                }],
                Some(StopReason::ToolUse),
                None,
            ))
        } else {
            Ok((
                vec![make_text_block("continued")],
                Some(StopReason::EndTurn),
                None,
            ))
        }
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(2));

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "work the goal")))
        .await
        .expect("a non-final proposal should continue");

    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert_eq!(outcome.turns, 2);
    assert_eq!(requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn truncated_final_goal_turn_cannot_complete_the_goal() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let mut context = test_context("kernel_goal_truncated_completion");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Truncated".to_string(),
            objective: "Do not complete from a truncated response".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "complete",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id);

    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_mock = requests.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        requests_for_mock.fetch_add(1, Ordering::SeqCst);
        assert_eq!(idx, 0, "the turn cap must stop after the truncated call");
        Ok((
            vec![ContentBlock::ToolUse {
                id: "completion-truncated".to_string(),
                name: "goal_propose_completion".to_string(),
                input: serde_json::json!({
                    "summary": "truncated proposal",
                    "claims": [{"criterion_id": "criterion-1", "claim": "complete"}]
                }),
            }],
            Some(StopReason::MaxTokens),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(1));

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "finish the goal")))
        .await
        .expect("a truncated final Goal turn returns a bounded stop");

    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(outcome.stop, AgentLoopStop::MaxTurns);
    assert_ne!(outcome.stop, AgentLoopStop::Completed);
    assert_eq!(requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn goal_context_input_keeps_its_non_user_origin_on_the_kernel_path() {
    ensure_config();
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();
    let mock = MockClient::new(vec![(
        vec![make_text_block("done")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("kernel_goal_context_origin"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_session("session-1".to_string(), store.clone());

    agent
        .agent_loop_detailed(Some(
            Message::new_text(Role::User, "<goal-context>work</goal-context>")
                .with_kind(MessageKind::GoalContext),
        ))
        .await
        .expect("Goal context activation completes");

    let messages = store.load_agent_session("session-1").await.unwrap();
    let goal = messages
        .iter()
        .find(|message| {
            message.content.iter().any(|block| {
                matches!(
                    block,
                    AgentContentBlock::Text { text }
                        if text.contains("<goal-context>work</goal-context>")
                )
            })
        })
        .expect("Goal context was persisted");
    assert_eq!(goal.origin, MessageOrigin::GoalContext);
}

#[tokio::test]
async fn agent_loop_respects_max_turns_cap() {
    ensure_config();
    let context = test_context("agent_loop_max_turns");
    let mock = MockClient::new(vec![(
        vec![make_text_block("done")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(1));

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();
    assert_eq!(agent.turns_taken, 1);
    assert!(
        !agent.max_turns_reached,
        "a child that answered inside its cap did not hit it: the summary must \
         not claim it was truncated"
    );
}

#[tokio::test]
async fn agent_loop_emits_turn_stats_each_iteration() {
    ensure_config();
    let context = test_context("agent_loop_turn_stats");
    let mock = MockClient::new(vec![(
        vec![make_text_block("done")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent = agent.with_ui_channel(tx);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    // Drain without blocking: the loop has returned, so every emitted
    // update is already queued.
    let mut stats = Vec::new();
    while let Ok(update) = rx.try_recv() {
        if let AgentUpdate::TurnStats {
            turns_taken,
            max_turns,
        } = update
        {
            stats.push((turns_taken, max_turns));
        }
    }
    assert_eq!(
        stats,
        vec![(1, None)],
        "one TurnStats per loop iteration, carrying the (absent) main-agent cap"
    );
}

#[tokio::test]
async fn agent_loop_turn_stats_carries_the_cap_and_counts_the_capped_turn() {
    ensure_config();
    let context = test_context("agent_loop_turn_stats_capped");
    let mock = MockClient::new(vec![(
        vec![make_text_block("done")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(1));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent = agent.with_ui_channel(tx);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    let mut stats = Vec::new();
    while let Ok(update) = rx.try_recv() {
        if let AgentUpdate::TurnStats {
            turns_taken,
            max_turns,
        } = update
        {
            stats.push((turns_taken, max_turns));
        }
    }
    assert_eq!(
        stats,
        vec![(1, Some(1))],
        "the cap is reported and the counted turn is a real model call"
    );
}

#[tokio::test]
async fn agent_loop_max_turns_zero_stops_immediately() {
    ensure_config();
    let context = test_context("agent_loop_max_turns_zero");
    // The mock is never called: the cap fires before the first LLM call.
    let mock = MockClient::new(vec![]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(0));

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();
    assert_eq!(
        agent.turns_taken, 0,
        "the cap is checked before the counter advances: no model call happened"
    );
    assert!(
        agent.max_turns_reached,
        "the summary that explains why a child stopped reads this flag, so the \
         loop that stopped it has to set it"
    );
    // Only the user message is present — the cap stopped before any reply.
    assert_eq!(agent.runtime.context.len(), 1);
}

#[tokio::test]
async fn agent_loop_detailed_reports_completed_without_tool_calls() {
    ensure_config();
    let context = test_context("agent_loop_detailed_completed");
    let mock = MockClient::new(vec![(
        vec![make_text_block("Hello, I am a coding agent.")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "Say hello")))
        .await
        .expect("agent_loop_detailed completes");

    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert_eq!(outcome.turns, 1);
    assert_eq!(outcome.tool_calls, 0);

    // The plain entry point still returns `()` for the same kind of script.
    let context = test_context("agent_loop_detailed_completed_unit");
    let mock = MockClient::new(vec![(
        vec![make_text_block("Hello again.")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    let unit: () = agent
        .agent_loop(Some(Message::new_text(Role::User, "Say hello")))
        .await
        .expect("agent_loop completes");
    assert_eq!(unit, ());
}

/// `turns` is what *this* activation consumed, not the agent's running
/// total: the Goal runner budgets one activation at a time and would
/// double-count if the field were cumulative.
#[tokio::test]
async fn agent_loop_detailed_reports_turns_as_activation_delta() {
    ensure_config();
    let context = test_context("agent_loop_detailed_turns_delta");
    let mock = MockClient::new(vec![
        (
            vec![make_text_block("first activation")],
            Some(StopReason::EndTurn),
        ),
        (
            vec![make_text_block("second activation")],
            Some(StopReason::EndTurn),
        ),
    ]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let first = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "one")))
        .await
        .expect("first activation completes");
    assert_eq!(first.turns, 1);

    let second = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "two")))
        .await
        .expect("second activation completes");
    assert_eq!(agent.turns_taken, 2, "the agent keeps a running total");
    assert_eq!(
        second.turns, 1,
        "the outcome must report the activation delta, not the running total"
    );
}

#[tokio::test]
async fn agent_loop_detailed_reports_goal_yield_and_skips_unexecuted_calls() {
    ensure_config();
    let mut context = test_context("agent_loop_detailed_goal_yield");
    let goal = context
        .goal_manager
        .create(crate::goal::GoalCreate {
            title: "Need user input".to_string(),
            objective: "Pause for a user answer".to_string(),
            criteria: vec![crate::goal::GoalCriterion::required_with_id(
                "criterion-1",
                "answer received",
            )],
        })
        .await
        .unwrap();
    context.active_goal_id = Some(goal.id.clone());

    let mock = MockClient::new(vec![(
        vec![
            ContentBlock::ToolUse {
                id: "goal-input-detailed".to_string(),
                name: "goal_request_input".to_string(),
                input: serde_json::json!({"question": "Which option should I use?"}),
            },
            ContentBlock::ToolUse {
                id: "bash-skipped".to_string(),
                name: "bash".to_string(),
                input: serde_json::json!({"command": "printf hello"}),
            },
        ],
        Some(StopReason::ToolUse),
    )]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "continue the goal")))
        .await
        .expect("agent_loop_detailed yields cleanly for user input");

    assert_eq!(
        outcome.stop,
        AgentLoopStop::GoalYield(GoalYieldReason::NeedsUser)
    );
    assert_eq!(outcome.turns, 1);
    assert_eq!(
        outcome.tool_calls, 1,
        "only the goal tool ran; the skipped bash call must not count"
    );
    assert_eq!(
        agent.runtime.goal_yield_reason,
        Some(GoalYieldReason::NeedsUser)
    );
}

#[tokio::test]
async fn agent_loop_detailed_counts_each_executed_tool_call() {
    ensure_config();
    let context = test_context("agent_loop_detailed_counts_tools");
    let mock = MockClient::new(vec![
        (
            vec![
                ContentBlock::ToolUse {
                    id: "b1".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "printf one"}),
                },
                ContentBlock::ToolUse {
                    id: "b2".to_string(),
                    name: "bash".to_string(),
                    input: serde_json::json!({"command": "printf two"}),
                },
            ],
            Some(StopReason::ToolUse),
        ),
        (vec![make_text_block("done")], Some(StopReason::EndTurn)),
    ]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "run both")))
        .await
        .expect("agent_loop_detailed completes");

    assert_eq!(outcome.tool_calls, 2);
    assert_eq!(outcome.stop, AgentLoopStop::Completed);
}

#[tokio::test]
async fn agent_loop_detailed_reports_max_turns() {
    ensure_config();
    let context = test_context("agent_loop_detailed_max_turns");
    let mock = MockClient::new(vec![(
        vec![ContentBlock::ToolUse {
            id: "b1".to_string(),
            name: "bash".to_string(),
            input: serde_json::json!({"command": "printf hi"}),
        }],
        Some(StopReason::ToolUse),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_max_turns(Some(1));

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("agent_loop_detailed stops at the cap");

    assert_eq!(outcome.stop, AgentLoopStop::MaxTurns);
    assert_eq!(
        outcome.turns, 1,
        "the probe that stops an exhausted loop is not a spent turn"
    );
}

#[tokio::test]
async fn agent_loop_detailed_reports_cancelled() {
    ensure_config();
    let context = test_context("agent_loop_detailed_cancelled");
    let mock = MockClient::new(vec![]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    agent.runtime.cancel_flag.cancel();

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("a cancelled loop returns Ok");

    assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
}

#[tokio::test]
async fn begin_user_turn_retires_a_cancelled_idle_token() {
    ensure_config();
    let context = test_context("begin_user_turn_retires_cancel");
    let mock = MockClient::new(vec![(
        vec![make_text_block("fresh turn")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    agent.runtime.cancel_flag.cancel();
    agent.begin_user_turn();
    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("a fresh user turn should run after begin_user_turn");

    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert!(!agent.runtime.cancel_flag.is_cancelled());
}

/// The loop-top cancel check runs *before* the model call, so a press that
/// arrives while the final response is still streaming is only visible to
/// the stop-reason arms — which must not book the response (and a Goal
/// completion proposal with it) against a Goal the user just cancelled
/// (2026-10-16).
#[tokio::test]
async fn agent_loop_detailed_reports_cancelled_when_the_flag_arrives_mid_response() {
    ensure_config();
    let context = test_context("agent_loop_detailed_cancel_mid_response");
    // The flag only becomes reachable once the agent exists, so the mock
    // reads it out of a slot the test fills afterwards.
    let flag_slot: std::sync::Arc<std::sync::Mutex<Option<Arc<CancelController>>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let slot = flag_slot.clone();
    let mock = MockClient::with_responder(move |_request, _idx| {
        if let Some(flag) = slot.lock().unwrap().as_ref() {
            flag.cancel();
        }
        Ok((
            vec![make_text_block("done")],
            Some(StopReason::EndTurn),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    let interrupt_calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let interrupt_calls_for_hook = interrupt_calls.clone();
    agent = agent.with_interrupt(move |_agent| {
        interrupt_calls_for_hook.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Box::pin(async { Ok(HookControl::Continue) })
    });
    *flag_slot.lock().unwrap() = Some(agent.tool_context.cancel_flag.clone());

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("a cancelled loop returns Ok");

    assert_eq!(
        outcome.stop,
        AgentLoopStop::Cancelled,
        "a cancel that lands mid-stream must beat the EndTurn classification"
    );
    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(
        interrupt_calls.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "the kernel cancellation outcome must dispatch Interrupt hooks once"
    );
}

#[tokio::test]
async fn agent_loop_detailed_reports_max_tokens_after_continuations_exhausted() {
    ensure_config();
    let context = test_context("agent_loop_detailed_max_tokens");
    let mock = MockClient::with_responder(|_request, _idx| {
        Ok((
            vec![make_text_block("partial output")],
            Some(StopReason::MaxTokens),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("an exhausted continuation finishes the turn");

    assert_eq!(outcome.stop, AgentLoopStop::MaxTokens);
    assert_eq!(outcome.tool_calls, 0);
    assert_eq!(agent.runtime.recovery_state.continuation_attempts, 3);
}

#[tokio::test]
async fn kernel_recovery_counts_are_visible_on_the_facade_after_success() {
    ensure_config();
    let context = test_context("kernel_recovery_success");
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Request("temporary connection failure".into())),
        _ => Ok((
            vec![make_text_block("recovered")],
            Some(StopReason::EndTurn),
            None,
        )),
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("the transient failure recovers");

    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert_eq!(agent.runtime.recovery_state.transport_attempts, 1);
}

#[tokio::test]
async fn kernel_recovery_counts_are_visible_on_the_facade_after_cancellation() {
    ensure_config();
    let context = test_context("kernel_recovery_cancellation");
    let mock = MockClient::with_responder(|_request, _idx| Err(LlmError::Request("retry".into())));
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    let cancel_flag = agent.runtime.cancel_flag.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        cancel_flag.cancel();
    });

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("cancellation still returns a typed outcome");

    assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
    assert_eq!(agent.runtime.recovery_state.transport_attempts, 1);
}

#[tokio::test]
async fn max_tokens_tool_use_runs_manual_compaction_then_continues() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let requests = Arc::new(AtomicUsize::new(0));
    let requests_for_mock = requests.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        requests_for_mock.fetch_add(1, Ordering::SeqCst);
        match idx {
            0 => Ok((
                vec![ContentBlock::ToolUse {
                    id: "compact-1".to_string(),
                    name: "compact".to_string(),
                    input: serde_json::json!({"focus": "keep the manual compaction"}),
                }],
                Some(StopReason::MaxTokens),
                None,
            )),
            1 => Ok((
                vec![make_text_block("compacted summary")],
                Some(StopReason::EndTurn),
                None,
            )),
            2 => Ok((
                vec![make_text_block("continued after compaction")],
                Some(StopReason::EndTurn),
                None,
            )),
            other => panic!("unexpected model request {other}"),
        }
    });
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("kernel_max_tokens_manual_compact"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "compact and continue")))
        .await
        .expect("manual compaction should precede continuation");

    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert_eq!(outcome.tool_calls, 1);
    assert_eq!(requests.load(Ordering::SeqCst), 3);
    let mut saw_manual_compaction = false;
    while let Ok(update) = rx.try_recv() {
        if matches!(
            update,
            AgentUpdate::Info(message) if message.contains("Compaction complete (Manual)")
        ) {
            saw_manual_compaction = true;
        }
    }
    assert!(
        saw_manual_compaction,
        "the kernel bridge must report the manual compaction"
    );
}

#[tokio::test]
async fn agent_loop_detailed_reports_blocked_by_hook() {
    ensure_config();
    let context = test_context("agent_loop_detailed_blocked_by_hook");
    let mock = MockClient::new(vec![]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_session_start(|_agent, _context| {
        Box::pin(async { Ok(HookControl::Block("no context yet".to_string())) })
    });

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("a blocked turn returns Ok");

    assert_eq!(outcome.stop, AgentLoopStop::BlockedByHook);
    assert_eq!(outcome.turns, 0);
}

#[tokio::test]
async fn agent_loop_drains_pending_subagent_results() {
    ensure_config();
    let context = test_context("agent_loop_drain_subagent");
    let mock = MockClient::new(vec![(
        vec![make_text_block("got it")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    agent
        .runtime
        .pending_subagent_results
        .lock()
        .unwrap()
        .push_back(SubagentResult {
            child_id: "child-1".to_string(),
            summary: "found the bug".to_string(),
            success: true,
        });

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    let text = agent
        .runtime
        .context
        .iter()
        .map(|m| crate::extract_text(&m.content))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.contains("<subagent-finished id=\"child-1\" success=\"true\">"),
        "context should contain the re-injected result: {text}"
    );
    assert!(
        text.contains("found the bug"),
        "summary should be injected: {text}"
    );
}

/// A `SessionStart` hook's context reaches the model as a message of its
/// own, framed so it does not read as user input.
///
/// Codex records the same text as a `developer` role message; Tact's
/// message model has only user/assistant, so the `<hook-context>` markers
/// carry the provenance instead. Before this, the text was logged as a
/// warning and dropped, which made the reference `basic-memory` plugin's
/// session briefing inert.
#[tokio::test]
async fn agent_loop_injects_session_start_context_as_its_own_message() {
    ensure_config();
    use tact_protocol::AgentUpdate;

    const BRIEF: &str = "graph says: resume from checkpoint 7";
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mock = MockClient::new(vec![(
        vec![make_text_block("ok")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("session_start_context"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session_start(|_agent, context| {
        context.push_additional_context(Some("plugin demo"), BRIEF);
        // Blank output must not produce an empty message.
        context.push_additional_context(None, "   \n  ");
        Box::pin(async { Ok(HookControl::Continue) })
    });

    agent.dispatch_session_start_hooks().await.unwrap();
    assert_eq!(
        agent.runtime.pending_session_context,
        vec![HookContextChunk::new(Some("plugin demo"), BRIEF)]
    );

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    let injected = agent
        .runtime
        .context
        .first()
        .expect("the injected context leads the conversation");
    assert!(
        injected.is_hook_context(),
        "the cell is marked so the TUI and compaction can tell it apart"
    );
    let text = crate::extract_text(&injected.content);
    assert!(
        crate::hook::is_hook_context_text(&text)
            && text.ends_with(crate::hook::HOOK_CONTEXT_CLOSE_TAG),
        "context is framed: {text}"
    );
    assert_eq!(
        crate::hook::hook_context_source(&text),
        Some("plugin demo"),
        "the source rides on the frame so a reload still knows it: {text}"
    );
    assert_eq!(
        crate::hook::hook_context_body(&text),
        BRIEF,
        "the body is what the hook printed: {text}"
    );
    assert!(
        agent.runtime.pending_session_context.is_empty(),
        "the queue is drained"
    );

    // The turn's own message still follows it.
    assert!(matches!(
        agent.runtime.context.get(1).map(|m| m.role),
        Some(Role::User)
    ));

    // The TUI is told what was injected, without the framing.
    let mut notices = Vec::new();
    while let Ok(update) = rx.try_recv() {
        if let AgentUpdate::HookContext { source, text } = update {
            notices.push((source, text));
        }
    }
    assert_eq!(
        notices,
        vec![(Some("plugin demo".to_string()), BRIEF.to_string())]
    );
}

/// The briefing outlives the pre-turn compaction it arrives with.
///
/// It is recorded *after* `auto_compact_due` / `compact_history`, because
/// `build_compacted_history` keeps only real user turns and would drop a
/// `<hook-context>` cell — the same reason Codex runs its start hooks after
/// `run_pre_sampling_compact`.
#[tokio::test]
async fn session_start_context_survives_the_pre_turn_compaction() {
    ensure_config();
    const BRIEF: &str = "graph says: resume from checkpoint 7";
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mock = MockClient::new(vec![
        (
            vec![make_text_block("## Handoff\nwe were mid-refactor")],
            Some(StopReason::EndTurn),
        ),
        (vec![make_text_block("ok")], Some(StopReason::EndTurn)),
    ]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("session_start_survives_compact"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session_start(|_agent, context| {
        context.push_additional_context(Some("plugin demo"), BRIEF);
        Box::pin(async { Ok(HookControl::Continue) })
    });
    agent.agent_settings.model_context_window = 60_000;
    agent.agent_settings.max_tokens = 100;

    // A restored history large enough to trip the pre-turn compaction.
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "x".repeat(300_000)));
    assert!(agent.auto_compact_due(0), "the fixture must compact");

    agent.dispatch_session_start_hooks().await.unwrap();
    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    let brief = agent
        .runtime
        .context
        .iter()
        .find(|message| message.is_hook_context())
        .expect("the briefing must outlive the compaction it arrived with");
    assert!(
        crate::extract_text(&brief.content).contains(BRIEF),
        "and carry its text: {}",
        crate::extract_text(&brief.content)
    );

    // The compaction re-queues the hooks as `compact` — Codex's checkpoint
    // trigger — so the next turn re-orients against the new history.
    assert!(agent.runtime.session_start_hooks_pending);
    assert_eq!(
        agent.runtime.session_start_source,
        crate::hook::SessionStartSource::Compact
    );
}

/// A compaction re-queues the `SessionStart` hooks as `compact`; the
/// *next* turn must actually re-run them with that source and record the
/// new briefing. The sibling test only asserts the re-queue flag/source,
/// so this one drives the second turn to prove the checkpoint is
/// delivered rather than merely armed.
#[tokio::test]
async fn session_start_hooks_rerun_as_compact_after_a_compaction() {
    ensure_config();
    use std::sync::{Arc, Mutex};

    let seen: Arc<Mutex<Vec<crate::hook::SessionStartSource>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let mock = MockClient::new(vec![
        (
            vec![make_text_block("## Handoff\nwe were mid-refactor")],
            Some(StopReason::EndTurn),
        ),
        (vec![make_text_block("first")], Some(StopReason::EndTurn)),
        (vec![make_text_block("second")], Some(StopReason::EndTurn)),
    ]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("session_start_compact_rerun"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_session_start(move |agent, context| {
        let recorder = recorder.clone();
        let source = agent.session_start_source();
        context.push_additional_context(Some("plugin demo"), &format!("brief:{}", source.as_str()));
        Box::pin(async move {
            recorder.lock().unwrap().push(source);
            Ok(HookControl::Continue)
        })
    });
    agent.agent_settings.model_context_window = 60_000;
    agent.agent_settings.max_tokens = 100;

    // A restored history large enough to trip the pre-turn compaction.
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "x".repeat(300_000)));
    assert!(agent.auto_compact_due(0), "the fixture must compact");

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();
    assert_eq!(
        &*seen.lock().unwrap(),
        &[crate::hook::SessionStartSource::Startup],
        "the first turn runs the startup hooks once"
    );
    assert!(
        agent.runtime.session_start_hooks_pending,
        "the compaction re-queues them for the next turn"
    );

    agent
        .agent_loop(Some(Message::new_text(Role::User, "again")))
        .await
        .unwrap();

    assert_eq!(
        &*seen.lock().unwrap(),
        &[
            crate::hook::SessionStartSource::Startup,
            crate::hook::SessionStartSource::Compact,
        ],
        "the re-queued hooks run with `compact` on the next turn"
    );
    let briefings: Vec<String> = agent
        .runtime
        .context
        .iter()
        .filter(|message| message.is_hook_context())
        .map(|message| crate::extract_text(&message.content))
        .collect();
    assert!(
        briefings
            .iter()
            .any(|brief| brief.contains("brief:startup")),
        "the startup briefing is recorded: {briefings:?}"
    );
    assert!(
        briefings
            .iter()
            .any(|brief| brief.contains("brief:compact")),
        "the compact briefing is recorded: {briefings:?}"
    );
}

/// Startup must not wait for a plugin hook: the first turn runs them, which
/// is still ahead of that turn's user message but no longer ahead of the
/// first frame. `basic-memory`'s hook measured ~8s warm here.
#[tokio::test]
async fn session_start_hooks_run_on_the_first_turn_and_only_once() {
    ensure_config();
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    let mock = MockClient::new(vec![
        (vec![make_text_block("first")], Some(StopReason::EndTurn)),
        (vec![make_text_block("second")], Some(StopReason::EndTurn)),
    ]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("session_start_first_turn"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_session_start(move |_agent, context| {
        let seen = seen.clone();
        Box::pin(async move {
            seen.fetch_add(1, Ordering::SeqCst);
            context.push_additional_context(None, "brief");
            Ok(HookControl::Continue)
        })
    });

    assert_eq!(calls.load(Ordering::SeqCst), 0, "startup must not run them");
    assert!(agent.runtime.session_start_hooks_pending);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 1, "the first turn runs them");

    agent
        .agent_loop(Some(Message::new_text(Role::User, "again")))
        .await
        .unwrap();
    assert_eq!(
        calls.load(Ordering::SeqCst),
        1,
        "a later turn must not re-run them"
    );
}

/// Codex's `continue: false` on `SessionStart` stops the turn: the user
/// message is never pushed, rather than pushed with a notice in the log.
#[tokio::test]
async fn session_start_stop_skips_the_turn() {
    ensure_config();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mock = MockClient::new(vec![(
        vec![make_text_block("should not run")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("session_start_stop_skips"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session_start(|_agent, _context| {
        Box::pin(async { Ok(HookControl::Block("no context yet".to_string())) })
    });

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    assert!(
        agent.runtime.context.is_empty(),
        "the turn must not run: {:?}",
        agent.runtime.context.len()
    );
    let mut notice = None;
    while let Ok(update) = rx.try_recv() {
        if let AgentUpdate::Info(text) = update {
            notice = Some(text);
        }
    }
    assert!(
        notice
            .as_deref()
            .is_some_and(|text| text.contains("no context yet")),
        "{notice:?}"
    );
}

/// Context the tool hooks collected is recorded before the next request, so
/// the model reads it alongside the tool call it annotates.
#[tokio::test]
async fn agent_loop_records_tool_hook_context() {
    ensure_config();
    let mock = MockClient::new(vec![(
        vec![make_text_block("ok")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("agent_loop_tool_hook_context"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    agent
        .runtime
        .pending_hook_context
        .lock()
        .unwrap()
        .push_back(HookContextChunk::new(Some("plugin demo"), "pre context"));

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .unwrap();

    let injected: Vec<String> = agent
        .runtime
        .context
        .iter()
        .filter(|message| message.is_hook_context())
        .map(|message| crate::extract_text(&message.content))
        .collect();
    assert_eq!(injected.len(), 1, "one cell: {injected:?}");
    assert!(injected[0].contains("pre context"), "{injected:?}");
}

#[tokio::test]
async fn agent_loop_surfaces_refusal_as_error() {
    ensure_config();
    use tact_protocol::AgentUpdate;

    let context = test_context("agent_loop_refusal");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let mock = MockClient::new(vec![(
        vec![make_text_block("I cannot help with that.")],
        Some(StopReason::Refusal),
    )]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_ui_channel(tx);

    let result = agent
        .agent_loop(Some(Message::new_text(Role::User, "unsafe request")))
        .await;

    let err = result.expect_err("refusal should return Err");
    assert!(
        err.to_string().contains("refusal"),
        "error should mention refusal, got: {err}"
    );

    let mut updates = Vec::new();
    while let Ok(u) = rx.try_recv() {
        updates.push(u);
    }
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("refused"))),
        "expected Info about refusal, got: {updates:?}"
    );
}
