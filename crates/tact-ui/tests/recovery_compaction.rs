//! Recovery and context-compaction scenarios for the agent harness.

mod harness;

use harness::{
    bash_tool_use, read_file_tool_use, run_single_task_with_config, task_completed_with, text_block,
};
use tact::{permission::PermissionMode, tool::test_support::write_workspace_file};
use tact_llm::{ContentBlock, LlmError, MessageContent, MockClient, ProviderKind, StopReason};
use tact_protocol::{AgentUpdate, TokenUsageInfo};

fn error_contains(updates: &[AgentUpdate], needle: &str) -> bool {
    updates.iter().any(
        |update| matches!(update, AgentUpdate::Error(error) if error.to_string().contains(needle)),
    )
}

fn tiny_context_config() -> tact::config::ResolvedConfig {
    tact::config::ResolvedConfig {
        llm: tact::config::LlmSettings {
            provider: ProviderKind::OpenAi,
            protocol: tact_llm::OpenAiProtocol::default(),
            reasoning_effort: None,
            api_key: String::new(),
            base_url: String::new(),
            model: "mock-model".to_string(),
            models: Vec::new(),
            responses_compact_threshold: None,
        },
        agent: tact::config::AgentSettings {
            model_context_window: 100_000,
            max_tokens: 8192,
            thinking_budget: 0,
            snapshot_max_items: 80,
            notifications_enabled: false,
            micro_compact_enabled: true,
            skill_body_auto_inject: false,
            skill_dirs: Vec::new(),
            instruction_sources: tact::config::InstructionSources::default(),
            subagent: None,
        },
        ui: tact::config::UiSettings {
            theme: "retro".to_string(),
            vision_image: tact::config::VisionImageSettings {
                compress: tact::config::VisionImageSettings::DEFAULT_COMPRESS,
                max_edge: tact::config::VisionImageSettings::DEFAULT_MAX_EDGE,
                jpeg_quality: tact::config::VisionImageSettings::DEFAULT_JPEG_QUALITY,
            },
        },
        tools: tact::config::ToolSettings {
            bash_timeout_secs: tact::config::ToolSettings::DEFAULT_BASH_TIMEOUT_SECS,
            bash_nice: tact::config::ToolSettings::DEFAULT_BASH_NICE,
        },
        voice: tact::config::VoiceSettings::disabled_defaults(),
        permission_mode: None,
        tokio_console: false,
        config_path: None,
    }
}

#[tokio::test]
async fn context_limit_triggers_auto_compact() {
    let big_content = "x".repeat(3000);

    let mock = MockClient::with_usage(vec![
        (
            vec![read_file_tool_use("read1", "big.txt")],
            Some(StopReason::ToolUse),
            TokenUsageInfo {
                total: 85_000,
                ..TokenUsageInfo::default()
            },
        ),
        (
            // This turn is consumed by compact_history's create_message call.
            vec![text_block("Summary of previous conversation.")],
            Some(StopReason::EndTurn),
            TokenUsageInfo::default(),
        ),
        (
            vec![text_block("Done after compact.")],
            Some(StopReason::EndTurn),
            TokenUsageInfo::default(),
        ),
    ]);

    let (updates, work_dir) = run_single_task_with_config(
        mock,
        "read big file",
        PermissionMode::Auto,
        tiny_context_config(),
        |dir| write_workspace_file(dir, "big.txt", &big_content),
    )
    .await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[auto compact]"))),
        "expected auto compact info, got: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[transcript saved"))),
        "expected transcript saved info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Done after compact"));

    // Transcript should have been written under .tact/transcripts.
    let transcript_dir = work_dir.join(".tact").join("transcripts");
    assert!(
        transcript_dir
            .read_dir()
            .map(|mut d| d.next().is_some())
            .unwrap_or(false),
        "transcript file should be persisted"
    );
}

