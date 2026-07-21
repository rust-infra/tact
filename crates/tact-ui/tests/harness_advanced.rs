//! Extended harness capabilities: dynamic mock LLM, error injection,
//! streaming chunks, batch tools, and permission sequences.

mod harness;

use std::sync::{Arc, Mutex};

use harness::{
    assert_update_before, batch_read_tool_use, edit_file_tool_use, run_single_task, step_failed, step_succeeded,
    task_completed_with, text_block, token_usage_total, wire_permission_responder_with_counter,
};
use tact::{permission::PermissionMode, tool::test_support::write_workspace_file};
use tact_llm::{LlmError, MockClient, StopReason};
use tact_protocol::{AgentUpdate, UserCommand};

#[tokio::test]
async fn dynamic_mock_inspects_request_and_branches() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let captured_clone = captured.clone();

    let mock = MockClient::with_responder(move |request, idx| {
        let summary = format!("turn={} messages={}", idx, request.messages.len());
        captured_clone.lock().unwrap().push(summary);

        if idx == 0 {
            Ok((vec![harness::read_file_tool_use("read1", "data.txt")], Some(StopReason::ToolUse), None))
        } else {
            Ok((vec![text_block("Saw tool result in request.")], Some(StopReason::EndTurn), None))
        }
    });

    let (updates, _work_dir) = run_single_task_with_setup(mock, "read data", PermissionMode::Auto, |dir| {
        write_workspace_file(dir, "data.txt", "dynamic mock data")
    })
    .await;

    assert!(step_succeeded(&updates, "read1"));
    assert!(task_completed_with(&updates, "Saw tool result"));

    let summaries = captured.lock().unwrap();
    assert_eq!(summaries.len(), 2, "expected two LLM calls");
    assert!(
        summaries[1].contains("messages=3"),
        "second call should include user + assistant + tool result messages: {summaries:?}"
    );
}

#[tokio::test]
async fn mock_timeout_retries_then_succeeds() {
    let mock = MockClient::with_responder(move |_request, idx| {
        if idx == 0 {
            Err(LlmError::Other("request timeout".to_string()))
        } else {
            Ok((vec![text_block("Recovered after timeout.")], Some(StopReason::EndTurn), None))
        }
    });

    let (updates, _work_dir) = run_single_task(mock, "retry timeout", PermissionMode::Auto).await;

    assert!(
        updates
            .iter()
            .any(|u| matches!(u, AgentUpdate::Info(msg) if msg.contains("Recovery") && msg.contains("backoff"))),
        "expected backoff recovery info, got: {updates:?}"
    );
    assert!(task_completed_with(&updates, "Recovered after timeout"));
}

#[tokio::test]
async fn streaming_mock_emits_chunks() {
    let mock = MockClient::new(vec![(vec![text_block("Hello streaming world.")], Some(StopReason::EndTurn))])
        .with_streaming_chunks();

    let (updates, _work_dir) = run_single_task(mock, "stream", PermissionMode::Auto).await;

    let chunks: Vec<String> = updates
        .iter()
        .filter_map(|u| match u {
            AgentUpdate::StreamChunk(s) => Some(s.clone()),
            _ => None,
        })
        .collect();
    assert!(!chunks.is_empty(), "expected stream chunks");
    let reconstructed = chunks.join("");
    assert_eq!(reconstructed, "Hello streaming world.");
    assert!(task_completed_with(&updates, "Hello streaming world."));
}

