use super::*;

#[tokio::test]
async fn command_compaction_uses_kernel_owned_replacement_and_events() {
    ensure_config();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mock = MockClient::new(vec![(
        vec![make_text_block("kernel command summary")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("kernel_command_compaction"),
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".into()),
    )
    .with_ui_channel(tx);
    agent.runtime.context = vec![
        Message::new_text(Role::User, "task"),
        Message::new_text(Role::Assistant, "result"),
    ];

    agent
        .compact_history_with_trigger(CompactTrigger::Command, None)
        .await
        .unwrap();

    assert!(agent.runtime.compact_state.has_compacted);
    assert_eq!(
        agent.runtime.session_start_source,
        SessionStartSource::Compact
    );
    assert!(agent.runtime.session_start_hooks_pending);
    assert!(
        agent
            .runtime
            .context
            .iter()
            .any(|message| message.kind == MessageKind::Summary),
        "{:?}",
        agent.runtime.context
    );
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    assert!(
        updates
            .iter()
            .any(|update| matches!(update, AgentUpdate::Info(text) if text.contains("Compaction complete (Command)."))),
        "{updates:?}"
    );
}

#[tokio::test]
async fn a_server_that_grows_its_tool_list_reaches_the_next_request() {
    // The agent sends a *cached* snapshot, so a refresh has to do both
    // halves: re-list the server, and rebuild the cache. Either alone leaves
    // the model with the list the handshake advertised.
    use std::borrow::Cow;
    use std::sync::Arc;

    use rmcp::model::{Content, JsonObject, Tool as McpTool};

    let tool = |name: &'static str| McpTool {
        name: Cow::Borrowed(name),
        title: None,
        description: None,
        input_schema: Arc::new(JsonObject::new()),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    };

    let service = Arc::new(crate::mcp::MockMcpService::new(
        vec![tool("search")],
        |_| {
            Ok(rmcp::model::CallToolResult::success(vec![Content::text(
                "ok",
            )]))
        },
    ));
    let mut mcp = crate::mcp::MCPToolRouter::new();
    mcp.register_client(crate::mcp::McpClient::with_service(
        "bm",
        vec![tool("search")],
        service.clone(),
    ));

    let mut agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        test_context("mcp_tool_list_changed"),
        crate::tool::toolset(),
        mcp,
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let names = |agent: &Agent| -> Vec<String> {
        agent
            .all_tool_specs()
            .into_iter()
            .map(|spec| spec.name)
            .collect()
    };
    assert!(!names(&agent).contains(&"mcp__bm__recall".to_string()));

    // The server grows and says so.
    service.set_tools(vec![tool("search"), tool("recall")]);
    service.announce_tools_changed();

    agent.refresh_mcp_tools().await;

    assert!(
        names(&agent).contains(&"mcp__bm__recall".to_string()),
        "{:?}",
        names(&agent)
    );

    // A second pass with nothing announced must not disturb it.
    agent.refresh_mcp_tools().await;
    assert!(names(&agent).contains(&"mcp__bm__recall".to_string()));
}

#[tokio::test]
async fn kernel_refreshes_a_mid_turn_mcp_catalog_before_the_next_request() {
    ensure_config();
    use std::borrow::Cow;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    use rmcp::model::{Content, JsonObject, Tool as McpTool};

    let tool = |name: &'static str| McpTool {
        name: Cow::Borrowed(name),
        title: None,
        description: None,
        input_schema: Arc::new(JsonObject::new()),
        output_schema: None,
        annotations: None,
        execution: None,
        icons: None,
        meta: None,
    };
    let service = Arc::new(crate::mcp::MockMcpService::new(
        vec![tool("search")],
        |_| {
            Ok(rmcp::model::CallToolResult::success(vec![Content::text(
                "ok",
            )]))
        },
    ));
    let mut mcp = crate::mcp::MCPToolRouter::new();
    mcp.register_client(crate::mcp::McpClient::with_service(
        "bm",
        vec![tool("search")],
        service.clone(),
    ));

    let first_saw_recall = Arc::new(AtomicBool::new(false));
    let first_saw_recall_for_mock = first_saw_recall.clone();
    let service_for_mock = service.clone();
    let mock = MockClient::with_responder(move |request, idx| {
        let names = request
            .tools
            .as_ref()
            .into_iter()
            .flatten()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        match idx {
            0 => {
                first_saw_recall_for_mock
                    .store(names.contains(&"mcp__bm__recall"), Ordering::SeqCst);
                service_for_mock.set_tools(vec![tool("search"), tool("recall")]);
                service_for_mock.announce_tools_changed();
                Ok((
                    vec![ContentBlock::ToolUse {
                        id: "mcp-search".into(),
                        name: "mcp__bm__search".into(),
                        input: serde_json::json!({}),
                    }],
                    Some(StopReason::ToolUse),
                    None,
                ))
            }
            1 => {
                assert!(
                    names.contains(&"mcp__bm__recall"),
                    "the catalog change must reach the very next request: {names:?}"
                );
                Ok((
                    vec![make_text_block("refreshed")],
                    Some(StopReason::EndTurn),
                    None,
                ))
            }
            other => panic!("unexpected request {other}"),
        }
    });
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        test_context("kernel_mcp_tool_list_changed"),
        crate::tool::toolset(),
        mcp,
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Auto)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let outcome = agent
        .agent_loop_detailed(Some(Message::new_text(Role::User, "search")))
        .await
        .expect("kernel activation");

    assert_eq!(outcome.stop, AgentLoopStop::Completed);
    assert!(agent.kernel_activations_for_test() >= 1);
    assert!(!first_saw_recall.load(Ordering::SeqCst));
}