#[tokio::test]
async fn end_turn_does_not_compact_until_another_model_call_is_needed() {
    let mock = MockClient::with_usage(vec![(
        vec![text_block("Finished near the context limit.")],
        Some(StopReason::EndTurn),
        TokenUsageInfo {
            total: 85_000,
            ..TokenUsageInfo::default()
        },
    )]);

    let (updates, _) = run_single_task_with_config(
        mock,
        "finish",
        PermissionMode::Auto,
        tiny_context_config(),
        |_| {},
    )
    .await;

    assert!(task_completed_with(
        &updates,
        "Finished near the context limit"
    ));
    assert!(
        !updates.iter().any(|update| matches!(update, AgentUpdate::Info(message) if message.contains("[auto compact]"))),
        "terminal response must not trigger an unused compaction call: {updates:?}"
    );
}

#[tokio::test]
async fn failed_compact_tool_does_not_trigger_manual_compaction() {
    let invalid_compact = ContentBlock::ToolUse {
        id: "compact1".to_string(),
        name: "compact".to_string(),
        input: serde_json::json!({ "focus": 42 }),
    };
    let mock = MockClient::with_responder(move |request, idx| match idx {
        0 => Ok((
            vec![invalid_compact.clone()],
            Some(StopReason::ToolUse),
            None,
        )),
        _ => {
            let prompt = serde_json::to_string(&request.messages).unwrap();
            if prompt.contains("Summarize this coding-agent conversation") {
                Ok((Vec::new(), Some(StopReason::EndTurn), None))
            } else {
                Ok((
                    vec![text_block("Continued after rejected compact.")],
                    Some(StopReason::EndTurn),
                    None,
                ))
            }
        }
    });

    let (updates, _) = run_single_task_with_config(
        mock,
        "compact",
        PermissionMode::Auto,
        tiny_context_config(),
        |_| {},
    )
    .await;

    assert!(task_completed_with(
        &updates,
        "Continued after rejected compact"
    ));
    assert!(
        !updates.iter().any(|update| matches!(update, AgentUpdate::Info(message) if message.contains("[manual compact]"))),
        "failed compact tool must not rewrite conversation history: {updates:?}"
    );
}

#[tokio::test]
async fn prompt_too_long_recovery_compacts_and_retries() {
    let mock = MockClient::with_responder(move |request, idx| {
        match idx {
            0 => Err(LlmError::Unsupported("prompt is too long".to_string())),
            // compact_history's create_message consumes this turn.
            1 => Ok((
                vec![text_block("Compacted summary.")],
                Some(StopReason::EndTurn),
                None,
            )),
            // Retry after compaction.
            _ => {
                assert!(
                    request.messages.iter().any(|message| matches!(
                        &message.content,
                        MessageContent::Blocks { content }
                            if content.iter().any(|block| matches!(
                                block,
                                ContentBlock::Text { text } if text == "recover"
                            ))
                    )),
                    "compacted request should retain the UI block prompt: {:?}",
                    request.messages
                );
                Ok((
                    vec![text_block("Recovered from long prompt.")],
                    Some(StopReason::EndTurn),
                    None,
                ))
            }
        }
    });

    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _work_dir) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[Recovery]") && msg.contains("compact"))),
        "expected compact recovery info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Recovered from long prompt"));
}

#[tokio::test]
async fn compact_summary_retries_transient_transport_error() {
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Unsupported("prompt is too long".to_string())),
        1 => Err(LlmError::Unsupported(
            "service temporarily unavailable".to_string(),
        )),
        2 => Ok((
            vec![text_block("Summary after retry.")],
            Some(StopReason::EndTurn),
            None,
        )),
        _ => Ok((
            vec![text_block("Recovered after summary retry.")],
            Some(StopReason::EndTurn),
            None,
        )),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(updates.iter().any(
        |update| matches!(update, AgentUpdate::Info(message) if message.contains("compact retry"))
    ));
    assert!(task_completed_with(
        &updates,
        "Recovered after summary retry"
    ));
}

#[tokio::test]
async fn compact_summary_rejects_empty_text_response() {
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Unsupported("prompt is too long".to_string())),
        _ => Ok((Vec::new(), Some(StopReason::EndTurn), None)),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(error_contains(
        &updates,
        "summary response contained no text"
    ));
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, AgentUpdate::TaskComplete(_)))
    );
}

