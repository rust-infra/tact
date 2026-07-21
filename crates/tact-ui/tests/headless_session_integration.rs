//! Full-stack headless sessions: driver + live App update loop + render snapshots.

mod harness;

use std::time::Duration;

use harness::{bash_tool_use, edit_file_tool_use, mock_turn, read_file_tool_use, text_block, write_file_tool_use};
use tact::{permission::PermissionMode, tool::test_support::write_workspace_file};
use tact_llm::{MockClient, StopReason};
use tact_protocol::UserCommand;
use tact_ui::headless_session::run_headless_session;

#[tokio::test]
async fn headless_session_simple_task_reaches_done() {
    let mock = MockClient::new(vec![(vec![text_block("Headless done.")], Some(StopReason::EndTurn))]);

    let result = run_headless_session(
        mock,
        PermissionMode::Auto,
        None,
        |_| {},
        |tx| {
            tokio::spawn(async move {
                tx.send(UserCommand::SubmitTask("hello".into())).unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(result.is_done, "simple task should reach Done in headless App");
    let final_text = result.snapshots.final_render.unwrap_or_default();
    assert!(
        final_text.contains("Task completed") || final_text.contains("Done"),
        "final frame should show completion:\n{final_text}"
    );
}

#[tokio::test]
async fn headless_session_read_file_shows_executing_snapshot() {
    let mock = MockClient::new(vec![
        (vec![read_file_tool_use("r1", "data.txt")], Some(StopReason::ToolUse)),
        (vec![text_block("Read ok.")], Some(StopReason::EndTurn)),
    ]);

    let result = run_headless_session(
        mock,
        PermissionMode::Auto,
        None,
        |dir| {
            write_workspace_file(dir, "data.txt", "headless data");
        },
        |tx| {
            tokio::spawn(async move {
                tx.send(UserCommand::SubmitTask("read data".into())).unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(result.is_done);
    let final_text = result.snapshots.final_render.unwrap_or_default();
    assert!(
        final_text.contains("read_file") || final_text.contains("data.txt"),
        "final render should include read_file tool card:\n{final_text}"
    );
}

#[tokio::test]
async fn headless_session_default_permission_shows_select_popup() {
    let mock = MockClient::new(vec![
        mock_turn(vec![edit_file_tool_use("e1", "main.rs", "old", "new")], StopReason::ToolUse),
        mock_turn(vec![text_block("Edited.")], StopReason::EndTurn),
    ]);

    let result = run_headless_session(
        mock,
        PermissionMode::Default,
        Some(0),
        |dir| write_workspace_file(dir, "main.rs", "old code"),
        |tx| {
            tokio::spawn(async move {
                tx.send(UserCommand::SubmitTask("edit main".into())).unwrap();
                drop(tx);
            })
        },
    )
    .await;

    let select_text = result.snapshots.select.unwrap_or_else(|| {
        panic!("expected select popup snapshot, final:\n{}", result.snapshots.final_render.unwrap_or_default())
    });
    assert!(
        select_text.contains("SELECT") || select_text.contains("Allow"),
        "permission prompt should render select overlay:\n{select_text}"
    );

    let content = std::fs::read_to_string(result.work_dir.join("main.rs")).unwrap();
    assert!(content.contains("new"), "allow-once should apply edit");
    assert!(result.is_done);
}

#[tokio::test]
async fn headless_session_cancel_mid_task_no_done() {
    let mock = MockClient::new(vec![
        (vec![bash_tool_use("b1", "sleep 2")], Some(StopReason::ToolUse)),
        (vec![text_block("Should not finish.")], Some(StopReason::EndTurn)),
    ]);

    let result = run_headless_session(
        mock,
        PermissionMode::Auto,
        None,
        |_| {},
        |tx| {
            tokio::spawn(async move {
                tx.send(UserCommand::SubmitTask("sleep".into())).unwrap();
                tokio::time::sleep(Duration::from_millis(120)).await;
                tx.send(UserCommand::Cancel).unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(!result.is_done, "cancelled task should not reach Done in headless App");
    let final_text = result.snapshots.final_render.unwrap_or_default();
    assert!(
        final_text.contains("Cancelling") || final_text.contains("bash"),
        "cancel path should be visible in render:\n{final_text}"
    );
}

#[tokio::test]
async fn headless_session_sequential_tasks_both_complete() {
    let mock = MockClient::new(vec![
        (vec![text_block("First.")], Some(StopReason::EndTurn)),
        (vec![text_block("Second.")], Some(StopReason::EndTurn)),
    ]);

    let result = run_headless_session(
        mock,
        PermissionMode::Auto,
        None,
        |_| {},
        |tx| {
            tokio::spawn(async move {
                tx.send(UserCommand::SubmitTask("first".into())).unwrap();
                tx.send(UserCommand::SubmitTask("second".into())).unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(result.is_done);
}

#[tokio::test]
async fn headless_session_write_read_chain_renders_tools() {
    let mock = MockClient::new(vec![
        (vec![write_file_tool_use("w1", "chain.rs", "fn chain() {}")], Some(StopReason::ToolUse)),
        (vec![read_file_tool_use("r1", "chain.rs")], Some(StopReason::ToolUse)),
        (vec![text_block("Chain done.")], Some(StopReason::EndTurn)),
    ]);

    let result = run_headless_session(
        mock,
        PermissionMode::Auto,
        None,
        |_| {},
        |tx| {
            tokio::spawn(async move {
                tx.send(UserCommand::SubmitTask("write then read".into())).unwrap();
                drop(tx);
            })
        },
    )
    .await;

    assert!(result.is_done);
    let final_text = result.snapshots.final_render.unwrap_or_default();
    assert!(
        final_text.contains("chain.rs") || final_text.contains("Write") || final_text.contains("Read"),
        "write→read chain should render tool cards:\n{final_text}"
    );
    assert!(result.work_dir.join("chain.rs").exists());
}
