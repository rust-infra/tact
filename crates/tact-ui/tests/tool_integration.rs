//! Tool execution scenarios via the tact-ui driver harness.

mod harness;

use std::time::Duration;

use harness::{
    bash_tool_use, first_index, mock_turn, mock_turn_with_usage, read_file_tool_use, run_commands,
    run_single_task, run_single_task_with_setup, sample_token_usage, step_finished_ids, text_block,
    write_file_tool_use,
};
use tact::{permission::PermissionMode, tool::test_support::write_workspace_file};
use tact_llm::{MockClient, StopReason};
use tact_protocol::{AgentUpdate, StepStatus, UserCommand};

#[tokio::test]
async fn parallel_read_files_both_succeed() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![
                read_file_tool_use("read_a", "a.txt"),
                read_file_tool_use("read_b", "b.txt"),
            ],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Both files read.")], StopReason::EndTurn),
    ]);

    let (updates, _work_dir) =
        run_single_task_with_setup(mock, "read both", PermissionMode::Auto, |work_dir| {
            write_workspace_file(work_dir, "a.txt", "content-a");
            write_workspace_file(work_dir, "b.txt", "content-b");
        })
        .await;

    let ids = step_finished_ids(&updates);
    assert_eq!(ids.len(), 2, "expected two StepFinished, got: {updates:?}");
    assert!(ids.contains(&"read_a".to_string()));
    assert!(ids.contains(&"read_b".to_string()));
    assert!(
        updates.iter().all(|u| {
            if let AgentUpdate::StepFinished { result, .. } = u {
                matches!(result.status, StepStatus::Success)
            } else {
                true
            }
        }),
        "all tool steps should succeed, got: {updates:?}"
    );
}

#[tokio::test]
async fn large_non_bash_output_is_persisted() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![read_file_tool_use("read_big", "big.txt")],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Large read handled.")], StopReason::EndTurn),
    ]);
    let (updates, work_dir) =
        run_single_task_with_setup(mock, "read big", PermissionMode::Auto, |dir| {
            write_workspace_file(dir, "big.txt", &"x".repeat(30_001));
        })
        .await;

    assert!(updates.iter().any(|update| matches!(
        update,
        AgentUpdate::StepFinished { tool_id, result, .. }
            if tool_id == "read_big"
                && matches!(result.status, StepStatus::Success)
                && result.message.contains("<persisted-output>")
    )));
    assert!(work_dir.join(".tact/tool-results/read_big.txt").exists());
}

#[tokio::test]
async fn plan_mode_blocks_write_file() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![write_file_tool_use("w1", "blocked.txt", "nope")],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Write blocked.")], StopReason::EndTurn),
    ]);

    let (updates, work_dir) = run_single_task(mock, "write file", PermissionMode::Plan).await;

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFailed { tool_id: id, error: msg, .. }
                    if id == "w1" && msg.contains("Plan mode")
            )
        }),
        "Plan mode should deny write_file, got: {updates:?}"
    );
    assert!(
        !work_dir.join("blocked.txt").exists(),
        "denied write must not create file"
    );
}

#[tokio::test]
async fn bash_echo_returns_success() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![bash_tool_use("bash1", "echo harness-ok")],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Echo done.")], StopReason::EndTurn),
    ]);

    let (updates, _work_dir) = run_single_task(mock, "echo test", PermissionMode::Auto).await;

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { tool_id: id, result, .. }
                    if id == "bash1"
                        && result.tool == "bash"
                        && matches!(result.status, StepStatus::Success)
            )
        }),
        "expected successful bash StepFinished, got: {updates:?}"
    );
}

