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
        context_window: (agent.model_context_window() != 0).then_some(agent.model_context_window()),
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
fn preview_system_prompt_assembles_project_context_on_demand() {
    ensure_config();
    let mut agent = chat_completions_test_agent("preview_system_prompt");
    agent.system_prompt = AgentSystemPrompt::Dynamic;

    let source = agent.preview_system_prompt(false).unwrap();
    let assembled = agent.preview_system_prompt(true).unwrap();

    assert!(!source.contains("Skill loading policy for this Responses request."));
    assert!(
        assembled.len() >= source.len(),
        "{assembled}\n---\n{source}"
    );
    assert!(assembled.contains("Current date:"), "{assembled}");
    assert!(!source.contains("Current date:"), "{source}");
    assert_ne!(
        assembled,
        "Dynamic Tact system prompt. Enable the assembled view to render project context."
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

/// The snapshot is a budget, not an inventory: payload directories and
/// redundant scaffold lines must not push real modules off the end.
#[test]
fn snapshot_dir_drops_payload_and_folds_scaffold() {
    let tmp = tempfile::tempdir().unwrap();
    // `walkdir` does not descend into a symlinked root, and macOS hands out
    // temp dirs under `/var/folders`, which is a symlink.
    let root = &std::fs::canonicalize(tmp.path()).unwrap();

    // A crate whose `src/` has listed children: the `src` line is redundant.
    for dir in [
        "crates/app/src/tool",
        "crates/app/src/store",
        "crates/app/assets/fonts",
        "crates/app/assets/icons/deep",
    ] {
        std::fs::create_dir_all(root.join(dir)).unwrap();
    }
    // A crate whose `src/` holds only files: the `src` line is the only hint
    // that a source tree exists, so it must survive.
    std::fs::create_dir_all(root.join("crates/flat/src")).unwrap();
    std::fs::write(root.join("crates/flat/src/lib.rs"), "").unwrap();

    let snapshot = snapshot_dir(root, 200).expect("snapshot");
    let lines = snapshot.lines().collect::<Vec<_>>();

    assert!(
        !snapshot.contains("assets"),
        "payload dirs must not be listed:\n{snapshot}"
    );
    // Exactly one `src/` line survives: the flat crate's, where it is the only
    // hint that a source tree exists. `crates/app/src` also names `src` but as
    // the header of its listed children, not as a redundant child of its own.
    let src_lines = lines.iter().filter(|l| l.trim() == "src/").count();
    assert_eq!(
        src_lines, 1,
        "only the flat crate needs a `src/` line:\n{snapshot}"
    );
    assert!(
        lines.contains(&"crates/app/src") && lines.contains(&"crates/flat"),
        "both crates must still be visible:\n{snapshot}"
    );
    assert!(
        snapshot.contains("tool/") && snapshot.contains("store/"),
        "real modules must survive the filter:\n{snapshot}"
    );
}

/// The truncation budget still applies, and says so.
#[test]
fn snapshot_dir_reports_truncation() {
    let tmp = tempfile::tempdir().unwrap();
    // As above: a symlinked root is not descended into.
    let root = &std::fs::canonicalize(tmp.path()).unwrap();
    for i in 0..12 {
        std::fs::create_dir_all(root.join(format!("crates/c{i}/src/mod{i}"))).unwrap();
    }

    let capped = snapshot_dir(root, 4).expect("snapshot");
    assert!(
        capped.contains("(truncated at 4 items)"),
        "a capped snapshot must say it was capped:\n{capped}"
    );

    let full = snapshot_dir(root, 200).expect("snapshot");
    assert!(
        !full.contains("(truncated at"),
        "an uncapped snapshot must not claim truncation:\n{full}"
    );
}

/// Print the real repository snapshot at both caps so the before/after effect
/// is inspectable rather than argued about. Ignored: a measurement, not a gate.
#[test]
#[ignore = "printing aid: shows the real snapshot for this checkout"]
fn snapshot_dir_prints_this_repository() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let at80 = snapshot_dir(root, 80).expect("snapshot");
    let full = snapshot_dir(root, 120).expect("snapshot");
    println!(
        "=== cap 80: lines={} chars={}",
        at80.lines().count(),
        at80.chars().count()
    );
    println!(
        "=== cap 120: lines={} chars={}",
        full.lines().count(),
        full.chars().count()
    );
    let lost = full
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter(|l| !at80.lines().any(|k| k == *l))
        .collect::<Vec<_>>();
    println!("=== dropped at cap 80 ===");
    for l in lost {
        println!("{l}");
    }
}
