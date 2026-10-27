use super::*;

#[tokio::test]
async fn agent_loop_runs_parallel_read_tools() {
    ensure_config();
    use tact_protocol::AgentUpdate;

    use crate::tool::test_support::{test_context, write_workspace_file};

    let context = test_context("agent_parallel_reads");
    let work_dir = context.work_dir.clone();
    write_workspace_file(&work_dir, "a.txt", "aaa");
    write_workspace_file(&work_dir, "b.txt", "bbb");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let mock = MockClient::new(vec![
        (
            vec![
                ContentBlock::ToolUse {
                    id: "r1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "a.txt" }),
                },
                ContentBlock::ToolUse {
                    id: "r2".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "b.txt" }),
                },
            ],
            Some(StopReason::ToolUse),
        ),
        (
            vec![make_text_block("reads done")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "read both")))
        .await
        .expect("agent_loop");

    let mut updates = Vec::new();
    while let Ok(u) = rx.try_recv() {
        updates.push(u);
    }

    let finished: Vec<_> = updates
        .iter()
        .filter_map(|u| match u {
            AgentUpdate::StepFinished {
                tool_id, result, ..
            } if result.tool == "read_file" => Some(tool_id.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(finished.len(), 2);
    assert!(finished.contains(&"r1"));
    assert!(finished.contains(&"r2"));
}

/// A cancel that lands after the assistant message was persisted but before
/// tool execution must still answer every `ToolUse` with a `ToolResult`.
/// Otherwise the stored history has a dangling tool call, which providers
/// that enforce pairing reject on the next request (and which a resume or a
/// later fork would inherit).
#[tokio::test]
async fn cancel_before_tool_execution_pairs_every_tool_use() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("cancel_before_tools");
    tool_context.events = UpdateSink::for_ui(tx.clone());

    // The responder sets the agent's cancel flag while producing the
    // tool-call turn, so the flag is guaranteed to be set when the loop
    // reaches its pre-execution cancel check. `Agent::new` allocates the
    // real flag, so it is installed into the slot after construction.
    use std::sync::{Arc, Mutex};
    let flag_slot: Arc<Mutex<Option<Arc<CancelController>>>> = Arc::new(Mutex::new(None));
    let slot = flag_slot.clone();
    let mock = MockClient::with_responder(move |_request, idx| {
        if idx > 0 {
            // Bound the loop if the cancel path regresses: answer later
            // calls with a plain end-of-turn.
            return Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ));
        }
        if let Some(flag) = slot.lock().unwrap().as_ref() {
            flag.cancel();
        }
        Ok((
            vec![ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "a.txt" }),
            }],
            Some(StopReason::ToolUse),
            None,
        ))
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());
    *flag_slot.lock().unwrap() = Some(agent.tool_context.cancel_flag.clone());

    agent
        .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
        .await
        .expect("cancelled loop returns Ok");

    // The tool must not have run.
    let mut updates = Vec::new();
    while let Ok(u) = rx.try_recv() {
        updates.push(u);
    }
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::StepFinished { .. })),
        "a cancelled-before-execution tool must not report a finished step"
    );
    assert!(agent.kernel_activations_for_test() >= 1);
    assert_eq!(
        updates
            .iter()
            .filter(
                |update| matches!(update, AgentUpdate::Info(text) if text == "Cancelled by user")
            )
            .count(),
        1,
        "facade cancellation must emit one terminal notice"
    );

    // Every persisted `ToolUse` has a matching `ToolResult`, carrying the
    // cancel reason.
    let messages = store.load_session("session-1").await.unwrap();
    assert_eq!(
        tool_pairing(&messages),
        (vec!["t1".to_string()], vec!["t1".to_string()]),
        "the persisted history must pair the cancelled tool call with a result"
    );
    assert_eq!(
        tool_result_for(&messages, "t1").as_deref(),
        Some("Cancelled by user")
    );
}

