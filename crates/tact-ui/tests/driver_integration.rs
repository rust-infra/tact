//! Integration tests for the tact-ui command driver (interactive mode without a terminal).

use std::time::Duration;

use anthropic_ai_sdk::types::message::{ContentBlock, StopReason};
use tact::tool::test_support::write_workspace_file;
use tact_ui::driver::run_command_loop;
use tact_ui::test_support::{
    build_test_agent, build_test_agent_with_session, collect_updates_after, install_test_config,
    user_command_channels,
};
use tact_llm::MockClient;
use tact_protocol::{AgentUpdate, UserCommand};

fn text_block(content: &str) -> ContentBlock {
    ContentBlock::Text {
        text: content.to_string(),
    }
}

fn read_file_tool_use(path: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: "tool_read_1".to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({ "path": path }),
    }
}

fn bash_tool_use(command: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: "tool_bash_1".to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({ "command": command }),
    }
}

#[tokio::test]
async fn submit_task_emits_task_complete() {
    install_test_config();

    let mock = MockClient::new(vec![(
        vec![text_block("Hello from mock.")],
        Some(StopReason::EndTurn),
    )]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(UserCommand::SubmitTask("Say hello".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TaskComplete(text) if text.contains("Hello"))),
        "expected TaskComplete with assistant text, got: {updates:?}"
    );
}

#[tokio::test]
async fn submit_task_clears_stale_cancel_flag() {
    install_test_config();

    let mock = MockClient::new(vec![(
        vec![text_block("After cancel clear.")],
        Some(StopReason::EndTurn),
    )]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    agent
        .runtime
        .cancel_flag
        .store(true, std::sync::atomic::Ordering::Relaxed);

    let (user_cmd_tx, user_cmd_rx) = user_command_channels();
    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(UserCommand::SubmitTask("Try again".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TaskComplete(_))),
        "new SubmitTask should clear cancel_flag and complete, got: {updates:?}"
    );
}

#[tokio::test]
async fn submit_task_runs_read_file_tool() {
    install_test_config();

    let mock = MockClient::new(vec![
        (
            vec![read_file_tool_use("hello.txt")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("File read complete.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    write_workspace_file(&work_dir, "hello.txt", "integration file contents");

    let (user_cmd_tx, user_cmd_rx) = user_command_channels();
    let driver = tokio::spawn(run_command_loop(
        agent,
        user_cmd_rx,
        work_dir.clone(),
    ));

    user_cmd_tx
        .send(UserCommand::SubmitTask("Read hello.txt".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished(_, id, result)
                    if id == "tool_read_1" && result.tool == "read_file"
            )
        }),
        "expected read_file StepFinished, got: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TaskComplete(_))),
        "expected TaskComplete after tool turn, got: {updates:?}"
    );
}

#[tokio::test]
async fn cancel_during_task_does_not_emit_task_complete() {
    install_test_config();

    let mock = MockClient::new(vec![
        (
            vec![bash_tool_use("sleep 2")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Should not complete.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(UserCommand::SubmitTask("run sleep".into()))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    user_cmd_tx.send(UserCommand::Cancel).unwrap();
    drop(user_cmd_tx);

    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver should finish after bash sleep")
        .unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("Cancelling"))),
        "expected Cancelling info, got: {updates:?}"
    );
    assert!(
        !updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TaskComplete(_))),
        "cancelled task must not emit TaskComplete, got: {updates:?}"
    );
}

#[tokio::test]
async fn submit_task_persists_messages_to_session_store() {
    install_test_config();

    let mock = MockClient::new(vec![(
        vec![text_block("Persist me.")],
        Some(StopReason::EndTurn),
    )]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir, session_store, session_id) =
        build_test_agent_with_session(mock, Some(agent_tx)).await;
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(UserCommand::SubmitTask("persist".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();
    let _updates = collect_updates_after(agent_rx).await;

    let messages = session_store
        .load_session(&session_id)
        .await
        .expect("load session");
    assert!(
        messages.len() >= 2,
        "expected user + assistant rows in SQLite, got {}",
        messages.len()
    );
}