#[test]
fn the_resource_tools_are_offered_only_with_a_connected_server() {
    let mut mcp = crate::mcp::MCPToolRouter::new();
    mcp.register_client(mcp_client_with_instructions("basic-memory", "guidance"));

    let mut agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        test_context("mcp_resource_tools"),
        crate::tool::toolset(),
        mcp,
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    );

    let names: Vec<String> = agent
        .all_tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(
        names.contains(&"list_mcp_resources".to_string()),
        "{names:?}"
    );
    assert!(
        names.contains(&"read_mcp_resource".to_string()),
        "{names:?}"
    );
    // The templates listing is what makes a template-addressed server
    // discoverable at all, so it must be advertised alongside the others.
    assert!(
        names.contains(&"list_mcp_resource_templates".to_string()),
        "{names:?}"
    );

    // No servers: the names must disappear, or the model gets a tool whose
    // only possible answer is "no MCP servers are connected".
    agent.mcp_router = McpCatalogHandle::new(crate::mcp::MCPToolRouter::new());
    agent.rebuild_cached_tool_specs();
    let names: Vec<String> = agent
        .all_tool_specs()
        .into_iter()
        .map(|spec| spec.name)
        .collect();
    assert!(
        !names.contains(&"list_mcp_resources".to_string()),
        "{names:?}"
    );
    assert!(
        !names.contains(&"read_mcp_resource".to_string()),
        "{names:?}"
    );
}

#[test]
fn an_agent_without_mcp_servers_has_no_instructions_section() {
    ensure_config();
    let mut agent = chat_completions_test_agent("no_mcp_instructions");
    agent.system_prompt = AgentSystemPrompt::Dynamic;

    let prompt = agent.build_system_prompt().unwrap();
    assert!(!prompt.contains("# MCP server instructions"), "{prompt}");
}

#[test]
fn enabled_memory_keeps_the_guidance_in_the_system_prompt() {
    ensure_config();
    let mut agent = chat_completions_test_agent("memory_enabled_prompt");
    agent.system_prompt = AgentSystemPrompt::Dynamic;

    let prompt = agent.build_system_prompt().unwrap();
    assert!(prompt.contains("# Memory guidance"), "{prompt}");
}

/// The injected memory section is the **index**, reachable through
/// `load_memory` — not the bodies.
///
/// Drives the real path: a memory saved by the tool, then a prompt built
/// from the same manager, so a regression that folds bodies back into the
/// prompt is caught here rather than in the prompt template.
#[tokio::test]
async fn the_system_prompt_carries_the_index_and_points_at_load_memory() {
    ensure_config();
    let context = crate::tool::test_support::test_context("memory_index_in_prompt");
    context
        .memory_manager
        .lock()
        .unwrap()
        .save_memory(
            "Release Branch",
            "the branch to cut from",
            crate::memory::MemoryType::Project,
            "BODY_SENTINEL cut releases from main.",
        )
        .unwrap();

    let mut agent = chat_completions_test_agent_for("memory_index_in_prompt", context);
    agent.system_prompt = AgentSystemPrompt::Dynamic;

    let prompt = agent.build_system_prompt().unwrap();
    assert!(prompt.contains("## Memory"), "{prompt}");
    assert!(prompt.contains("release_branch.md"), "{prompt}");
    assert!(prompt.contains("load_memory"), "{prompt}");
    assert!(
        !prompt.contains("BODY_SENTINEL"),
        "bodies must not be injected: {prompt}"
    );
}

#[test]
fn disabled_memory_is_absent_from_the_system_prompt() {
    ensure_config();
    let mut agent = chat_completions_test_agent("memory_disabled_prompt");
    agent.system_prompt = AgentSystemPrompt::Dynamic;
    agent.agent_settings.memory_enabled = false;

    let prompt = agent.build_system_prompt().unwrap();
    assert!(!prompt.contains("# Memory guidance"), "{prompt}");
    assert!(!prompt.contains("# Memory"), "{prompt}");
}

/// The advisory reaches the rendered prompt, and only when the window is
/// actually filling — driven through the real `build_system_prompt`.
#[test]
fn a_filling_window_steers_the_prompt_at_subagents() {
    ensure_config();

    // Room to spare: no advisory.
    let mut roomy = chat_completions_test_agent("advisory_roomy");
    roomy.system_prompt = AgentSystemPrompt::Dynamic;
    roomy.agent_settings.model_context_window = 200_000;
    let prompt = roomy.build_system_prompt().unwrap();
    assert!(!prompt.contains("spawn_subagent"), "{prompt}");

    // Same agent, a window that is 65% full: the advisory appears.
    let mut full = chat_completions_test_agent("advisory_full");
    full.system_prompt = AgentSystemPrompt::Dynamic;
    full.agent_settings.model_context_window = 200_000;
    full.runtime.last_token_total = 130_000;
    let prompt = full.build_system_prompt().unwrap();
    assert!(prompt.contains("spawn_subagent"), "{prompt}");
    assert!(prompt.contains("60% full"), "{prompt}");

    // It lands in the dynamic section, which is the one allowed to change
    // between tasks without invalidating the cached prefix.
    let (_, dynamic) = prompt
        .split_once("=== DYNAMIC_BOUNDARY ===")
        .expect("template emits the boundary");
    assert!(dynamic.contains("spawn_subagent"), "{dynamic}");
}

