//! Integration tests for the tact-ui command driver (interactive mode without a terminal).

use std::time::Duration;

use tact::tool::test_support::write_workspace_file;
use tact_llm::{ContentBlock, MockClient, StopReason};
use tact_protocol::{AgentUpdate, DriverMessage, StepStatus};
use tact_ui::{
    driver::run_command_loop,
    test_support::{
        build_test_agent, build_test_agent_with_session, collect_updates_after,
        install_test_config, user_command_channels,
    },
};

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

fn write_file_tool_use(path: &str, content: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: "tool_write_1".to_string(),
        name: "write_file".to_string(),
        input: serde_json::json!({ "path": path, "content": content }),
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
        .send(DriverMessage::SubmitTask("Say hello".into()))
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
        .send(DriverMessage::SubmitTask("Try again".into()))
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
    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir.clone()));

    user_cmd_tx
        .send(DriverMessage::SubmitTask("Read hello.txt".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { tool_id: id, result, .. }
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
        (vec![bash_tool_use("sleep 30")], Some(StopReason::ToolUse)),
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
        .send(DriverMessage::SubmitTask("run sleep".into()))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    user_cmd_tx.send(DriverMessage::Cancel).unwrap();
    drop(user_cmd_tx);

    tokio::time::timeout(Duration::from_secs(2), driver)
        .await
        .expect("driver should cancel bash promptly")
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
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TaskCancelled)),
        "cancelled task must emit TaskCancelled so TUI leaves busy state, got: {updates:?}"
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
        .send(DriverMessage::SubmitTask("persist".into()))
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

#[tokio::test]
async fn sequential_submit_tasks_both_complete() {
    install_test_config();

    let mock = MockClient::new(vec![
        (
            vec![text_block("First task done.")],
            Some(StopReason::EndTurn),
        ),
        (
            vec![text_block("Second task done.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(DriverMessage::SubmitTask("first".into()))
        .unwrap();
    user_cmd_tx
        .send(DriverMessage::SubmitTask("second".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    let completes: Vec<_> = updates
        .iter()
        .filter_map(|u| match u {
            AgentUpdate::TaskComplete(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        completes.len(),
        2,
        "expected two TaskComplete, got: {updates:?}"
    );
    assert!(completes[0].contains("First"));
    assert!(completes[1].contains("Second"));
}

#[tokio::test]
async fn submit_task_runs_write_file_tool() {
    install_test_config();

    let mock = MockClient::new(vec![
        (
            vec![write_file_tool_use("out.txt", "written by test")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Write complete.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir.clone()));

    user_cmd_tx
        .send(DriverMessage::SubmitTask("write out.txt".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { tool_id: id, result, .. }
                    if id == "tool_write_1"
                        && result.tool == "write_file"
                        && matches!(result.status, StepStatus::Success)
            )
        }),
        "expected write_file StepFinished, got: {updates:?}"
    );

    let written = std::fs::read_to_string(work_dir.join("out.txt")).expect("read out.txt");
    assert_eq!(written, "written by test");
}

#[tokio::test]
async fn read_missing_file_emits_failed_step() {
    install_test_config();

    let mock = MockClient::new(vec![
        (
            vec![read_file_tool_use("missing.txt")],
            Some(StopReason::ToolUse),
        ),
        (
            vec![text_block("Handled missing file.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(DriverMessage::SubmitTask("read missing".into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { tool_id: id, result, .. }
                    if id == "tool_read_1"
                        && matches!(result.status, StepStatus::Failed)
            )
        }),
        "missing file should produce failed StepFinished, got: {updates:?}"
    );
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::TaskComplete(_))),
        "agent should continue after tool failure, got: {updates:?}"
    );
}

#[tokio::test]
async fn cancel_emits_cancelled_by_user_info() {
    install_test_config();

    let mock = MockClient::new(vec![
        (vec![bash_tool_use("sleep 3")], Some(StopReason::ToolUse)),
        (
            vec![text_block("Should not finish.")],
            Some(StopReason::EndTurn),
        ),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    user_cmd_tx
        .send(DriverMessage::SubmitTask("slow".into()))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(100)).await;
    user_cmd_tx.send(DriverMessage::Cancel).unwrap();
    drop(user_cmd_tx);

    tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver should finish")
        .unwrap();

    let updates = collect_updates_after(agent_rx).await;
    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("Cancelled by user"))),
        "cancelled agent_loop should emit info, got: {updates:?}"
    );
}

/// A crashed Goal runner leaves an active run behind an expired lease and a Goal
/// still marked `Running`. Starting the command loop must reconcile that state
/// once, report it, and keep serving commands.
#[tokio::test]
async fn command_loop_recovers_an_interrupted_goal_run() {
    install_test_config();

    let mock = MockClient::new(vec![]);
    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
    let manager = agent.tool_context.goal_manager.clone();

    let goal = manager
        .create(tact::goal::GoalCreate {
            title: "interrupted goal".to_string(),
            objective: "survive a crash".to_string(),
            criteria: vec![tact::goal::GoalCriterion::required_with_id(
                "c1",
                "the crash is recovered",
            )],
        })
        .await
        .unwrap();
    let goal = manager
        .update(
            &goal.id,
            goal.version,
            tact::goal::GoalUpdate {
                status: Some(tact::goal::GoalStatus::Queued),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();
    let goal = manager
        .update(
            &goal.id,
            goal.version,
            tact::goal::GoalUpdate {
                status: Some(tact::goal::GoalStatus::Running),
                ..Default::default()
            },
            None,
            None,
        )
        .await
        .unwrap();

    // The crashed process: an active run whose lease expires unreleased.
    let ttl = Duration::from_millis(20);
    let (_token, run) = manager
        .start_run(
            "crashed-runner",
            ttl,
            tact::goal::GoalRun::new(goal.id.clone()),
        )
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;

    let (user_cmd_tx, user_cmd_rx) = user_command_channels();
    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir));

    let updates = collect_updates_after(agent_rx).await;
    drop(user_cmd_tx);
    let agent = tokio::time::timeout(Duration::from_secs(5), driver)
        .await
        .expect("driver should finish")
        .unwrap();

    assert!(
        updates.iter().any(|update| matches!(
            update,
            AgentUpdate::Info(message)
                if message.contains("recovered interrupted run") && message.contains(&run.id)
        )),
        "the loop reports the recovered run, got: {updates:?}"
    );
    let manager = agent.tool_context.goal_manager.clone();
    assert!(
        manager.list_running_runs().await.unwrap().is_empty(),
        "recovery interrupts the expired run"
    );
    assert_eq!(
        manager.get(&goal.id).await.unwrap().status,
        tact::goal::GoalStatus::Paused,
        "an interrupted Goal is paused, never successful"
    );
}