#[tokio::test]
async fn compact_summary_rejects_truncated_response() {
    let mock = MockClient::with_responder(|_request, idx| match idx {
        0 => Err(LlmError::Unsupported("prompt is too long".to_string())),
        _ => Ok((
            vec![text_block("partial summary")],
            Some(StopReason::MaxTokens),
            None,
        )),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 200_000;
    let (updates, _) =
        run_single_task_with_config(mock, "recover", PermissionMode::Auto, config, |_| {}).await;

    assert!(error_contains(&updates, "invalid stop reason: MaxTokens"));
}

#[tokio::test]
async fn compact_summary_request_is_window_aware_for_oversized_turn() {
    let task = "x".repeat(100_000);
    let mock = MockClient::with_responder(|request, idx| match idx {
        0 => Err(LlmError::Unsupported("prompt is too long".to_string())),
        1 => {
            let prompt = serde_json::to_string(&request.messages).unwrap();
            assert_eq!(request.max_tokens, 2_000);
            assert!(
                prompt.chars().count() < 100_000,
                "summary prompt was not bounded"
            );
            Ok((
                vec![text_block("Bounded summary.")],
                Some(StopReason::EndTurn),
                None,
            ))
        }
        _ => Ok((
            vec![text_block("Recovered from oversized turn.")],
            Some(StopReason::EndTurn),
            None,
        )),
    });
    let mut config = tiny_context_config();
    config.agent.model_context_window = 35_000;
    config.agent.max_tokens = 2_000;
    let (updates, _) =
        run_single_task_with_config(mock, &task, PermissionMode::Auto, config, |_| {}).await;

    assert!(task_completed_with(
        &updates,
        "Recovered from oversized turn"
    ));
}

#[tokio::test]
async fn max_tokens_with_pending_tools_executes_then_continues() {
    let mock = MockClient::new(vec![
        (
            // Simulate truncation mid-tool-call: the model emitted a tool use but hit max_tokens.
            vec![bash_tool_use("bash1", "echo ok")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![text_block("Continued after max_tokens.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (updates, _work_dir) = run_single_task_with_config(
        mock,
        "truncated tool",
        PermissionMode::Auto,
        tiny_context_config(),
        |_| {},
    )
    .await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "bash1")),
        "pending bash tool should still execute, got: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("[Recovery]") && msg.contains("continue"))),
        "expected continuation recovery info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Continued after max_tokens"));
}

#[tokio::test]
async fn max_tokens_with_large_pending_tool_result_compacts_before_continuation() {
    let large_content = "x".repeat(30_000);
    let mock = MockClient::with_responder(|request, idx| match idx {
        0 => Ok((
            vec![read_file_tool_use("read1", "large.txt")],
            Some(StopReason::MaxTokens),
            Some(TokenUsageInfo {
                total: 30_000,
                ..TokenUsageInfo::default()
            }),
        )),
        1 => {
            let prompt = serde_json::to_string(&request.messages).unwrap();
            assert!(
                prompt.contains("Summarize this coding-agent conversation"),
                "expected compaction request: {prompt}"
            );
            Ok((
                vec![text_block("Summary before continuation.")],
                Some(StopReason::EndTurn),
                None,
            ))
        }
        _ => Ok((
            vec![text_block("Continued after compact.")],
            Some(StopReason::EndTurn),
            None,
        )),
    });

    let mut config = tiny_context_config();
    config.agent.model_context_window = 35_000;
    config.agent.max_tokens = 2_000;
    let (updates, _) = run_single_task_with_config(
        mock,
        "truncated read",
        PermissionMode::Auto,
        config,
        |dir| write_workspace_file(dir, "large.txt", &large_content),
    )
    .await;

    assert!(task_completed_with(&updates, "Continued after compact"));
}

// ── Native Responses `/compact` through the command loop ──────────────
//
// `UserCommand::Compact` stays provider-agnostic: the driver calls
// `agent.compact_history(None)` and the Responses agent dispatches to the
// native `/responses/compact` endpoint. These tests drive the real command
// loop against a local wiremock server.

use std::sync::Arc;

use tact::store::{SessionStore, session_store::SqliteSessionStore};
use tact_llm::{Message, ProviderConversationState, ResponsesConversationState, Role};
use tact_protocol::UserCommand;
use tact_ui::{
    driver::run_command_loop,
    test_support::{build_responses_test_agent, collect_updates_after, user_command_channels},
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn compact_resource_fixture() -> serde_json::Value {
    serde_json::json!({
        "id": "cmp_native_01",
        "object": "response.compaction",
        "created_at": 1754000000,
        "output": [
            {
                "type": "function_call_output",
                "call_id": "call_sanitized_1",
                "output": "sanitized tool output retained by compaction",
                "id": "fc_out_sanitized_1",
                "status": "completed"
            },
            {
                "type": "compaction",
                "id": "cmp_native_01",
                "encrypted_content": "opaque-encrypted-compaction-content"
            }
        ],
        "usage": {
            "input_tokens": 10,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens": 2,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": 12
        }
    })
}

fn messages_json(messages: &[tact_llm::Message]) -> serde_json::Value {
    serde_json::to_value(messages).unwrap()
}

fn info_messages(updates: &[AgentUpdate]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|update| match update {
            AgentUpdate::Info(msg) => Some(msg.clone()),
            _ => None,
        })
        .collect()
}

/// Drive a single `UserCommand::Compact` through the command loop and return
/// the finished agent plus every update it emitted.
async fn drive_compact_command(
    agent: tact::Agent,
    work_dir: std::path::PathBuf,
    agent_rx: tokio::sync::mpsc::UnboundedReceiver<AgentUpdate>,
) -> (tact::Agent, Vec<AgentUpdate>) {
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();
    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));
    user_cmd_tx.send(UserCommand::Compact).unwrap();
    drop(user_cmd_tx);
    let agent = driver.await.unwrap();
    (agent, collect_updates_after(agent_rx).await)
}

#[tokio::test]
async fn command_compact_native_responses_success() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_json(compact_resource_fixture()))
        .expect(1)
        .mount(&server)
        .await;
    // A local summary `create_message()` must never be attempted: an
    // ordinary `/responses` request would fail the test.
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut agent, work_dir) = build_responses_test_agent(&server.uri(), Some(agent_tx));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    let (agent, updates) = drive_compact_command(agent, work_dir, agent_rx).await;

    // Exactly one native compact request; no local summary request.
    server.verify().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "one native compact request expected");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(
        body.get("tools").is_none(),
        "native compact request must not declare any tools: {body}"
    );
    let serialized = serde_json::to_string(&body).unwrap();
    assert!(
        !serialized.contains("\"name\":\"compact\""),
        "native compact request must not declare a compact function: {body}"
    );
    assert!(
        !agent
            .all_tool_specs()
            .iter()
            .any(|spec| spec.name == "compact"),
        "Responses model-facing tool specs must not include the local compact tool"
    );

    let infos = info_messages(&updates);
    for needle in [
        "[compacting]",
        "[native compact]",
        "[responses compacted: items=2, id=cmp_native_01]",
        "Compaction complete.",
    ] {
        assert!(
            infos.iter().any(|msg| msg.contains(needle)),
            "expected info {needle:?}, got: {updates:?}"
        );
    }
    assert!(
        !updates
            .iter()
            .any(|update| matches!(update, AgentUpdate::Error(_))),
        "no error expected after successful compaction, got: {updates:?}"
    );

    // Committed runtime state: the replacement baseline carries the fixture
    // compaction id and the logical context is unchanged.
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("provider state must be committed after native compaction");
    };
    assert_eq!(state.compaction_id.as_deref(), Some("cmp_native_01"));
    assert!(state.is_compacted);
    assert_eq!(state.input_items.len(), 2);
    assert_eq!(
        agent.runtime.context.len(),
        2,
        "native compaction keeps the logical context unchanged"
    );
    let encrypted = state
        .input_items
        .iter()
        .find_map(|item| item.get("encrypted_content").and_then(|v| v.as_str()))
        .expect("compaction item must carry encrypted_content");
    assert!(
        infos.iter().all(|msg| !msg.contains(encrypted)),
        "encrypted compaction content must never surface in Info updates, got: {updates:?}"
    );
}