/// The cancel flag can also flip *during* pre-flight: the driver sets it
/// from its own command loop while the agent awaits a hook or a permission
/// prompt, so calls earlier in the same turn are already resolved when the
/// remaining ones are stubbed out. Every call still needs exactly one
/// result — including the one the user just denied.
#[tokio::test]
async fn cancel_mid_preflight_answers_every_tool_use_once() {
    ensure_config();
    use crate::tool::test_support::test_context;
    use tact_protocol::{AgentUpdate, UiResponse};

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("cancel_mid_preflight_deny");
    tool_context.events = UpdateSink::for_ui(tx.clone());

    // Two calls in one turn behind the usual leading text block, so the
    // cancel lands after `t1` was already resolved.
    let mock = MockClient::with_responder(|_request, idx| {
        if idx > 0 {
            return Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ));
        }
        Ok((
            vec![
                make_text_block("writing both files"),
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "a.txt", "content": "a" }),
                },
                ContentBlock::ToolUse {
                    id: "t2".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "b.txt", "content": "b" }),
                },
            ],
            Some(StopReason::ToolUse),
            None,
        ))
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());

    let flag = agent.tool_context.cancel_flag.clone();
    let responder = agent.tool_context.ui_responder.clone();
    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            if let AgentUpdate::RequestSelect { request_id, .. } = update {
                // Deny `t1`, then cancel: the flag is first observed at the
                // top of the `t2` iteration, i.e. mid-pre-flight.
                flag.cancel();
                responder.respond(UiResponse::Select {
                    request_id,
                    choice: Some(1),
                });
                return;
            }
        }
    });

    agent
        .agent_loop(Some(Message::new_text(Role::User, "write both")))
        .await
        .expect("a cancelled loop returns Ok");

    assert!(agent.kernel_activations_for_test() >= 1);
    let messages = store.load_session("session-1").await.unwrap();
    let (uses, results) = tool_pairing(&messages);
    assert_eq!(uses, vec!["t1".to_string(), "t2".to_string()]);
    assert_eq!(results, uses, "each tool use must be answered exactly once");
}

/// As above, but the first call was *approved* before the cancel landed, so
/// it is still sitting in `PreparedState::Run` with no output. It has to be
/// answered as cancelled rather than indexed out of an empty output list.
#[tokio::test]
async fn cancel_mid_preflight_answers_an_approved_but_unrun_call() {
    ensure_config();
    use crate::tool::test_support::test_context;
    use tact_protocol::{AgentUpdate, UiResponse};

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("cancel_mid_preflight_allow");
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let mock = MockClient::with_responder(|_request, idx| {
        if idx > 0 {
            return Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ));
        }
        Ok((
            vec![
                make_text_block("writing both files"),
                ContentBlock::ToolUse {
                    id: "t1".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "a.txt", "content": "a" }),
                },
                ContentBlock::ToolUse {
                    id: "t2".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "b.txt", "content": "b" }),
                },
            ],
            Some(StopReason::ToolUse),
            None,
        ))
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());

    let flag = agent.tool_context.cancel_flag.clone();
    let responder = agent.tool_context.ui_responder.clone();
    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            if let AgentUpdate::RequestSelect { request_id, .. } = update {
                // Allow `t1` once, then cancel.
                flag.cancel();
                responder.respond(UiResponse::Select {
                    request_id,
                    choice: Some(0),
                });
                return;
            }
        }
    });

    agent
        .agent_loop(Some(Message::new_text(Role::User, "write both")))
        .await
        .expect("a cancelled loop returns Ok");

    assert!(agent.kernel_activations_for_test() >= 1);
    let messages = store.load_session("session-1").await.unwrap();
    let (uses, results) = tool_pairing(&messages);
    assert_eq!(uses, vec!["t1".to_string(), "t2".to_string()]);
    assert_eq!(results, uses, "each tool use must be answered exactly once");
    assert_eq!(
        tool_result_for(&messages, "t1").as_deref(),
        Some("Cancelled by user"),
        "an approved call the cancel caught before execution is answered, not run"
    );
}

