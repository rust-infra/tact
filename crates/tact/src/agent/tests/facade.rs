use super::*;

#[test]
fn failed_model_build_restores_the_previous_model_and_provider_info() {
    ensure_config();
    let mut agent = responses_test_agent("failed_model_commit", "https://api.openai.com/v1")
        .with_stable_provider()
        .with_provider_info(tact_llm::ProviderInfo {
            api_key: "test-key".into(),
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-old".into(),
            provider: tact_llm::ProviderKind::OpenAi,
            protocol: tact_llm::OpenAiProtocol::Responses,
            responses_compact_threshold: None,
        });
    let previous_model = agent.model().to_string();
    let invalid = tact_llm::ProviderInfo {
        api_key: "test-key".into(),
        base_url: "http://[::1".into(),
        model: "gpt-new".into(),
        provider: tact_llm::ProviderKind::OpenAi,
        protocol: tact_llm::OpenAiProtocol::Responses,
        responses_compact_threshold: None,
    };

    let error = agent
        .try_set_model_with_provider_info("gpt-new".into(), Some(invalid))
        .expect_err("an invalid replacement endpoint must fail");

    assert!(error.to_string().contains("replacement provider"));
    assert_eq!(agent.model(), previous_model);
    assert_eq!(
        agent
            .model_provider_info
            .as_ref()
            .map(|info| info.model.as_str()),
        Some("gpt-old")
    );
}

#[tokio::test]
async fn successful_model_switch_changes_the_next_responses_wire_model() {
    ensure_config();
    let server = MockServer::start().await;
    let completed = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_switch",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-switched",
            "output": [{
                "type": "message",
                "id": "msg_switch",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "switched"
                }]
            }],
            "usage": {
                "input_tokens": 1,
                "input_tokens_details": {"cached_tokens": 0},
                "output_tokens": 1,
                "output_tokens_details": {"reasoning_tokens": 0},
                "total_tokens": 2
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

    let mut agent = responses_test_agent("stable_model_switch", &server.uri())
        .with_stable_provider()
        .with_provider_info(tact_llm::ProviderInfo {
            api_key: "test-key".into(),
            base_url: server.uri(),
            model: "gpt-old".into(),
            provider: tact_llm::ProviderKind::OpenAi,
            protocol: tact_llm::OpenAiProtocol::Responses,
            responses_compact_threshold: None,
        });
    agent
        .try_set_model_with_provider_info(
            "gpt-switched".into(),
            Some(tact_llm::ProviderInfo {
                api_key: "test-key".into(),
                base_url: server.uri(),
                model: "gpt-switched".into(),
                provider: tact_llm::ProviderKind::OpenAi,
                protocol: tact_llm::OpenAiProtocol::Responses,
                responses_compact_threshold: Some(160_000),
            }),
        )
        .unwrap();
    agent
        .agent_loop(Some(Message::new_text(Role::User, "switch")))
        .await
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(body["model"], "gpt-switched");
    assert_eq!(body["context_management"][0]["compact_threshold"], 160_000);
}

#[test]
fn agent_settings_snapshot_survives_global_config_override() {
    ensure_config();
    let context = test_context("agent_settings_snapshot");
    let router = crate::tool::toolset();
    let mcp = crate::mcp::MCPToolRouter::new();
    let perm =
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap();

    let tiny = crate::config::AgentSettings {
        model: "mock-model".to_string(),
        reasoning_effort: None,
        model_context_window: 500,
        max_tokens: 1024,
        thinking_budget: 0,
        snapshot_max_items: 10,
        notifications_enabled: false,
        max_token_usage_bodies: crate::store::session_store::MAX_TOKEN_USAGE_BODIES,
        micro_compact_enabled: true,
        memory_enabled: true,
        skill_body_auto_inject: false,
        skill_dirs: Vec::new(),
        skill_description_cap: crate::skill::DEFAULT_SKILL_DESCRIPTION_CAP,
        skill_disable: Vec::new(),
        instruction_sources: crate::config::InstructionSources::default(),
        auto_memory_directory: None,
        subagent: None,
    };
    let agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        router,
        mcp,
        perm,
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_agent_settings(tiny.clone());

    #[cfg(feature = "test-support")]
    {
        let mut big = crate::config::settings();
        big.agent.model_context_window = 900_000;
        crate::config::install_or_override(big);
    }

    assert_eq!(agent.model_context_window(), 500);
    assert_eq!(agent.max_tokens(), 1024);
    assert_eq!(
        agent.agent_settings.model_context_window,
        tiny.model_context_window
    );
}