#[tokio::test]
async fn command_compact_native_responses_failure_keeps_context() {
    let server = MockServer::start().await;
    // Malformed compact resource: protocol error, never retried.
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "id": "cmp_bad",
            "object": "response.compaction",
            "output": []
        })))
        .expect(1)
        .mount(&server)
        .await;

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut agent, work_dir) = build_responses_test_agent(&server.uri(), Some(agent_tx));
    let original = vec![
        Message::new_text(Role::User, "first turn"),
        Message::new_text(Role::Assistant, "second turn"),
    ];
    agent.runtime.context = original.clone();

    let (agent, updates) = drive_compact_command(agent, work_dir, agent_rx).await;

    assert!(
        updates.iter().any(|update| matches!(
            update,
            AgentUpdate::Error(error) if error.to_string().contains("Compaction failed")
        )),
        "expected compaction failure error, got: {updates:?}"
    );
    assert!(
        !info_messages(&updates)
            .iter()
            .any(|msg| msg == "Compaction complete."),
        "no success info after failure, got: {updates:?}"
    );
    assert_eq!(
        messages_json(&agent.runtime.context),
        messages_json(&original),
        "failed compaction must leave the logical context unchanged"
    );
    assert!(
        agent.runtime.provider_state.is_none(),
        "failed compaction must leave the old committed provider state intact"
    );

    server.verify().await;
}