#[tokio::test]
async fn facade_cancel_while_bash_is_in_flight_drains_and_pairs_the_result() {
    ensure_config();

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("cancel_during_bash");
    tool_context.events = UpdateSink::for_ui(tx.clone());
    let mock = MockClient::with_responder(|_request, idx| {
        if idx > 0 {
            return Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ));
        }
        Ok((
            vec![ContentBlock::ToolUse {
                id: "bash-1".into(),
                name: "bash".into(),
                input: serde_json::json!({
                    "command": "printf started; sleep 0.2; printf finished"
                }),
            }],
            Some(StopReason::ToolUse),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());

    let flag = agent.tool_context.cancel_flag.clone();
    let listener = tokio::spawn(async move {
        let mut updates = Vec::new();
        while let Some(update) = rx.recv().await {
            let cancelling = matches!(
                &update,
                AgentUpdate::Info(text) if text == "Cancelled by user"
            );
            if matches!(
                &update,
                AgentUpdate::ToolProgress { tool_id, .. } if tool_id == "bash-1"
            ) {
                flag.cancel();
            }
            updates.push(update);
            if cancelling {
                break;
            }
        }
        updates
    });

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        agent.agent_loop_detailed(Some(Message::new_text(Role::User, "run it"))),
    )
    .await
    .expect("in-flight bash cancellation must be bounded")
    .expect("cancelled loop returns Ok");
    assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
    assert!(agent.kernel_activations_for_test() >= 1);

    let updates = tokio::time::timeout(std::time::Duration::from_secs(1), listener)
        .await
        .expect("cancellation notice must close the bounded update listener")
        .expect("update listener");
    assert_eq!(
        updates
            .iter()
            .filter(
                |update| matches!(update, AgentUpdate::Info(text) if text == "Cancelled by user")
            )
            .count(),
        1
    );
    assert!(
        updates.iter().any(|update| matches!(
            update,
            AgentUpdate::StepFinished { tool_id, .. } if tool_id == "bash-1"
        )),
        "the drained bash wave must produce a terminal tool event"
    );

    let messages = store.load_session("session-1").await.unwrap();
    assert_eq!(
        tool_pairing(&messages),
        (vec!["bash-1".to_string()], vec!["bash-1".to_string()])
    );
}