#[tokio::test]
async fn batch_read_files_succeeds() {
    let mock = MockClient::new(vec![
        (vec![batch_read_tool_use("batch1", &["a.txt", "b.txt"])], Some(StopReason::ToolUse)),
        (vec![text_block("Batch read done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, _work_dir) = run_single_task_with_setup(mock, "batch read", PermissionMode::Auto, |dir| {
        write_workspace_file(dir, "a.txt", "alpha");
        write_workspace_file(dir, "b.txt", "beta");
    })
    .await;

    assert!(step_succeeded(&updates, "batch1"));
    assert!(task_completed_with(&updates, "Batch read done."));
}

#[tokio::test]
async fn permission_sequence_allow_then_deny() {
    let mock = MockClient::new(vec![
        (vec![edit_file_tool_use("e1", "lib.rs", "v1", "v2")], Some(StopReason::ToolUse)),
        (vec![edit_file_tool_use("e2", "lib.rs", "v2", "v3")], Some(StopReason::ToolUse)),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, work_dir, prompt_count) = run_single_task_with_permission_choice_and_choices(
        mock,
        "edit twice",
        PermissionMode::Default,
        vec![Some(0), Some(1)], // allow once, then deny
        |dir| write_workspace_file(dir, "lib.rs", "v1"),
    )
    .await;

    assert!(step_succeeded(&updates, "e1"), "first edit should succeed: {updates:?}");
    assert!(step_failed(&updates, "e2"), "second edit should be denied: {updates:?}");
    assert_eq!(prompt_count.load(std::sync::atomic::Ordering::Relaxed), 2, "expected two permission prompts");

    let content = std::fs::read_to_string(work_dir.join("lib.rs")).unwrap();
    assert_eq!(content, "v2", "only first edit should be applied");
}

#[tokio::test]
async fn token_usage_aggregates_across_turns() {
    let u1 = harness::sample_token_usage();
    let mut u2 = u1.clone();
    u2.prompt += 10;
    u2.completion += 5;
    u2.total = u2.prompt + u2.completion;

    let mock = MockClient::with_usage(vec![
        harness::mock_turn_with_usage(
            vec![harness::read_file_tool_use("r1", "data.txt")],
            StopReason::ToolUse,
            u1.clone(),
        ),
        harness::mock_turn_with_usage(vec![text_block("Done.")], StopReason::EndTurn, u2.clone()),
    ]);

    let (updates, _work_dir) = run_single_task_with_setup(mock, "count tokens", PermissionMode::Auto, |dir| {
        write_workspace_file(dir, "data.txt", "token data")
    })
    .await;

    let total = token_usage_total(&updates);
    assert_eq!(total.prompt, u1.prompt + u2.prompt);
    assert_eq!(total.completion, u1.completion + u2.completion);
    assert_eq!(total.total, u1.total + u2.total);
}

#[tokio::test]
async fn update_order_read_before_write() {
    let mock = MockClient::new(vec![
        (
            vec![
                harness::read_file_tool_use("r1", "source.txt"),
                harness::write_file_tool_use("w1", "dest.txt", "copied"),
            ],
            Some(StopReason::ToolUse),
        ),
        (vec![text_block("Done.")], Some(StopReason::EndTurn)),
    ]);

    let (updates, _work_dir) = run_single_task_with_setup(mock, "read and write", PermissionMode::Auto, |dir| {
        write_workspace_file(dir, "source.txt", "original")
    })
    .await;

    assert_update_before(
        &updates,
        |u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "r1"),
        |u| matches!(u, AgentUpdate::StepFinished { tool_id: id, .. } if id == "w1"),
        "read must finish before write",
    );
}

// Helper: like run_single_task_with_permission_choice but uses a sequence of choices.
async fn run_single_task_with_permission_choice_and_choices(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
    choices: Vec<Option<usize>>,
    setup: impl FnOnce(&std::path::Path),
) -> (Vec<AgentUpdate>, std::path::PathBuf, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use tact_ui::test_support::{
        build_test_agent_with_mode, collect_updates_after, install_test_config, user_command_channels,
    };
    use tokio::sync::mpsc::unbounded_channel;

    install_test_config();
    let (agent_tx, agent_rx) = unbounded_channel();
    let (collect_rx, prompt_count) = wire_permission_responder_with_counter(agent_rx, choices);
    let (agent, work_dir) = build_test_agent_with_mode(mock, Some(agent_tx), permission_mode);
    setup(&work_dir);
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(tact_ui::driver::run_command_loop(agent, user_cmd_rx, work_dir.clone()));

    user_cmd_tx.send(UserCommand::SubmitTask(task.into())).unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();
    let updates = collect_updates_after(collect_rx).await;
    (updates, work_dir, prompt_count)
}

async fn run_single_task_with_setup(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
    setup: impl FnOnce(&std::path::Path),
) -> (Vec<AgentUpdate>, std::path::PathBuf) {
    harness::run_single_task_with_setup(mock, task, permission_mode, setup).await
}