#[tokio::test]
async fn restart_round_trip_preserves_compaction_and_unknown_items() {
    let server = MockServer::start().await;
    let completed = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_1",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "msg_1",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "hello after restart"
                }]
            }],
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 2,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 12
            }
        }
    });
    let sse_body = format!("data: {completed}\n\n");
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(sse_body),
        )
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("session.db");
    let session_id = "restart-session";
    let root_dir = tmp.path().display().to_string();

    // 1. Create a session and persist messages plus a Responses state that
    //    contains a compaction item and an unknown future item.
    let messages = vec![
        Message::new_text(Role::User, "first turn"),
        Message::new_text(Role::Assistant, "second turn"),
    ];
    let state = ProviderConversationState::OpenAiResponses(ResponsesConversationState {
        version: 1,
        provider: "openai_responses".to_string(),
        base_url: server.uri(),
        model: "mock-model".to_string(),
        input_items: vec![
            serde_json::json!({
                "type": "compaction",
                "id": "cmp_restart_1",
                "encrypted_content": "opaque-encrypted-bytes",
                "retention": "kept"
            }),
            serde_json::json!({
                "type": "custom_future_item",
                "id": "unknown_item_1",
                "role": "user",
                "payload": { "future": true }
            }),
        ],
        compaction_id: Some("cmp_restart_1".to_string()),
        is_compacted: true,
        logical_message_count: 2,
        logical_context_hash: tact_llm::context_hash(&messages).unwrap(),
    });
    {
        let store = tact::store::open_sqlite_session_store(&db_path)
            .await
            .unwrap();
        store
            .create_session(session_id, &root_dir, "")
            .await
            .unwrap();
        store
            .replace_session_messages_and_provider_state(session_id, &messages, Some(&state))
            .await
            .unwrap();
        // 3. Drop the store: simulates the process restarting.
    }

    // Reopen the database from scratch.
    let store = tact::store::open_sqlite_session_store(&db_path)
        .await
        .unwrap();

    // 4/5. Load both values and assert exact equality.
    let loaded_messages = store.load_session(session_id).await.unwrap();
    assert_eq!(
        messages_json(&loaded_messages),
        messages_json(&messages),
        "messages must survive the restart"
    );
    let loaded_state = store
        .load_provider_state(session_id)
        .await
        .unwrap()
        .expect("provider state must survive the restart");
    assert_eq!(
        loaded_state, state,
        "provider state must survive the restart"
    );

    // 6. Build the next request after the restart: ensure_session restores
    //    context + baseline, then a new user turn goes to the endpoint with
    //    the baseline items replayed verbatim.
    let (agent, _) = build_responses_test_agent(&server.uri(), None);
    let mut agent = agent.with_session(session_id.to_string(), store);
    agent.ensure_session().await.unwrap();
    assert_eq!(agent.runtime.context.len(), 2);
    agent
        .agent_loop(Some(Message::new_text(Role::User, "third turn")))
        .await
        .expect("agent loop after restart should complete");

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let input = body["input"].as_array().expect("next request input array");
    let compaction_count = input
        .iter()
        .filter(|item| item.get("type").and_then(|v| v.as_str()) == Some("compaction"))
        .count();
    assert_eq!(
        compaction_count, 1,
        "compaction item must be present exactly once in the next request: {body}"
    );
    let unknown = input
        .iter()
        .find(|item| item.get("type").and_then(|v| v.as_str()) == Some("custom_future_item"))
        .expect("unknown item must be preserved in the next request");
    assert_eq!(unknown["payload"]["future"], true);
    assert_eq!(unknown["id"], "unknown_item_1");
    // No compact function declaration in the model request.
    let tools = body["tools"].as_array().cloned().unwrap_or_default();
    assert!(
        !tools
            .iter()
            .any(|tool| tool.get("name").and_then(|v| v.as_str()) == Some("compact")),
        "model request must not declare the local compact tool: {body}"
    );

    server.verify().await;
}