#[tokio::test]
async fn bash_streams_progress_before_step_finished() {
    let command =
        "printf 'out-1\\n'; sleep 0.1; printf 'err-1\\n' >&2; sleep 0.1; printf 'out-2\\n'";
    let mock = MockClient::new(vec![
        mock_turn(
            vec![bash_tool_use("bash_stream", command)],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Streaming done.")], StopReason::EndTurn),
    ]);

    let (updates, _) = run_single_task(mock, "stream", PermissionMode::Auto).await;
    let progress_idx = updates
        .iter()
        .position(|update| {
            matches!(
                update,
                AgentUpdate::ToolProgress { tool_id, .. } if tool_id == "bash_stream"
            )
        })
        .expect("expected bash progress");
    let finish_idx = first_index(&updates, |update| {
        matches!(
            update,
            AgentUpdate::StepFinished { tool_id, .. } if tool_id == "bash_stream"
        )
    })
    .expect("expected bash finish");
    let progress_text = updates
        .iter()
        .filter_map(|update| match update {
            AgentUpdate::ToolProgress { tool_id, chunks } if tool_id == "bash_stream" => Some(
                chunks
                    .iter()
                    .map(|chunk| chunk.text.as_str())
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect::<String>();

    assert!(progress_idx < finish_idx);
    assert!(progress_text.contains("out-1"));
    assert!(progress_text.contains("err-1"));
    assert!(progress_text.contains("out-2"));
}

#[tokio::test]
async fn read_then_write_chain() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![read_file_tool_use("read1", "source.txt")],
            StopReason::ToolUse,
        ),
        mock_turn(
            vec![write_file_tool_use("write1", "dest.txt", "copied")],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Chain done.")], StopReason::EndTurn),
    ]);

    let (updates, work_dir) =
        run_single_task_with_setup(mock, "copy file", PermissionMode::Auto, |dir| {
            write_workspace_file(dir, "source.txt", "original")
        })
        .await;

    let ids = step_finished_ids(&updates);
    assert!(
        ids.contains(&"read1".to_string()) && ids.contains(&"write1".to_string()),
        "expected read then write steps, got: {updates:?}"
    );
    assert_eq!(
        std::fs::read_to_string(work_dir.join("dest.txt")).unwrap(),
        "copied"
    );
}

#[tokio::test]
async fn cancel_then_submit_completes_fresh_task() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![bash_tool_use("bash_sleep", "sleep 2")],
            StopReason::ToolUse,
        ),
        mock_turn(
            vec![text_block("Recovered after cancel.")],
            StopReason::EndTurn,
        ),
    ]);

    let (updates, _work_dir) = run_commands(mock, PermissionMode::Auto, |user_cmd_tx| {
        tokio::spawn(async move {
            user_cmd_tx
                .send(UserCommand::SubmitTask("slow task".into()))
                .unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            user_cmd_tx.send(UserCommand::Cancel).unwrap();
            user_cmd_tx
                .send(UserCommand::SubmitTask("fast recovery".into()))
                .unwrap();
            drop(user_cmd_tx);
        })
    })
    .await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("Cancelling"))),
        "expected cancel info, got: {updates:?}"
    );
    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::TaskComplete(text) if text.contains("Recovered")
            )
        }),
        "second task should complete after cancel, got: {updates:?}"
    );
    let completes = updates
        .iter()
        .filter(|u| matches!(u, AgentUpdate::TaskComplete(_)))
        .count();
    assert_eq!(completes, 1, "only the recovery task should TaskComplete");
}

#[tokio::test]
async fn submit_task_emits_model_info() {
    let mock = MockClient::new(vec![mock_turn(
        vec![text_block("With model info.")],
        StopReason::EndTurn,
    )]);

    let (updates, _work_dir) = run_single_task(mock, "hello", PermissionMode::Auto).await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::ModelInfo(_))),
        "agent_loop should emit ModelInfo before LLM call, got: {updates:?}"
    );
}

#[tokio::test]
async fn tool_turn_persists_to_session_store() {
    use tact_ui::test_support::build_test_agent_with_session;

    let mock = MockClient::new(vec![
        mock_turn(
            vec![read_file_tool_use("read1", "data.txt")],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Done.")], StopReason::EndTurn),
    ]);

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (agent, work_dir, session_store, session_id) =
        build_test_agent_with_session(mock, Some(agent_tx)).await;
    write_workspace_file(&work_dir, "data.txt", "session data");

    let (user_cmd_tx, user_cmd_rx) = tact_ui::test_support::user_command_channels();
    let driver = tokio::spawn(tact_ui::driver::run_command_loop(
        agent,
        user_cmd_rx,
        work_dir,
    ));
    user_cmd_tx
        .send(UserCommand::SubmitTask("read and persist".into()))
        .unwrap();
    drop(user_cmd_tx);
    driver.await.unwrap();
    let _updates = tact_ui::test_support::collect_updates_after(agent_rx).await;

    let messages = session_store
        .load_session(&session_id)
        .await
        .expect("load session");
    assert!(
        messages.len() >= 2,
        "tool turn should persist user + assistant messages, got {}",
        messages.len()
    );
}

#[tokio::test]
async fn read_write_same_file_serializes() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![
                read_file_tool_use("read_shared", "shared.txt"),
                write_file_tool_use("write_shared", "shared.txt", "updated"),
            ],
            StopReason::ToolUse,
        ),
        mock_turn(vec![text_block("Done.")], StopReason::EndTurn),
    ]);

    let (updates, work_dir) = run_single_task_with_setup(
        mock,
        "read then write shared.txt",
        PermissionMode::Auto,
        |dir| write_workspace_file(dir, "shared.txt", "original"),
    )
    .await;

    let read_done = first_index(
        &updates,
        |u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "read_shared"),
    );
    let write_done = first_index(
        &updates,
        |u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "write_shared"),
    );
    assert!(
        read_done.is_some() && write_done.is_some() && read_done < write_done,
        "read must finish before write on same path, got: {updates:?}"
    );
    assert_eq!(
        std::fs::read_to_string(work_dir.join("shared.txt")).unwrap(),
        "updated"
    );
}

#[tokio::test]
async fn submit_task_emits_token_usage() {
    let usage = sample_token_usage();
    let mock = MockClient::with_usage(vec![mock_turn_with_usage(
        vec![text_block("With usage.")],
        StopReason::EndTurn,
        usage.clone(),
    )]);

    let (updates, _work_dir) = run_single_task(mock, "hello", PermissionMode::Auto).await;

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::TokenUsage(info)
                    if info.prompt == usage.prompt
                        && info.completion == usage.completion
                        && info.total == usage.total
            )
        }),
        "mock with usage should emit TokenUsage, got: {updates:?}"
    );
}