#[tokio::test]
async fn facade_cancel_while_another_tool_is_in_flight_drains_without_another_turn() {
    ensure_config();

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("cancel_during_sleep");
    tool_context.events = UpdateSink::for_ui(tx.clone());
    let mock = MockClient::with_responder(|_request, idx| {
        if idx > 0 {
            return Ok((
                vec![make_text_block("must not run")],
                Some(StopReason::EndTurn),
                None,
            ));
        }
        Ok((
            vec![ContentBlock::ToolUse {
                id: "sleep-1".into(),
                name: "sleep".into(),
                input: serde_json::json!({"ms": 200}),
            }],
            Some(StopReason::ToolUse),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());

    let flag = agent.tool_context.cancel_flag.clone();
    let listener = tokio::spawn(async move {
        let mut updates = Vec::new();
        while let Some(update) = rx.recv().await {
            if matches!(
                &update,
                AgentUpdate::StepStarted { tool_id, .. } if tool_id == "sleep-1"
            ) {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                flag.cancel();
            }
            let cancelling = matches!(
                &update,
                AgentUpdate::Info(text) if text == "Cancelled by user"
            );
            updates.push(update);
            if cancelling {
                break;
            }
        }
        updates
    });

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        agent.agent_loop_detailed(Some(Message::new_text(Role::User, "wait"))),
    )
    .await
    .expect("in-flight sleep cancellation must be bounded")
    .expect("cancelled loop returns Ok");
    assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
    assert_eq!(
        outcome.turns, 1,
        "cancellation must not dispatch another turn"
    );
    assert!(agent.kernel_activations_for_test() >= 1);

    let updates = tokio::time::timeout(std::time::Duration::from_secs(1), listener)
        .await
        .expect("cancellation notice must close the bounded update listener")
        .expect("update listener");
    assert!(
        updates.iter().any(|update| matches!(
            update,
            AgentUpdate::StepFinished { .. } | AgentUpdate::StepFailed { .. }
        )),
        "the in-flight sleep must drain to a terminal tool event"
    );

    let messages = store.load_session("session-1").await.unwrap();
    assert_eq!(
        tool_pairing(&messages),
        (vec!["sleep-1".to_string()], vec!["sleep-1".to_string()])
    );
}

#[tokio::test]
async fn facade_cancel_between_waves_suppresses_later_side_effects_and_pairs_calls() {
    ensure_config();

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("cancel_between_waves");
    tool_context.events = UpdateSink::for_ui(tx.clone());
    let work_dir = tool_context.work_dir.clone();
    let flag_slot: Arc<Mutex<Option<Arc<CancelController>>>> = Arc::new(Mutex::new(None));
    let hook_flag = flag_slot.clone();
    let mock = MockClient::with_responder(|_request, idx| {
        if idx > 0 {
            return Ok((
                vec![make_text_block("done")],
                Some(StopReason::EndTurn),
                None,
            ));
        }
        Ok((
            vec![
                ContentBlock::ToolUse {
                    id: "w1".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "printf first > wave1.txt"}),
                },
                ContentBlock::ToolUse {
                    id: "w2".into(),
                    name: "bash".into(),
                    input: serde_json::json!({"command": "printf second > wave2.txt"}),
                },
            ],
            Some(StopReason::ToolUse),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone())
    .with_post_tool_hook(move |_view, tool, _result, _status| {
        let flag_slot = hook_flag.clone();
        Box::pin(async move {
            if tool.id == "w1"
                && let Some(flag) = flag_slot.lock().unwrap().as_ref()
            {
                flag.cancel();
            }
            Ok(HookControl::Continue)
        })
    });
    *flag_slot.lock().unwrap() = Some(agent.tool_context.cancel_flag.clone());

    let listener = tokio::spawn(async move {
        let mut updates = Vec::new();
        while let Some(update) = rx.recv().await {
            let cancelling = matches!(
                &update,
                AgentUpdate::Info(text) if text == "Cancelled by user"
            );
            updates.push(update);
            if cancelling {
                break;
            }
        }
        updates
    });

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        agent.agent_loop_detailed(Some(Message::new_text(Role::User, "write twice"))),
    )
    .await
    .expect("between-wave cancellation must be bounded")
    .expect("cancelled loop returns Ok");
    assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
    assert!(agent.kernel_activations_for_test() >= 1);

    let updates = tokio::time::timeout(std::time::Duration::from_secs(1), listener)
        .await
        .expect("cancellation notice must close the bounded update listener")
        .expect("update listener");
    assert_eq!(
        updates
            .iter()
            .filter(|update| matches!(
                update,
                AgentUpdate::StepFinished { tool_id, .. } if tool_id == "w1"
            ))
            .count(),
        1
    );
    assert!(
        !updates.iter().any(|update| matches!(
            update,
            AgentUpdate::StepFinished { tool_id, .. } if tool_id == "w2"
        )),
        "the later wave must not execute after cancellation"
    );
    assert!(
        updates.iter().any(|update| matches!(
            update,
            AgentUpdate::StepFailed { tool_id, .. } if tool_id == "w2"
        )),
        "the suppressed call still needs one paired terminal result"
    );

    let messages = store.load_session("session-1").await.unwrap();
    let (uses, results) = tool_pairing(&messages);
    assert_eq!(uses, vec!["w1".to_string(), "w2".to_string()]);
    assert_eq!(results, uses);
    assert_eq!(
        tool_result_for(&messages, "w2").as_deref(),
        Some("Cancelled by user")
    );
    assert_eq!(
        std::fs::read_to_string(work_dir.join("wave1.txt")).unwrap(),
        "first"
    );
    assert!(
        !work_dir.join("wave2.txt").exists(),
        "the later wave must not reach the shell"
    );
}

/// A refusal is surfaced as an error, but the assistant message that asked
/// for a tool is already persisted: it still has to be answered.
#[tokio::test]
async fn refusal_pairs_every_tool_use() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("refusal_pairs_tools");
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let mock = MockClient::new(vec![(
        vec![
            make_text_block("I will not do that"),
            ContentBlock::ToolUse {
                id: "t1".to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "a.txt" }),
            },
        ],
        Some(StopReason::Refusal),
    )]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());

    agent
        .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
        .await
        .expect_err("a refusal is surfaced as an error");

    let messages = store.load_session("session-1").await.unwrap();
    assert_eq!(
        tool_pairing(&messages),
        (vec!["t1".to_string()], vec!["t1".to_string()]),
        "a refused turn must still answer the tool call it asked for"
    );
    assert_eq!(
        tool_result_for(&messages, "t1").as_deref(),
        Some("Not executed: the model refused this request")
    );
}

/// Once the continuation budget is exhausted, a truncated response finishes
/// the turn — including the tool call it may have emitted mid-truncation,
/// which the recovery branch above would otherwise have executed.
#[tokio::test]
async fn truncated_turn_pairs_every_tool_use() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = test_context("truncated_pairs_tools");
    tool_context.events = UpdateSink::for_ui(tx.clone());

    // Every call is truncated with a fresh tool call, so the continuation
    // path runs the tool for the first `MAX_CONTINUATION_ATTEMPTS` turns and
    // the last one is left for the give-up arm.
    let mock = MockClient::with_responder(move |_request, idx| {
        Ok((
            vec![ContentBlock::ToolUse {
                id: format!("t{idx}"),
                name: "read_file".to_string(),
                input: serde_json::json!({ "path": "missing.txt" }),
            }],
            Some(StopReason::MaxTokens),
            None,
        ))
    });

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx)
    .with_session("session-1".to_string(), store.clone());

    agent
        .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
        .await
        .expect("an exhausted continuation finishes the turn");

    let messages = store.load_session("session-1").await.unwrap();
    let (uses, results) = tool_pairing(&messages);
    assert!(
        uses.len() > 1,
        "the continuation path must have executed at least one turn: {uses:?}"
    );
    assert_eq!(
        uses, results,
        "every truncated turn's tool call needs a matching result"
    );
    let last = uses.last().expect("at least one tool call");
    assert_eq!(
        tool_result_for(&messages, last).as_deref(),
        Some("Not executed: output was truncated before this tool ran"),
        "the call the agent gave up on is answered, not executed"
    );
}