#[tokio::test]
async fn native_compact_failure_rolls_back_runtime_and_database() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_json(compact_resource_fixture()))
        .expect(1)
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let db_path = tmp.path().join("session.db");
    let store = SqliteSessionStore::new(&db_path).await.unwrap();
    store
        .create_session("rollback-session", tmp.path().to_str().unwrap(), "")
        .await
        .unwrap();

    let old_messages = vec![
        Message::new_text(Role::User, "old user turn"),
        Message::new_text(Role::Assistant, "old assistant turn"),
    ];
    let old_state = ProviderConversationState::OpenAiResponses(ResponsesConversationState {
        version: 1,
        provider: "openai_responses".to_string(),
        base_url: server.uri(),
        model: "mock-model".to_string(),
        input_items: vec![serde_json::json!({"type": "message", "id": "old_msg_1"})],
        compaction_id: Some("cmp_old".to_string()),
        is_compacted: true,
        logical_message_count: 2,
        logical_context_hash: tact_llm::context_hash(&old_messages).unwrap(),
    });
    store
        .replace_session_messages_and_provider_state(
            "rollback-session",
            &old_messages,
            Some(&old_state),
        )
        .await
        .unwrap();

    // Force a database insertion failure mid-replacement: the transaction
    // aborts after deleting old messages and inserting the first new one.
    store.inject_message_insert_failure().await.unwrap();

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (mut agent, work_dir) = build_responses_test_agent(&server.uri(), Some(agent_tx));
    agent = agent.with_session("rollback-session".to_string(), Arc::new(store));
    agent.runtime.context = old_messages.clone();
    agent.runtime.provider_state = Some(old_state.clone());

    let (agent, updates) = drive_compact_command(agent, work_dir, agent_rx).await;

    assert!(
        updates.iter().any(|update| matches!(
            update,
            AgentUpdate::Error(error) if error.to_string().contains("Compaction failed")
        )),
        "expected compaction failure error, got: {updates:?}"
    );

    // Runtime state rolled back: context and provider state are untouched.
    assert_eq!(
        messages_json(&agent.runtime.context),
        messages_json(&old_messages),
        "runtime context must equal the old committed context"
    );
    assert_eq!(
        agent.runtime.provider_state.as_ref(),
        Some(&old_state),
        "runtime provider state must equal the old committed state"
    );

    // Database state rolled back: messages and responses_states are intact.
    let dyn_store = agent.runtime.session_store.as_ref().expect("session store");
    assert_eq!(
        messages_json(&dyn_store.load_session("rollback-session").await.unwrap()),
        messages_json(&old_messages),
        "messages table must equal the old messages"
    );
    assert_eq!(
        dyn_store
            .load_provider_state("rollback-session")
            .await
            .unwrap(),
        Some(old_state),
        "responses_states table must equal the old state"
    );
    server.verify().await;
}
