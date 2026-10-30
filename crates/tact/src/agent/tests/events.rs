use super::*;

#[test]
fn agent_new_preserves_work_dir_in_tool_context() {
    let context = test_context("agent_work_dir");
    let router = crate::tool::toolset();
    let mcp = crate::mcp::MCPToolRouter::new();
    let perm =
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap();

    let expected = context.work_dir.clone();

    let agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        router,
        mcp,
        perm,
        AgentSystemPrompt::Static("test".to_string()),
    );

    assert_eq!(agent.tool_context.work_dir, expected);
}

#[test]
fn with_ui_channel_syncs_tool_context_events() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let context = test_context("ui_tx_sync");
    let agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_ui_channel(tx);

    assert!(agent.runtime.events.has_ui());
    assert!(agent.tool_context.events.has_ui());
}

#[test]
fn an_observer_receives_events_and_cannot_answer_prompts() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let context = test_context("observer_events");
    let agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_event_observer(tx);

    agent.emit_update(AgentUpdate::Info("progress".into()));

    assert!(
        matches!(rx.try_recv(), Ok(AgentUpdate::Info(text)) if text == "progress"),
        "the observer sees every event"
    );
    assert!(
        !agent.runtime.events.has_ui(),
        "an observer is not an answering frontend"
    );
    assert!(
        agent.tool_context.events.ui().is_none(),
        "tool prompts must not treat an observer as an answerer"
    );
}

#[test]
fn detaching_observers_closes_their_channel() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let context = test_context("detach_observers");
    let mut agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_event_observer(tx);

    agent.detach_event_observers();

    assert!(
        rx.try_recv().is_err(),
        "the channel is closed, so a pump awaiting it can return"
    );
}

/// A clone handed to a detached background task keeps the observer channel
/// open, so detaching alone does not end the pump.
///
/// This is the headless hang: `background_run` gives the spawned task a
/// `BackgroundProgressSink` built from the invocation's reporter, and that
/// reporter holds a clone of the observer sender. The task outlives the
/// tool call by design ("there is no time limit"), so the sender outlives
/// `detach_event_observers` too, and a pump that waits for the channel to
/// close waits for the command.
#[test]
fn a_cloned_reporter_keeps_the_observer_channel_open_after_detach() {
    ensure_config();
    use crate::tool::test_support::test_context;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let context = test_context("detach_with_cloned_reporter");
    let mut agent = Agent::new(
        LlmProvider::Mock(MockClient::new(vec![])),
        context,
        crate::tool::toolset(),
        crate::mcp::MCPToolRouter::new(),
        crate::permission::PermissionManager::try_new(crate::permission::PermissionMode::Default)
            .unwrap(),
        AgentSystemPrompt::Static("test".to_string()),
    )
    .with_event_observer(tx);

    // What `background_run` does: a per-invocation clone, moved into a task
    // that outlives the call.
    let detached = agent.tool_context.progress_reporter.clone();

    agent.detach_event_observers();

    assert!(
        rx.try_recv().is_err(),
        "the channel is still open: the cloned reporter holds a sender"
    );

    // The channel closes only once that clone is dropped — i.e. when the
    // background command ends, which may be never.
    drop(detached);
    assert!(rx.try_recv().is_err(), "now it is closed");
}