#[tokio::test]
async fn agent_loop_plan_mode_denies_write() {
    ensure_config();
    use tact_protocol::AgentUpdate;

    use crate::tool::test_support::test_context;

    let context = test_context("agent_plan_deny");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let mock = MockClient::new(vec![
        (
            vec![ContentBlock::ToolUse {
                id: "w1".to_string(),
                name: "write_file".to_string(),
                input: serde_json::json!({ "path": "x.txt", "content": "data" }),
            }],
            Some(StopReason::ToolUse),
        ),
        (
            vec![make_text_block("continued")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Plan)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "write")))
        .await
        .expect("agent_loop");

    let mut updates = Vec::new();
    while let Ok(u) = rx.try_recv() {
        updates.push(u);
    }

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFailed { tool_id, error, .. }
                    if tool_id == "w1" && error.contains("Plan mode")
            )
        }),
        "Plan mode should StepFailed on write, got: {updates:?}"
    );
}

#[tokio::test]
async fn agent_loop_emits_token_usage_from_mock() {
    ensure_config();
    use tact_protocol::{AgentUpdate, TokenUsageInfo};

    use crate::tool::test_support::test_context;

    let context = test_context("agent_token_usage");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let usage = TokenUsageInfo {
        prompt: 50,
        completion: 10,
        total: 60,
        prompt_cache_hit_tokens: 0,
        prompt_cache_miss_tokens: 50,
        reasoning_tokens: 0,
    };

    let mock = MockClient::with_usage(vec![(
        vec![make_text_block("ok")],
        Some(StopReason::EndTurn),
        crate::runtime::provider_token_usage(&usage),
    )]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("agent_loop");

    let mut updates = Vec::new();
    while let Ok(u) = rx.try_recv() {
        updates.push(u);
    }

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::TokenUsage(u) if u.total == usage.total
            )
        }),
        "expected TokenUsage from mock, got: {updates:?}"
    );
}

#[tokio::test]
async fn agent_loop_serializes_read_before_write_on_same_file() {
    ensure_config();
    use tact_protocol::AgentUpdate;

    use crate::tool::test_support::{test_context, write_workspace_file};

    let context = test_context("agent_read_write_serial");
    let work_dir = context.work_dir.clone();
    write_workspace_file(&work_dir, "shared.txt", "seed");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx.clone());

    let mock = MockClient::new(vec![
        (
            vec![
                ContentBlock::ToolUse {
                    id: "r1".to_string(),
                    name: "read_file".to_string(),
                    input: serde_json::json!({ "path": "shared.txt" }),
                },
                ContentBlock::ToolUse {
                    id: "w1".to_string(),
                    name: "write_file".to_string(),
                    input: serde_json::json!({ "path": "shared.txt", "content": "next" }),
                },
            ],
            Some(StopReason::ToolUse),
        ),
        (vec![make_text_block("done")], Some(StopReason::EndTurn)),
    ]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    agent
        .agent_loop(Some(Message::new_text(Role::User, "rw")))
        .await
        .expect("agent_loop");

    let mut updates = Vec::new();
    while let Ok(u) = rx.try_recv() {
        updates.push(u);
    }

    let read_done = updates
        .iter()
        .position(|u| matches!(u, AgentUpdate::StepFinished { tool_id, .. } if tool_id == "r1"));
    let write_done = updates
        .iter()
        .position(|u| matches!(u, AgentUpdate::StepFinished { tool_id, .. } if tool_id == "w1"));
    assert!(
        read_done.is_some() && write_done.is_some() && read_done < write_done,
        "read must finish before write on same file, got: {updates:?}"
    );
    assert_eq!(
        std::fs::read_to_string(work_dir.join("shared.txt")).unwrap(),
        "next"
    );
}