#[test]
fn agent_new_initializes_with_correct_tool_specs() {
    let context = test_context("agent_new_test");
    let router = crate::tool::toolset();
    let mcp = crate::mcp::MCPToolRouter::new();
    let perm =
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap();

    let agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        router,
        mcp,
        perm,
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    );

    let specs = agent.all_tool_specs();
    assert!(!specs.is_empty(), "tool specs should not be empty");
    let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
    assert!(names.contains(&"bash"));
    assert!(names.contains(&"read_file"));
    assert!(names.contains(&"write_file"));
}

/// A one-tool MCP client whose server sent `instructions`.
fn mcp_client_with_instructions(server: &str, instructions: &str) -> crate::mcp::McpClient {
    use std::borrow::Cow;
    use std::sync::Arc;

    use rmcp::model::{Content, JsonObject, Tool as McpTool};
    use serde_json::json;

    let tool = McpTool {
        name: Cow::Borrowed("recall"),
        title: None,
        description: Some(Cow::Borrowed("Recall a note")),
        input_schema: Arc::new(JsonObject::from_iter([(
            "type".to_string(),
            json!("object"),
        )])),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    };
    let service = crate::mcp::MockMcpService::new(vec![tool.clone()], |_| {
        Ok(rmcp::model::CallToolResult::success(vec![Content::text(
            "ok",
        )]))
    })
    .with_instructions(instructions);
    crate::mcp::McpClient::with_service(server, vec![tool], Arc::new(service))
}

#[test]
fn the_system_prompt_carries_mcp_server_instructions() {
    ensure_config();
    let mut mcp = crate::mcp::MCPToolRouter::new();
    mcp.register_client(mcp_client_with_instructions(
        "basic-memory",
        "Call recent_activity to orient yourself.",
    ));

    let mut agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        test_context("mcp_instructions_prompt"),
        crate::tool::toolset(),
        mcp,
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Dynamic,
    );

    let prompt = agent.build_system_prompt().unwrap();
    assert!(prompt.contains("# MCP server instructions"), "{prompt}");
    assert!(prompt.contains("## basic-memory"), "{prompt}");
    assert!(
        prompt.contains("Call recent_activity to orient yourself."),
        "{prompt}"
    );
}

#[tokio::test]
async fn responses_agent_loop_passes_and_commits_provider_state() {
    ensure_config();
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
                    "text": "hello there"
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

    let mut agent = responses_test_agent("responses_agent_loop_state", &server.uri());
    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("agent loop should complete");

    // The logical context got user + assistant messages.
    assert_eq!(agent.runtime.context.len(), 2);

    // The response committed a provider state covering the request
    // messages plus the pushed assistant message (the protocol baseline
    // already contains the terminal output), so the anchor is the
    // post-assistant logical context.
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("agent loop must commit provider state after the LLM response");
    };
    assert_eq!(state.logical_message_count, 2);
    assert_eq!(
        state.logical_context_hash,
        wire_context_hash(&agent.runtime.context),
        "committed anchor must cover the post-assistant logical context"
    );
    assert!(!state.is_compacted);
    assert_eq!(state.input_items.len(), 2, "user input + assistant output");
    assert!(
        state
            .input_items
            .iter()
            .any(|item| item.get("type").and_then(|v| v.as_str()) == Some("message")),
        "baseline must contain the converted messages"
    );

    // The request sent to the endpoint carried the user input converted to
    // wire items (state-aware conversion path was used).
    let requests = server.received_requests().await.expect("received requests");
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body is JSON");
    let input = body["input"].as_array().expect("input array");
    assert_eq!(
        input.len(),
        1,
        "one user message converted for the first request"
    );
    assert_eq!(input[0]["role"], "user");
}

