use super::*;

fn hook_test_agent(context_name: &str) -> Agent {
    ensure_config();
    let context = test_context(context_name);
    Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("hook test".into()),
    )
}

#[tokio::test]
async fn stop_hook_block_requests_continuation() {
    let agent = hook_test_agent("stop_hook_block")
        .with_stop(|_agent| Box::pin(async move { Ok(HookControl::Block("keep going".into())) }));
    let mut agent = agent;
    let control = agent.dispatch_stop_hooks().await.unwrap();
    assert_eq!(control, HookControl::Block("keep going".into()));
}

#[tokio::test]
async fn stop_hook_continue_defaults_to_stop() {
    let mut agent = hook_test_agent("stop_hook_continue");
    let control = agent.dispatch_stop_hooks().await.unwrap();
    assert_eq!(control, HookControl::Continue);
}

#[tokio::test]
async fn session_end_hook_is_observational() {
    let mut agent = hook_test_agent("session_end_obs").with_session_end(|_agent| {
        Box::pin(async move { Ok(HookControl::Block("ignored".into())) })
    });
    // A block must not fail the dispatch — the session is ending.
    agent.dispatch_session_end_hooks().await.unwrap();
}

#[tokio::test]
async fn task_completed_hook_is_observational() {
    let mut agent = hook_test_agent("task_completed_obs").with_task_completed(|_agent| {
        Box::pin(async move { Ok(HookControl::Block("ignored".into())) })
    });
    // A block must not fail the dispatch — the task already completed.
    agent.dispatch_task_completed_hooks().await.unwrap();
}

#[test]
fn new_hook_builders_register_variants() {
    let agent = hook_test_agent("new_hook_builders")
        .with_post_tool_failure(|_agent, _tool_use, _error| {
            Box::pin(async { Ok(HookControl::Continue) })
        })
        .with_notification(|_agent, _ctx| Box::pin(async { Ok(HookControl::Continue) }))
        .with_task_completed(|_agent| Box::pin(async { Ok(HookControl::Continue) }));
    assert_eq!(agent.hooks_by_type(HookTypes::PostToolUseFailure).len(), 1);
    assert_eq!(agent.hooks_by_type(HookTypes::Notification).len(), 1);
    assert_eq!(agent.hooks_by_type(HookTypes::TaskCompleted).len(), 1);
}

#[tokio::test]
async fn pre_compact_block_vetoes_compaction() {
    let mut agent = hook_test_agent("pre_compact_veto").with_pre_compact(|_agent, trigger| {
        assert_eq!(trigger, CompactTrigger::Auto);
        Box::pin(async move { Ok(HookControl::Block("no auto compact".into())) })
    });
    // PreCompact veto returns Ok without compacting (has_compacted stays false).
    agent
        .compact_history_with_trigger(CompactTrigger::Auto, None)
        .await
        .unwrap();
    assert!(!agent.runtime.compact_state.has_compacted);
}
