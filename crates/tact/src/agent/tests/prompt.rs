use super::*;

fn assert_kernel_prompt_matches_legacy(agent: &mut Agent) -> String {
    let legacy = agent.build_system_prompt().unwrap();
    let assembler = agent.kernel_prompt_assembler();
    let transcript = agent
        .runtime
        .context
        .iter()
        .map(tact_llm::neutral::agent_message)
        .collect::<Vec<_>>();
    let capabilities = tact_agent_api::ModelCapabilities {
        model: agent.agent_settings.model.clone(),
        context_window: agent.model_context_window(),
        max_output_tokens: agent.max_tokens(),
        supports_streaming: true,
        supports_native_compaction: agent.native_compaction_available(),
    };
    let session = tact_agent_api::SessionRef::new("prompt-parity");
    let state = tact_agent_api::ContextView {
        session: &session,
        transcript: &transcript,
        cursor: None,
        total_turns: 0,
        max_total_turns: None,
        last_token_total: Some(agent.runtime.last_token_total),
        pending_inputs: &[],
        capabilities: &capabilities,
        remaining_turns: None,
    };
    let kernel = assembler.build(&state).unwrap();
    assert_eq!(kernel, legacy);
    kernel
}

#[test]
fn kernel_prompt_assembler_matches_legacy_chat_completions_prompt() {
    ensure_config();
    let mut agent = chat_completions_test_agent("kernel_prompt_chat_parity");
    agent.system_prompt = AgentSystemPrompt::Dynamic;
    agent.agent_settings.model_context_window = 200_000;
    agent.runtime.last_token_total = 130_000;

    let prompt = assert_kernel_prompt_matches_legacy(&mut agent);
    assert!(prompt.contains("spawn_subagent"), "{prompt}");
    assert!(
        !prompt.contains("Skill loading policy for this Responses request."),
        "{prompt}"
    );
}

#[test]
fn kernel_prompt_assembler_keeps_responses_template_without_native_compaction() {
    ensure_config();
    let agent = responses_test_agent(
        "kernel_prompt_responses_parity",
        "https://example.invalid/v1",
    )
    .with_provider_kind(ProviderKind::DeepSeek);
    let mut agent = agent;
    agent.system_prompt = AgentSystemPrompt::Dynamic;

    let prompt = assert_kernel_prompt_matches_legacy(&mut agent);
    assert!(
        prompt.contains("Skill loading policy for this Responses request."),
        "{prompt}"
    );
    assert!(!agent.native_compaction_available());
}

#[test]
fn assemble_agents_md_prompt_reads_workdir_file() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("AGENTS.md"),
        "# Crate map\n\n- `crates/tact` — runtime\n",
    )
    .unwrap();

    let rendered =
        assemble_agents_md_prompt(dir.path(), &crate::config::InstructionSources::default());
    assert!(rendered.starts_with("## AGENTS.md instructions"));
    assert!(rendered.contains("### From project root (AGENTS.md)"));
    assert!(rendered.contains("Crate map"));
    assert!(rendered.contains("crates/tact"));
}

#[test]
fn assemble_agents_md_prompt_empty_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        assemble_agents_md_prompt(dir.path(), &crate::config::InstructionSources::default())
            .is_empty()
    );
}

#[test]
fn assemble_agents_md_prompt_skipped_when_disabled() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("AGENTS.md"), "# Rules\n").unwrap();
    let sources = crate::config::InstructionSources { agents_md: false };
    assert!(assemble_agents_md_prompt(dir.path(), &sources).is_empty());
}

#[test]
fn cached_md_section_computes_once() {
    let mut cache = None;
    let mut calls = 0usize;
    let first = cached_md_section(&mut cache, || {
        calls += 1;
        "hello".to_string()
    });
    let second = cached_md_section(&mut cache, || {
        calls += 1;
        "should-not-run".to_string()
    });
    assert_eq!(first, "hello");
    assert_eq!(second, "hello");
    assert_eq!(calls, 1);
    assert_eq!(cache.as_deref(), Some("hello"));
}

#[test]
fn cached_md_section_caches_empty_string() {
    let mut cache = None;
    let mut calls = 0usize;
    let _ = cached_md_section(&mut cache, || {
        calls += 1;
        String::new()
    });
    let _ = cached_md_section(&mut cache, || {
        calls += 1;
        "later".to_string()
    });
    assert_eq!(calls, 1);
    assert_eq!(cache.as_deref(), Some(""));
}

// ── ensure_max_tokens_gt_thinking_budget ──

#[test]
fn zero_budget_is_noop() {
    let mut mt = 8_000u32;
    assert!(Agent::ensure_max_tokens_gt_thinking_budget(&mut mt, 0).is_none());
    assert_eq!(mt, 8_000);
}

#[test]
fn max_tokens_already_larger_is_noop() {
    let mut mt = 16_000u32;
    assert!(Agent::ensure_max_tokens_gt_thinking_budget(&mut mt, 8_000).is_none());
    assert_eq!(mt, 16_000);
}

#[test]
fn expands_when_budget_exceeds_max_tokens() {
    let mut mt = 8_000u32;
    let msg = Agent::ensure_max_tokens_gt_thinking_budget(&mut mt, 32_000).unwrap();
    assert_eq!(mt, 32_001);
    assert!(msg.contains("32000"));
    assert!(msg.contains("32001"));
}

#[test]
fn set_thinking_budget_auto_expands_max_tokens() {
    ensure_config();
    let context = test_context("set_thinking_auto_test");
    let router = crate::tool::toolset();
    let mcp = crate::mcp::MCPToolRouter::new();
    let perm =
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

    let mut agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        router,
        mcp,
        perm,
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    assert_eq!(agent.thinking_budget(), 0);
    agent.set_thinking_budget(64_000);
    assert!(agent.max_tokens() > 64_000);
    assert_eq!(agent.thinking_budget(), 64_000);

    let mut saw_model_info = false;
    while let Ok(update) = rx.try_recv() {
        match update {
            AgentUpdate::ModelInfo(params) => {
                assert_eq!(params.thinking_budget, Some(64_000));
                assert!(params.max_tokens > 64_000);
                saw_model_info = true;
            }
            AgentUpdate::Info(_) => {}
            other => panic!("unexpected update: {other:?}"),
        }
    }
    assert!(saw_model_info, "set_thinking_budget must emit ModelInfo");
}