#[tokio::test]
async fn kernel_activation_uses_direct_facade_provider_state_mutations() {
    ensure_config();
    let server = MockServer::start().await;
    let completed = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_direct_state",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "msg_direct_state",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "done"
                }]
            }],
            "usage": {
                "input_tokens": 2,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 1,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 3
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

    let mut agent = responses_test_agent("direct_provider_state", &server.uri());
    agent.runtime.provider_state = Some(ProviderConversationState::OpenAiResponses(
        tact_llm::ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: server.uri(),
            model: agent.agent_settings.model.clone(),
            input_items: vec![serde_json::json!({
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "DIRECT PROVIDER STATE"
                }]
            })],
            compaction_id: None,
            is_compacted: false,
            logical_message_count: 0,
            logical_context_hash: tact_llm::context_hash(&[]).unwrap(),
        },
    ));

    agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "new turn")))
        .await
        .expect("kernel activation with direct provider state completes");
    assert!(agent.kernel_activations_for_test() >= 1);

    let requests = server.received_requests().await.expect("received requests");
    assert_eq!(requests.len(), 1);
    let body: serde_json::Value =
        serde_json::from_slice(&requests[0].body).expect("request body is JSON");
    assert!(
        body["input"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| {
                item["content"].as_array().is_some_and(|content| {
                    content
                        .iter()
                        .any(|part| part["text"].as_str() == Some("DIRECT PROVIDER STATE"))
                })
            })),
        "the directly mutated provider baseline must be sent: {body}"
    );
}

#[tokio::test]
async fn cancellation_restores_direct_facade_context_and_provider_state() {
    ensure_config();
    const DIRECT_CONTEXT: &str = "DIRECT CANCEL CONTEXT";
    let mut agent = chat_completions_test_agent("cancel_restores_facade_state");
    let provider_state =
        ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "mock-model".to_string(),
            input_items: vec![serde_json::json!({"type": "message", "id": "cancel"})],
            compaction_id: Some("cancel-state".to_string()),
            is_compacted: true,
            logical_message_count: 0,
            logical_context_hash: tact_llm::context_hash(&[]).unwrap(),
        });
    agent.runtime.context = vec![Message::new_text(Role::User, DIRECT_CONTEXT)];
    agent.runtime.provider_state = Some(provider_state);
    agent.runtime.cancel_flag.cancel();

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "cancel me")))
        .await
        .expect("kernel cancellation returns Ok");

    assert_eq!(outcome.stop, AgentLoopStop::Cancelled);
    assert!(agent.kernel_activations_for_test() >= 1);
    assert!(
        agent
            .runtime
            .context
            .iter()
            .any(|message| crate::extract_text(&message.content) == DIRECT_CONTEXT)
    );
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("direct provider state must survive cancellation");
    };
    assert_eq!(state.compaction_id.as_deref(), Some("cancel-state"));
}

#[tokio::test]
async fn kernel_error_restores_direct_facade_state_without_mid_run_fallback() {
    ensure_config();
    const DIRECT_CONTEXT: &str = "DIRECT ERROR CONTEXT";
    let mock = MockClient::with_responder(|_request, _idx| {
        Err(tact_llm::LlmError::Mock("forced kernel failure".into()))
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("error_restores_facade_state"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );
    let provider_state =
        ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "mock-model".to_string(),
            input_items: vec![serde_json::json!({"type": "message", "id": "error"})],
            compaction_id: Some("error-state".to_string()),
            is_compacted: true,
            logical_message_count: 0,
            logical_context_hash: tact_llm::context_hash(&[]).unwrap(),
        });
    agent.runtime.context = vec![Message::new_text(Role::User, DIRECT_CONTEXT)];
    agent.runtime.provider_state = Some(provider_state);

    let error = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "fail")))
        .await
        .expect_err("kernel errors must surface");
    assert!(format!("{error:#}").contains("forced kernel failure"));
    assert!(agent.kernel_activations_for_test() >= 1);
    assert!(
        agent
            .runtime
            .context
            .iter()
            .any(|message| crate::extract_text(&message.content) == DIRECT_CONTEXT)
    );
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("direct provider state must survive the error");
    };
    assert_eq!(state.compaction_id.as_deref(), Some("error-state"));
}