/// The advice names `spawn_subagent`, so it must not appear when that tool
/// is not offered — a prompt pointing at a tool the model does not have is
/// worse than silence.
#[test]
fn a_full_window_stays_quiet_when_subagents_are_not_offered() {
    ensure_config();
    let mut agent = chat_completions_test_agent("advisory_no_subagent");
    agent.system_prompt = AgentSystemPrompt::Dynamic;
    agent.agent_settings.model_context_window = 200_000;
    agent.runtime.last_token_total = 130_000;
    // The premise of the test: a full window would otherwise trigger.
    assert!(crate::compact::context_pressure_advisory(130_000, 200_000));
    assert!(
        agent.has_tool("spawn_subagent"),
        "fixture must start with it"
    );

    // Make the premise of *this* test explicit: the same full window, with
    // the tool removed from the set the request is built from.
    agent
        .cached_tool_specs
        .retain(|spec| spec.name != "spawn_subagent");
    assert!(!agent.has_tool("spawn_subagent"));

    let prompt = agent.build_system_prompt().unwrap();
    assert!(!prompt.contains("spawn_subagent"), "{prompt}");
    assert!(!prompt.contains("Context window is over"), "{prompt}");
}

#[tokio::test]
async fn responses_compact_history_dispatches_to_native_endpoint() {
    ensure_config();
    let server = MockServer::start().await;
    let fixture = serde_json::json!({
        "id": "cmp_sanitized_01",
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
                "id": "cmp_sanitized_01",
                "encrypted_content": "sanitized-encrypted-compaction-content-placeholder"
            }
        ],
        "usage": {
            "input_tokens": 1200,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens": 340,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": 1540
        }
    });
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
        .expect(1)
        .mount(&server)
        .await;
    // No local summary `create_message()` may be attempted: an ordinary
    // `/responses` request must never arrive.
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(500))
        .expect(0)
        .mount(&server)
        .await;

    let mut agent = responses_test_agent("responses_compact_native", &server.uri());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    agent = agent.with_ui_channel(tx);
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    agent
        .compact_history(None)
        .await
        .expect("native compaction should succeed");

    // Exactly one native compact request; no local summary `create_message`
    // request may hit a `/responses` endpoint.
    server.verify().await;

    // The runtime provider state carries the fixture compaction id and the
    // replacement baseline.
    let Some(ProviderConversationState::OpenAiResponses(state)) = &agent.runtime.provider_state
    else {
        panic!("runtime provider state must be set after native compaction");
    };
    assert_eq!(state.compaction_id.as_deref(), Some("cmp_sanitized_01"));
    assert!(state.is_compacted);
    assert_eq!(state.logical_message_count, 2);
    assert_eq!(
        state.input_items.len(),
        2,
        "retained function_call_output + compaction item"
    );
    // The opaque encrypted content is preserved in the protocol baseline
    // (it must be replayed to the endpoint) but never surfaces in TUI
    // diagnostics.
    let encrypted = state
        .input_items
        .iter()
        .find_map(|item| item.get("encrypted_content").and_then(|v| v.as_str()))
        .expect("compaction item must carry encrypted_content");
    assert!(!encrypted.is_empty());
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    let info_messages: Vec<&str> = updates
        .iter()
        .filter_map(|u| match u {
            tact_protocol::AgentUpdate::Info(msg) => Some(msg.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        info_messages
            .iter()
            .any(|msg| msg.contains("[responses compacted: items=2, id=cmp_sanitize]")),
        "success Info must display only a bounded compaction id prefix, got: {updates:?}"
    );
    assert!(
        info_messages
            .iter()
            .all(|msg| !msg.contains("cmp_sanitized_01")),
        "full compaction id must never surface in Info updates, got: {updates:?}"
    );
    assert!(
        info_messages.iter().all(|msg| !msg.contains(encrypted)),
        "encrypted compaction content must never surface in Info updates, got: {updates:?}"
    );
    assert_eq!(
        agent.runtime.context.len(),
        2,
        "native compaction keeps the logical context unchanged"
    );
}

#[tokio::test]
async fn deepseek_responses_compact_falls_back_to_local_summary() {
    ensure_config();
    // The DeepSeek endpoint lacks `/responses/compact` (verified live
    // 2026-08-02: it returns an empty body), so DeepSeek + Responses must
    // compact through the local summary path and clear the stale baseline.

    let server = MockServer::start().await;
    // The local summary request is an ordinary (non-streaming) `/responses`
    // call; DeepSeek serves it fine.
    let summary_fixture = serde_json::json!({
        "id": "resp_summary",
        "object": "response",
        "created_at": 1,
        "completed_at": 2,
        "status": "completed",
        "model": "deepseek-v4-flash",
        "output": [{
            "type": "message",
            "id": "msg_summary",
            "role": "assistant",
            "status": "completed",
            "content": [{
                "type": "output_text",
                "annotations": [],
                "logprobs": null,
                "text": "compaction summary text"
            }]
        }],
        "usage": {
            "input_tokens": 10,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens": 2,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": 12
        }
    });
    Mock::given(method("POST"))
        .and(path("/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_json(summary_fixture))
        .mount(&server)
        .await;
    // `/responses/compact` must never be called for DeepSeek.
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let mut agent = responses_test_agent("deepseek_responses_local_compact", &server.uri())
        .with_provider_kind(tact_llm::ProviderKind::DeepSeek);
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    agent
        .compact_history(None)
        .await
        .expect("local summary compaction should succeed for DeepSeek Responses");

    server.verify().await;

    // The old Responses baseline covers the pre-compact history and must
    // not survive the rebuild.
    assert!(
        agent.runtime.provider_state.is_none(),
        "stale Responses baseline must be cleared after local compaction"
    );
    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("compaction summary text"),
        "rebuilt context must contain the summary, got: {context_text}"
    );
}

#[tokio::test]
async fn local_compact_continues_truncated_summary() {
    ensure_config();
    use tact_protocol::AgentUpdate;

    let context = test_context("local_compact_continue");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    // First summary call hits the output limit (MaxTokens); the partial
    // summary must be continued on the next call (EndTurn).
    let mock = MockClient::new(vec![
        (
            vec![make_text_block("summary part one")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("summary part two")],
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
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_ui_channel(tx);
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact with truncated summary must continue and succeed");

    // Both the truncated part and the continuation end up in the rebuilt
    // context (the continuation request carries the partial assistant
    // message, so both blocks are merged into the final summary).
    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("summary part one"),
        "truncated summary part missing: {context_text}"
    );
    assert!(
        context_text.contains("summary part two"),
        "continuation summary part missing: {context_text}"
    );

    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    assert!(
        updates.iter().any(|u| {
            matches!(u, AgentUpdate::Info(msg) if msg.contains("[compact continue 1/5]"))
        }),
        "expected a compact-continue Info update, got: {updates:?}"
    );
}

#[tokio::test]
async fn local_compact_continues_through_multiple_truncations() {
    ensure_config();
    let context = test_context("local_compact_continue_multi");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx);

    // Two consecutive truncations, then a completed call.
    let mock = MockClient::new(vec![
        (
            vec![make_text_block("part one")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("part two")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("part three")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    );
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact must survive repeated truncations");

    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("part one") && context_text.contains("part two"),
        "all truncated parts must be retained: {context_text}"
    );
    assert!(
        context_text.contains("part three"),
        "final continuation part missing: {context_text}"
    );
}

#[tokio::test]
async fn local_compact_accepts_partial_summary_when_continuations_exhausted() {
    ensure_config();
    let context = test_context("local_compact_continue_exhausted");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx);

    // Every call is truncated: MAX_COMPACT_SUMMARY_ATTEMPTS (5)
    // continuations run, then the partial summary is accepted as
    // best-effort instead of failing the whole compaction.
    let mock = MockClient::new(vec![
        (
            vec![make_text_block("partial one")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("partial two")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("partial three")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("partial four")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("partial five")],
            Some(StopReason::MaxTokens),
        ),
        (
            vec![make_text_block("partial six")],
            Some(StopReason::MaxTokens),
        ),
    ]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    );
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));

    agent
        .compact_history(None)
        .await
        .expect("exhausted continuations must not fail compaction");

    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("partial one") && context_text.contains("partial two"),
        "partial summary must still be used: {context_text}"
    );
}

#[test]
fn compact_summary_server_default_effort_tiers() {
    use tact_llm::ProviderKind;
    // Only providers that reason by **server default** when the summary
    // request omits effort get headroom: DeepSeek and Kimi K3 default
    // thinking ON + effort high server-side, everyone else needs none.
    assert_eq!(
        compact_summary_server_default_effort(&ProviderKind::OpenAi),
        None
    );
    assert_eq!(
        compact_summary_server_default_effort(&ProviderKind::Anthropic),
        None
    );
    assert_eq!(
        compact_summary_server_default_effort(&ProviderKind::DeepSeek),
        Some(tact_llm::OpenAiReasoningEffort::High)
    );
    assert_eq!(
        compact_summary_server_default_effort(&ProviderKind::Kimi),
        Some(tact_llm::OpenAiReasoningEffort::High)
    );
    assert_eq!(
        compact_summary_server_default_effort(&ProviderKind::Custom("other".to_string())),
        None
    );
}

#[test]
fn compact_effort_reserve_bucket_tiers() {
    use tact_llm::OpenAiReasoningEffort as E;
    // Absolute token buckets, independent of the summary text budget.
    assert_eq!(compact_effort_reserve_tokens(E::None), 0);
    assert_eq!(compact_effort_reserve_tokens(E::Minimal), 2_000);
    assert_eq!(compact_effort_reserve_tokens(E::Low), 2_000);
    assert_eq!(compact_effort_reserve_tokens(E::Medium), 4_000);
    assert_eq!(compact_effort_reserve_tokens(E::High), 8_000);
    assert_eq!(compact_effort_reserve_tokens(E::Xhigh), 16_000);
    assert_eq!(compact_effort_reserve_tokens(E::Max), 16_000);
}

#[test]
fn effort_folds_to_what_deepseek_actually_runs() {
    use tact_llm::OpenAiReasoningEffort as E;
    // Documented compatibility folds (DeepSeek only, per the thinking-mode
    // guide): the request is forwarded verbatim, the budget is not.
    for (requested, actual) in [
        (E::Minimal, E::Low),
        (E::Medium, E::High),
        (E::Xhigh, E::High),
    ] {
        assert_eq!(
            effort_after_provider_fold(&ProviderKind::DeepSeek, requested),
            actual,
            "{requested} must be budgeted as {actual} on DeepSeek"
        );
    }
    // Real tiers pass through untouched.
    for effort in [E::None, E::Low, E::High, E::Max] {
        assert_eq!(
            effort_after_provider_fold(&ProviderKind::DeepSeek, effort),
            effort
        );
    }
    // No other kind documents a fold; OpenAI's enum is real per model.
    for effort in [E::Minimal, E::Medium, E::Xhigh] {
        assert_eq!(
            effort_after_provider_fold(&ProviderKind::OpenAi, effort),
            effort
        );
        assert_eq!(
            effort_after_provider_fold(&ProviderKind::Kimi, effort),
            effort
        );
    }
}

#[test]
fn next_compaction_reserve_never_shrinks() {
    // A small observed reasoning must not shrink an already-large reserve.
    assert_eq!(next_compaction_reserve(0, 2_000, 4_000, 100), 4_000);
}

#[test]
fn next_compaction_reserve_caps_growth_at_double() {
    // Even a huge observed reasoning grows the room by at most 2x per step.
    assert_eq!(next_compaction_reserve(0, 2_000, 4_000, 1_000_000), 8_000);
    // Within the cap, the target covers the observed reasoning plus headroom.
    assert_eq!(next_compaction_reserve(1_500, 2_000, 0, 1_800), 2_250);
}

#[test]
fn next_compaction_reserve_keeps_minimum_headroom() {
    // No effort reserve and no history: start from a quarter of the text
    // budget so a text-only overrun still gets more room.
    assert_eq!(next_compaction_reserve(0, 2_000, 0, 0), 1_000);
    // The effort-implied reserve wins when it is larger.
    assert_eq!(next_compaction_reserve(1_500, 2_000, 0, 0), 3_000);
    // Small observed reasoning is floored at the minimum headroom.
    assert_eq!(next_compaction_reserve(0, 2_000, 0, 400), 500);
}

#[test]
fn compact_summary_effort_ladder_per_provider() {
    use tact_llm::ProviderKind;
    let high = Some(tact_llm::OpenAiReasoningEffort::High);
    let low = tact_llm::OpenAiReasoningEffort::Low;
    let none = tact_llm::OpenAiReasoningEffort::None;

    // Stage 0 always inherits the session effort.
    assert_eq!(compact_summary_effort(&ProviderKind::OpenAi, high, 0), high);
    assert_eq!(
        compact_summary_effort(&ProviderKind::DeepSeek, None, 0),
        None
    );

    // Stage 1+ minimizes: DeepSeek / Kimi K3 drop to `low`.
    assert_eq!(
        compact_summary_effort(&ProviderKind::DeepSeek, None, 1),
        Some(low)
    );
    assert_eq!(
        compact_summary_effort(&ProviderKind::Kimi, high, 2),
        Some(low)
    );
    // OpenAI reasoning models send `none`; without a configured effort the
    // field stays omitted (the model may not support it).
    assert_eq!(
        compact_summary_effort(&ProviderKind::OpenAi, high, 1),
        Some(none)
    );
    assert_eq!(compact_summary_effort(&ProviderKind::OpenAi, None, 1), None);
    // Anthropic / unknown keep omitting the field.
    assert_eq!(
        compact_summary_effort(&ProviderKind::Anthropic, None, 1),
        None
    );
}

#[tokio::test]
async fn local_compact_inherits_session_effort() {
    ensure_config();
    let context = test_context("local_compact_inherits_effort");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let tool_context = context;

    // Capture the summarizer request so we can assert the first attempt
    // inherits the session's reasoning effort while never enabling a
    // Claude-style thinking budget.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
    let seen_arc = seen.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        seen_arc.lock().unwrap().push(request.clone());
        Ok((
            vec![make_text_block("reasoning-aware summary")],
            Some(StopReason::EndTurn),
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
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_provider_kind(tact_llm::ProviderKind::OpenAi)
    .with_ui_channel(tx);
    // High effort + a Claude-style thinking budget on a 128k window: the
    // effort is inherited, the budget is not.
    agent.agent_settings.reasoning_effort = Some(tact_llm::OpenAiReasoningEffort::High);
    agent.agent_settings.thinking_budget = 4096;
    agent.agent_settings.model_context_window = 128_000;
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact without thinking must succeed");

    let requests = seen.lock().unwrap();
    let request = requests
        .first()
        .expect("summarizer request must be captured");
    // Text budget = min(20% × 128k, 2000) = 2000; the inherited high effort
    // adds its 8,000-token reasoning bucket, so the wire max_tokens is
    // 10,000. No Claude-style thinking budget is forwarded.
    assert_eq!(
        request.max_tokens, 10_000,
        "wire max_tokens must be text budget + the inherited effort bucket"
    );
    assert_eq!(
        request.reasoning_effort,
        Some(tact_llm::OpenAiReasoningEffort::High),
        "the first attempt must inherit the configured reasoning effort"
    );
    assert!(
        request.thinking.is_none(),
        "compact summary must not forward a Claude-style thinking budget"
    );

    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("reasoning-aware summary"),
        "rebuilt context must contain the summary: {context_text}"
    );

    // A successful first attempt (0 continuations) must still log its
    // envelope, and the printed `max_tokens` must be the wire value split
    // into its text and reasoning parts.
    drop(requests);
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    let infos: Vec<&str> = updates
        .iter()
        .filter_map(|u| match u {
            AgentUpdate::Info(msg) => Some(msg.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        infos.iter().any(|msg| msg.starts_with(
            "[compact summary 1/6] request model=mock-model max_tokens=10000 (text 2000 + reasoning 8000), reasoning_effort=high, input "
        ) && msg.ends_with(" chars")),
        "first attempt must log its request envelope: {infos:?}"
    );
    assert!(
        infos.iter().any(|msg| msg
            .starts_with("[compact summary 1/6] response stop=end_turn, no usage reported")),
        "first attempt must log its response even without truncation: {infos:?}"
    );
}

#[test]
fn compact_summary_envelope_ceiling_leaves_room_for_the_instructions() {
    // No window → no ceiling.
    assert_eq!(compact_summary_envelope_ceiling(0), u32::MAX);
    // A real window keeps the 10% headroom plus the fixed instructions.
    let instructions = approx_text_tokens(COMPACT_SUMMARY_INSTRUCTIONS) as u32;
    assert_eq!(
        compact_summary_envelope_ceiling(128_000),
        128_000 - 12_800 - instructions
    );
    assert!(compact_summary_envelope_ceiling(500_000) > 65_536);
    // A window too small for its own instructions cannot raise the floor.
    assert_eq!(compact_summary_envelope_ceiling(100), 0);
}

#[tokio::test]
async fn local_compact_envelope_is_at_least_the_configured_output_budget() {
    ensure_config();
    let context = test_context("local_compact_envelope_floor");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
    let seen_arc = seen.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        seen_arc.lock().unwrap().push(request.clone());
        Ok((
            vec![make_text_block("floored summary")],
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
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_provider_kind(tact_llm::ProviderKind::DeepSeek)
    .with_ui_channel(tx);
    // DeepSeek reasons by server default, so its envelope is matter of
    // billing: the computed value is text 2,000 + the high bucket 8,000 =
    // 10,000, which a reasoning-inside-`max_tokens` provider can spend
    // entirely on thinking. The configured output budget floors it.
    agent.agent_settings.max_tokens = 65_536;
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact with a floored envelope must succeed");

    let requests = seen.lock().unwrap();
    let request = requests
        .first()
        .expect("summarizer request must be captured");
    assert_eq!(
        request.max_tokens, 65_536,
        "the summarizer envelope must never be smaller than [agent] max_tokens"
    );
}

#[tokio::test]
async fn local_compact_folds_deepseek_effort_for_the_reserve() {
    ensure_config();
    let context = test_context("local_compact_deepseek_fold");
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mock = MockClient::new(vec![(
        vec![make_text_block("deepseek summary")],
        Some(StopReason::EndTurn),
    )]);

    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_provider_kind(tact_llm::ProviderKind::DeepSeek)
    .with_ui_channel(tx);
    // DeepSeek accepts `medium` but folds it to `high` server-side, so the
    // reserve must be the high bucket while the wire value stays verbatim.
    agent.agent_settings.reasoning_effort = Some(tact_llm::OpenAiReasoningEffort::Medium);
    agent.agent_settings.model_context_window = 128_000;
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact with a folded effort must succeed");

    let mut infos = Vec::new();
    while let Ok(update) = rx.try_recv() {
        if let AgentUpdate::Info(message) = update {
            infos.push(message);
        }
    }
    let envelope = infos
        .iter()
        .find(|msg| msg.starts_with("[compact summary 1/6] request"))
        .expect("the first attempt logs its envelope");
    assert!(
        envelope.contains("(text 2000 + reasoning 8000)"),
        "the reserve must be sized from the effort DeepSeek actually runs: {envelope}"
    );
    assert!(
        envelope.contains("reasoning_effort=medium"),
        "the configured value is still forwarded verbatim: {envelope}"
    );
}

#[tokio::test]
async fn local_compact_keeps_server_default_reasoning_reserve() {
    ensure_config();
    let context = test_context("local_compact_server_default_reserve");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx);

    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
    let seen_arc = seen.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        seen_arc.lock().unwrap().push(request.clone());
        Ok((
            vec![make_text_block("deepseek summary")],
            Some(StopReason::EndTurn),
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
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_provider_kind(tact_llm::ProviderKind::DeepSeek);
    // DeepSeek reasons at effort high by server default even without an
    // explicit effort, so the summary call reserves the high bucket.
    agent.agent_settings.reasoning_effort = None;
    agent.agent_settings.model_context_window = 128_000;
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact with server-default reserve must succeed");

    let requests = seen.lock().unwrap();
    let request = requests
        .first()
        .expect("summarizer request must be captured");
    // Text budget 2000 + the high effort bucket (8000) = 10_000, so
    // DeepSeek's forced reasoning never starves the summary text; but no
    // effort and no thinking budget are sent.
    assert_eq!(
        request.max_tokens, 10_000,
        "DeepSeek keeps the server-default (high) reasoning bucket"
    );
    assert_eq!(request.reasoning_effort, None);
    assert!(request.thinking.is_none());

    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("deepseek summary"),
        "rebuilt context must contain the summary: {context_text}"
    );
}

#[tokio::test]
async fn local_compact_omits_thinking_for_anthropic() {
    ensure_config();
    let context = test_context("local_compact_anthropic_no_thinking");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx);

    // Capture the summarizer request so we can assert no Claude-style
    // thinking budget is forwarded even though one is configured.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::<CreateMessageParams>::new()));
    let seen_arc = seen.clone();
    let mock = MockClient::with_responder(move |request, _idx| {
        seen_arc.lock().unwrap().push(request.clone());
        Ok((
            vec![make_text_block("plain summary")],
            Some(StopReason::EndTurn),
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
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    )
    .with_provider_kind(tact_llm::ProviderKind::Anthropic);
    // Claude-style thinking budget (8k) far above the capped summarizer
    // output budget (2k): the compact call must not forward it.
    agent.agent_settings.reasoning_effort = None;
    agent.agent_settings.thinking_budget = 8_000;
    agent.agent_settings.model_context_window = 128_000;
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));
    agent
        .runtime
        .context
        .push(Message::new_text(Role::Assistant, "second turn"));

    agent
        .compact_history(None)
        .await
        .expect("compact without thinking must succeed");

    let requests = seen.lock().unwrap();
    let request = requests
        .first()
        .expect("summarizer request must be captured");
    // Wire max_tokens = 2000 (Anthropic gets no reasoning reserve); no
    // thinking budget may be forwarded.
    assert_eq!(
        request.max_tokens, 2000,
        "Anthropic text budget stays at the classic cap"
    );
    assert!(
        request.thinking.is_none(),
        "summarizer must not receive a thinking budget"
    );
    assert_eq!(
        request.reasoning_effort, None,
        "Anthropic must not receive a reasoning effort"
    );

    let context_text = serde_json::to_string(&agent.runtime.context).unwrap_or_default();
    assert!(
        context_text.contains("plain summary"),
        "rebuilt context must contain the summary: {context_text}"
    );
}

#[tokio::test]
async fn local_compact_bails_when_window_too_small_for_summary_prompt() {
    ensure_config();
    let context = test_context("local_compact_too_small_window");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut tool_context = context;
    tool_context.events = UpdateSink::for_ui(tx);

    let mock = MockClient::new(vec![(
        vec![make_text_block("unused summary")],
        Some(StopReason::EndTurn),
    )]);
    let mut agent = Agent::new(
        LlmProvider::Mock(mock),
        tool_context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("You are a test agent.".to_string()),
    );
    // Tiny window: output + 10% headroom reservation leaves so little
    // input room that the fixed summarizer prompt cannot fit → compaction
    // must bail instead of silently over-reserving the window. (No
    // thinking budget is involved anymore — the compact call never thinks.)
    agent.agent_settings.model_context_window = 100;
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "first turn"));

    let err = agent
        .compact_history(None)
        .await
        .expect_err("too-small window must bail from compaction");
    assert!(
        err.to_string().contains("too small"),
        "expected a too-small-window error, got: {err}"
    );
}

#[tokio::test]
async fn responses_compact_surfaces_usage_persistence_failure() {
    ensure_config();
    let server = MockServer::start().await;
    let fixture = serde_json::json!({
        "id": "cmp_sanitized_02",
        "object": "response.compaction",
        "created_at": 1754000001,
        "output": [
            {
                "type": "function_call_output",
                "call_id": "call_sanitized_2",
                "output": "sanitized tool output retained by compaction",
                "id": "fc_out_sanitized_2",
                "status": "completed"
            },
            {
                "type": "compaction",
                "id": "cmp_sanitized_02",
                "encrypted_content": "sanitized-encrypted-compaction-content-placeholder"
            }
        ],
        "usage": {
            "input_tokens": 1200,
            "input_tokens_details": { "cached_tokens": 0 },
            "output_tokens": 340,
            "output_tokens_details": { "reasoning_tokens": 0 },
            "total_tokens": 1540
        }
    });
    Mock::given(method("POST"))
        .and(path("/responses/compact"))
        .respond_with(ResponseTemplate::new(200).set_body_json(fixture))
        .expect(1)
        .mount(&server)
        .await;

    let dir = tempfile::tempdir().unwrap();
    let sqlite =
        crate::store::session_store::SqliteSessionStore::new(&dir.path().join("session.db"))
            .await
            .unwrap();
    sqlite
        .create_session("session-1", dir.path().to_str().unwrap(), "")
        .await
        .unwrap();

    // Seed the committed baseline that a successful compaction would
    // replace: two messages plus a compacted provider state.
    let old_messages = vec![
        Message::new_text(Role::User, "old user turn"),
        Message::new_text(Role::Assistant, "old assistant turn"),
    ];
    // Provider state hashes the neutral message representation that the
    // kernel sends. A Message::new_text round-trips as one text block, so
    // hashing the original Text shorthand would make this fixture stale
    // before the usage-failure path is reached.
    let old_wire_messages = old_messages
        .iter()
        .map(tact_llm::neutral::agent_message)
        .map(|message| tact_llm::neutral::llm_message(&message))
        .collect::<Vec<_>>();
    let old_state =
        ProviderConversationState::OpenAiResponses(tact_llm::ResponsesConversationState {
            version: 1,
            provider: "openai_responses".to_string(),
            base_url: server.uri(),
            model: "mock-model".to_string(),
            input_items: vec![serde_json::json!({"type": "message", "id": "old_msg_1"})],
            compaction_id: Some("cmp_old".to_string()),
            is_compacted: true,
            logical_message_count: 2,
            logical_context_hash: tact_llm::context_hash(&old_wire_messages).unwrap(),
        });
    sqlite
        .replace_session_messages_and_provider_state("session-1", &old_messages, Some(&old_state))
        .await
        .unwrap();

    // The native compact endpoint succeeds, but persisting its usage
    // (`responses_compact` token-usage row) fails in the database.
    sqlite.inject_token_usage_insert_failure().await.unwrap();
    let store: crate::store::DynSessionStore = std::sync::Arc::new(sqlite);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut agent = responses_test_agent("responses_compact_usage_failure", &server.uri());
    agent = agent
        .with_ui_channel(tx)
        .with_session("session-1".to_string(), store);
    agent.runtime.context = old_messages.clone();
    agent.runtime.provider_state = Some(old_state.clone());

    let error = agent
        .compact_history(None)
        .await
        .expect_err("usage persistence failure must surface from native compaction");
    assert!(
        format!("{error:#}").contains("failed to persist Responses compact usage"),
        "error must carry the Responses compact usage context, got: {error:#}"
    );

    // Atomic correctness: usage persistence precedes the commit, so a
    // usage failure must leave the old runtime state fully intact.
    assert_eq!(
        agent.runtime.provider_state.as_ref(),
        Some(&old_state),
        "runtime provider state must remain the old committed state"
    );
    assert_eq!(
        serde_json::to_value(&agent.runtime.context).unwrap(),
        serde_json::to_value(&old_messages).unwrap(),
        "runtime context must remain the old committed context"
    );
    assert_eq!(
        agent
            .runtime
            .stats
            .read()
            .expect("session stats lock poisoned")
            .compactions,
        0,
        "no compaction may be recorded when usage persistence failed"
    );
    assert!(
        !agent.runtime.compact_state.has_compacted,
        "runtime must not report compacted when usage persistence failed"
    );

    // No success Info may be emitted for a compaction whose usage row
    // never persisted.
    let mut updates = Vec::new();
    while let Ok(update) = rx.try_recv() {
        updates.push(update);
    }
    assert!(
        updates.iter().all(|u| !matches!(
            u,
            tact_protocol::AgentUpdate::Info(msg) if msg.contains("[responses compacted")
        )),
        "no compaction success Info may be emitted, got: {updates:?}"
    );

    // DB state intact: old messages and old provider state remain, and no
    // `responses_compact` usage row was recorded.
    let pool = sqlx::SqlitePool::connect(&format!(
        "sqlite:{}",
        dir.path().join("session.db").display()
    ))
    .await
    .unwrap();
    let message_count: i64 =
        sqlx::query("SELECT COUNT(*) as cnt FROM messages WHERE session_id = 'session-1'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("cnt")
            .unwrap();
    assert_eq!(
        message_count, 2,
        "old committed messages must stay in the database"
    );
    let state_count: i64 =
        sqlx::query("SELECT COUNT(*) as cnt FROM responses_states WHERE session_id = 'session-1'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("cnt")
            .unwrap();
    assert_eq!(
        state_count, 1,
        "old committed provider state must stay in the database"
    );
    let usage_count: i64 =
        sqlx::query("SELECT COUNT(*) as cnt FROM token_usages WHERE session_id = 'session-1'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .try_get("cnt")
            .unwrap();
    assert_eq!(
        usage_count, 0,
        "no token usage row may be recorded for the failed compact call"
    );
}

#[tokio::test]
async fn responses_compact_history_rejects_malformed_compact_resource() {
    ensure_config();
    let server = MockServer::start().await;
    // No compaction item in the output → protocol error, not retried.
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

    let mut agent = responses_test_agent("responses_compact_malformed", &server.uri());
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "hello"));

    let error = agent
        .compact_history(None)
        .await
        .expect_err("malformed compact resource must fail");
    assert!(
        error.to_string().contains("compaction item"),
        "error should describe the malformed resource, got: {error}"
    );
    assert!(
        agent.runtime.provider_state.is_none(),
        "failed native compaction must leave the old committed state intact"
    );
    server.verify().await;
}

#[test]
fn responses_auto_compact_requires_reported_usage() {
    ensure_config();
    // Explicit local snapshot (window 500_000, max_tokens 8192 → threshold
    // 400_000) so this test never depends on process-global config, which
    // parallel tests may override via `install_or_override`.
    let defaults = crate::config::AgentSettings {
        model: "mock-model".to_string(),
        reasoning_effort: None,
        model_context_window: 500_000,
        max_tokens: 8192,
        thinking_budget: 0,
        snapshot_max_items: 80,
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
    let agent = responses_test_agent("responses_auto_compact", "https://api.openai.com/v1")
        .with_agent_settings(defaults);
    assert!(!agent.auto_compact_due(0), "no usage yet → no auto compact");

    let mut agent = agent;
    // projected = usage + incoming + max_tokens; threshold = 400_000.
    agent.runtime.last_token_total = 390_000;
    assert!(
        !agent.auto_compact_due(0),
        "usage + max_tokens below threshold must not compact"
    );
    agent.runtime.last_token_total = 395_000;
    assert!(
        agent.auto_compact_due(10_000),
        "usage + incoming turn crossing the threshold must compact"
    );
    agent.runtime.last_token_total = 400_000;
    assert!(agent.auto_compact_due(0), "usage at threshold must compact");
}

#[test]
fn non_responses_auto_compact_keeps_context_estimate_trigger() {
    ensure_config();
    let mut agent = chat_completions_test_agent("non_responses_auto_compact");
    let tiny = crate::config::AgentSettings {
        model: "mock-model".to_string(),
        reasoning_effort: None,
        model_context_window: 1_000,
        max_tokens: 100,
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
    agent.agent_settings = tiny;
    // Threshold = 80% of 1000 = 800 tokens; a ~4000-char message
    // estimates above that, so the estimate branch fires even with no
    // provider usage reported.
    agent
        .runtime
        .context
        .push(Message::new_text(Role::User, "x".repeat(4_000)));
    assert!(agent.auto_compact_due(0));
    agent.runtime.context.clear();
    assert!(!agent.auto_compact_due(0));
}
