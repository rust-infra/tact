#![allow(clippy::result_large_err)]

use super::*;

async fn seed_unmatched_assistant_tool_calls(
    store: &crate::store::DynSessionStore,
    session_id: &str,
    call_ids: &[&str],
) {
    let assistant = AgentMessage::new(
        AgentRole::Assistant,
        MessageOrigin::Assistant,
        call_ids
            .iter()
            .map(|call_id| AgentContentBlock::ToolUse {
                id: (*call_id).to_string(),
                name: "read_file".to_string(),
                input: serde_json::json!({"path": "missing.txt"}),
            })
            .collect(),
    );
    store
        .append_agent_message(session_id, &assistant, 0)
        .await
        .unwrap();
}

async fn assert_one_grouped_interrupted_result(
    store: &crate::store::DynSessionStore,
    session_id: &str,
    expected_ids: &[&str],
) {
    let messages = store.load_agent_session(session_id).await.unwrap();
    let grouped = messages
        .iter()
        .filter(|message| message.origin == MessageOrigin::ToolResult)
        .collect::<Vec<_>>();
    assert_eq!(
        grouped.len(),
        1,
        "crash reconciliation must persist one grouped result: {messages:?}"
    );
    assert_eq!(
        grouped[0].tool_result_ids().collect::<Vec<_>>(),
        expected_ids
    );
    assert!(
        grouped[0].content.iter().all(|block| matches!(
            block,
            AgentContentBlock::ToolResult { content, .. }
                if content.contains("interrupted before result commit")
        )),
        "every missing call must carry the interrupted reason: {:?}",
        grouped[0].content
    );
}

#[tokio::test]
async fn local_session_reconciles_a_durable_unmatched_tool_call_before_the_request() {
    ensure_config();
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();
    seed_unmatched_assistant_tool_calls(&store, "session-1", &["orphan-1", "orphan-2"]).await;

    let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<Message>>::new()));
    let captured = requests.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        captured
            .lock()
            .expect("request capture")
            .push(request.messages.clone());
        Ok((
            vec![make_text_block("resumed")],
            Some(StopReason::EndTurn),
            None,
        ))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("local_crash_recovery"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_session("session-1".to_string(), store.clone());

    agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "resume")))
        .await
        .expect("resumed local activation completes");

    let first_request = requests
        .lock()
        .expect("request capture")
        .first()
        .cloned()
        .expect("resumed request");
    let (uses, results) = tool_pairing(&first_request);
    assert_eq!(uses, ["orphan-1", "orphan-2"]);
    assert_eq!(results, ["orphan-1", "orphan-2"]);
    assert_eq!(
        tool_pairing(&store.load_session("session-1").await.unwrap()),
        (
            vec!["orphan-1".to_string(), "orphan-2".to_string()],
            vec!["orphan-1".to_string(), "orphan-2".to_string()]
        )
    );
    assert_one_grouped_interrupted_result(&store, "session-1", &["orphan-1", "orphan-2"]).await;
}

#[tokio::test]
async fn responses_session_reconciles_a_durable_unmatched_tool_call_before_the_request() {
    ensure_config();
    let server = MockServer::start().await;
    let completed = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_resume",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "msg_resume",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "resumed"
                }]
            }],
            "usage": {
                "input_tokens": 4,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 1,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 5
            }
        }
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {completed}\n\n")),
        )
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();
    seed_unmatched_assistant_tool_calls(&store, "session-1", &["orphan-r1", "orphan-r2"]).await;

    let mut agent = responses_test_agent("responses_crash_recovery", &server.uri())
        .with_session("session-1".to_string(), store.clone());
    agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "resume")))
        .await
        .expect("resumed Responses activation completes");

    let requests = server.received_requests().await.expect("received requests");
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body is JSON");
    let outputs = body["input"]
        .as_array()
        .expect("Responses input")
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();
    assert_eq!(outputs.len(), 2, "both missing calls need outputs: {body}");
    for call_id in ["orphan-r1", "orphan-r2"] {
        assert!(
            outputs.iter().any(|item| {
                item["call_id"] == call_id
                    && item["output"]
                        .as_str()
                        .is_some_and(|output| output.contains("interrupted before result commit"))
            }),
            "missing interrupted output for {call_id}: {body}"
        );
    }
    assert_one_grouped_interrupted_result(&store, "session-1", &["orphan-r1", "orphan-r2"]).await;
}

#[tokio::test]
async fn ensure_session_restores_matching_provider_state() {
    ensure_config();
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();
    let state = ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
        version: 1,
        provider: "openai_responses".to_string(),
        base_url: "https://api.openai.com/v1".to_string(),
        model: "mock-model".to_string(),
        input_items: vec![serde_json::json!({"type": "message"})],
        compaction_id: Some("cmp_1".to_string()),
        is_compacted: true,
        logical_message_count: 1,
        logical_context_hash: "abc".to_string(),
    });
    store
        .replace_session_messages_and_provider_state("session-1", &[], Some(&state))
        .await
        .unwrap();

    let mut agent = responses_test_agent("ensure_session_restore", "https://api.openai.com/v1");
    agent = agent.with_session("session-1".to_string(), store);
    agent
        .ensure_session()
        .await
        .expect("matching state must load");

    let Some(ProviderConversationState::OpenAiResponses(loaded)) = &agent.runtime.provider_state
    else {
        panic!("provider state must be loaded from the store");
    };
    assert_eq!(loaded.compaction_id.as_deref(), Some("cmp_1"));
    assert!(loaded.is_compacted);
}

#[tokio::test]
async fn ensure_session_rejects_provider_state_bound_to_other_base_url() {
    ensure_config();
    let dir = tempfile::tempdir().unwrap();
    let store = crate::store::open_sqlite_session_store(&dir.path().join("session.db"))
        .await
        .unwrap();
    store
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();
    let state = ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
        version: 1,
        provider: "openai_responses".to_string(),
        base_url: "https://other.example.com/v1".to_string(),
        model: "mock-model".to_string(),
        input_items: Vec::new(),
        compaction_id: None,
        is_compacted: false,
        logical_message_count: 0,
        logical_context_hash: "abc".to_string(),
    });
    store
        .replace_session_messages_and_provider_state("session-1", &[], Some(&state))
        .await
        .unwrap();

    let mut agent = responses_test_agent("ensure_session_binding", "https://api.openai.com/v1");
    agent = agent.with_session("session-1".to_string(), store);
    let error = agent
        .ensure_session()
        .await
        .expect_err("base URL mismatch must be rejected before any LLM call");
    assert!(
        error.to_string().contains("base URL"),
        "error must describe the binding mismatch, got: {error}"
    );
    assert!(
        agent.runtime.provider_state.is_none(),
        "mismatched state must not be adopted"
    );
}

// ——— lifecycle hook wiring (Stop / SessionEnd / PreCompact / PostCompact) ———