#[tokio::test]
async fn responses_agent_loop_two_turns_never_duplicates_assistant_tool_items() {
    ensure_config();
    let server = MockServer::start().await;
    // Turn 1: reasoning + assistant message + read_file function call
    // (the agent executes the tool and continues). Turn 2: terminal
    // assistant message. Served sequentially by request index.
    let turn1 = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_1",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-5",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "plan"}],
                    "encrypted_content": "encrypted-plan-1",
                    "status": "completed"
                },
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": null,
                        "text": "reading the file"
                    }]
                },
                {
                    "type": "function_call",
                    "arguments": "{\"path\":\"a.txt\"}",
                    "call_id": "call_1",
                    "name": "read_file",
                    "id": "fc_1",
                    "status": "completed"
                }
            ],
            "usage": {
                "input_tokens": 10,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 5,
                "output_tokens_details": { "reasoning_tokens": 2 },
                "total_tokens": 15
            }
        }
    });
    let turn2 = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_2",
            "object": "response",
            "created_at": 3,
            "completed_at": 4,
            "status": "completed",
            "model": "gpt-5",
            "output": [{
                "type": "message",
                "id": "msg_2",
                "role": "assistant",
                "status": "completed",
                "content": [{
                    "type": "output_text",
                    "annotations": [],
                    "logprobs": null,
                    "text": "done reading"
                }]
            }],
            "usage": {
                "input_tokens": 12,
                "input_tokens_details": { "cached_tokens": 0 },
                "output_tokens": 2,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 14
            }
        }
    });
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let call_count_for_responder = call_count.clone();
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(move |_request: &wiremock::Request| {
            let index = call_count_for_responder.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let body = if index == 0 { &turn1 } else { &turn2 };
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(format!("data: {body}\n\n"))
        })
        .expect(2)
        .mount(&server)
        .await;

    let context = crate::tool::test_support::test_context("responses_two_turns");
    crate::tool::test_support::write_workspace_file(&context.work_dir, "a.txt", "aaa");
    let mut tool_context = context;
    tool_context.events = UpdateSink::default();
    let mut agent = Agent::new(
        LlmProvider::OpenAiResponses(
            tact_llm::openai::responses::OpenAiResponsesAdapter::new(
                "test-key",
                server.uri(),
                None,
            )
            // This wiremock serves an OpenAI-shaped protocol stream
            // (reasoning + encrypted content + item ids), so reasoning
            // replay must stay enabled even though the mock base URL is
            // not api.openai.com.
            .with_replay_prior_reasoning(true),
        ),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    agent
        .agent_loop(Some(Message::new_text(Role::User, "read a.txt")))
        .await
        .expect("agent loop should finish both turns");
    server.verify().await;

    // Logical context: user, assistant (tool call), user (tool result),
    // assistant (final).
    assert_eq!(agent.runtime.context.len(), 4);

    // The committed anchor covers the full post-assistant logical context.
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("agent loop must commit provider state after the LLM response");
    };
    assert_eq!(
        state.logical_message_count,
        agent.runtime.context.len(),
        "committed anchor must match the post-assistant logical context"
    );
    assert_eq!(
        state.logical_context_hash,
        wire_context_hash(&agent.runtime.context)
    );
    assert_eq!(
        state.input_items.len(),
        6,
        "user + reasoning + message + function_call + function_call_output + message"
    );

    // Second request body: the baseline is replayed verbatim and only the
    // new user/tool suffix is converted — no duplicated assistant,
    // reasoning, or function-call items/ids.
    let requests = server.received_requests().await.expect("received requests");
    assert_eq!(requests.len(), 2, "exactly two /responses requests");
    let second: serde_json::Value =
        serde_json::from_slice(&requests[1].body).expect("second request body is JSON");
    let input = second["input"].as_array().expect("second request input");
    assert_eq!(
        input.len(),
        5,
        "baseline (4) + one converted tool-result suffix item, got: {input:?}"
    );
    let reasoning_items = input
        .iter()
        .filter(|item| item["type"] == "reasoning")
        .collect::<Vec<_>>();
    assert_eq!(
        reasoning_items.len(),
        1,
        "reasoning item must not be duplicated, got: {input:?}"
    );
    assert_eq!(reasoning_items[0]["id"], "rs_1");
    let assistant_messages = input
        .iter()
        .filter(|item| item["type"] == "message" && item["role"] == "assistant")
        .collect::<Vec<_>>();
    assert_eq!(
        assistant_messages.len(),
        1,
        "assistant message must not be duplicated, got: {input:?}"
    );
    assert_eq!(assistant_messages[0]["id"], "msg_1");
    let function_calls = input
        .iter()
        .filter(|item| item["type"] == "function_call")
        .collect::<Vec<_>>();
    assert_eq!(
        function_calls.len(),
        1,
        "function_call item must not be duplicated, got: {input:?}"
    );
    assert_eq!(function_calls[0]["call_id"], "call_1");
    assert_eq!(function_calls[0]["id"], "fc_1");
    // The only newly converted item is the tool result of the executed
    // read_file call.
    let outputs = input
        .iter()
        .filter(|item| item["type"] == "function_call_output")
        .collect::<Vec<_>>();
    assert_eq!(
        outputs.len(),
        1,
        "exactly one tool-result suffix item, got: {input:?}"
    );
    assert_eq!(outputs[0]["call_id"], "call_1");
    assert_eq!(outputs[0]["output"], "aaa");
    // Baseline items are replayed verbatim (same ids, same order).
    assert_eq!(input[0]["type"], "message");
    assert_eq!(input[0]["role"], "user");
    assert_eq!(input[1]["type"], "reasoning");
    assert_eq!(input[2]["id"], "msg_1");
    assert_eq!(input[3]["id"], "fc_1");
    assert_eq!(input[4]["type"], "function_call_output");
}

#[tokio::test]
async fn responses_agent_loop_automatic_compaction_stays_single_stream_call() {
    ensure_config();
    let server = MockServer::start().await;
    // A streamed terminal response that includes a provider-side
    // automatic-compaction item (the opaque encrypted content must never
    // surface in diagnostics).
    let completed = serde_json::json!({
        "type": "response.completed",
        "sequence_number": 1,
        "response": {
            "id": "resp_auto_1",
            "object": "response",
            "created_at": 1,
            "completed_at": 2,
            "status": "completed",
            "model": "gpt-5",
            "output": [
                {
                    "type": "reasoning",
                    "id": "rs_1",
                    "summary": [{"type": "summary_text", "text": "reasoning"}],
                    "encrypted_content": "opaque-reasoning-encrypted-content",
                    "status": "completed"
                },
                {
                    "type": "message",
                    "id": "msg_1",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{
                        "type": "output_text",
                        "annotations": [],
                        "logprobs": null,
                        "text": "automatic compaction handled inline"
                    }]
                },
                {
                    "type": "compaction",
                    "id": "cmp_auto_01",
                    "encrypted_content": "opaque-encrypted-compaction-content"
                }
            ],
            "usage": {
                "input_tokens": 160012,
                "input_tokens_details": { "cached_tokens": 120000 },
                "output_tokens": 120,
                "output_tokens_details": { "reasoning_tokens": 0 },
                "total_tokens": 160132
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
    // Automatic compaction must NOT trigger a second HTTP call to the
    // explicit compact endpoint.
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
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
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut agent = responses_test_agent("responses_auto_compact_stream", &server.uri());
    agent = agent
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store);
    agent
        .agent_loop(Some(Message::new_text(Role::User, "hi")))
        .await
        .expect("agent loop should complete with provider-side compaction");

    // Exactly one streamed /responses request; no explicit compact call.
    server.verify().await;

    // The compaction item committed to the provider state without a
    // separate HTTP call.
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("agent loop must commit provider state after the LLM response");
    };
    assert!(
        state.is_compacted,
        "provider state must reflect auto compaction"
    );
    assert_eq!(state.compaction_id.as_deref(), Some("cmp_auto_01"));

    // Usage accounting stays on the ordinary stream call: exactly one
    // `stream` row and no `responses_compact` row for a call that never
    // happened.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("session.db").display()
    ))
    .await
    .unwrap();
    let rows = sqlx::query("SELECT call_type FROM token_usages WHERE session_id = 'session-1'")
        .fetch_all(&pool)
        .await
        .unwrap();
    let call_types: Vec<String> = rows
        .iter()
        .map(|row| row.try_get("call_type").unwrap())
        .collect();
    assert_eq!(
        call_types,
        vec!["stream"],
        "automatic compaction must stay associated with the stream call, \
         got: {call_types:?}"
    );

    // Encrypted compaction content and the reasoning encrypted envelope
    // never surface in Info diagnostics: the compaction payload is
    // strictly provider state/request body, and the reasoning envelope
    // lives only in the internal Thinking.signature.
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    for secret in [
        "opaque-encrypted-compaction-content",
        "opaque-reasoning-encrypted-content",
    ] {
        assert!(
            updates
                .iter()
                .filter_map(|u| match u {
                    tact_protocol::AgentUpdate::Info(msg) => Some(msg.as_str()),
                    _ => None,
                })
                .all(|msg| !msg.contains(secret)),
            "encrypted payload {secret:?} must never surface in Info updates: {updates:?}"
        );
    }
}
