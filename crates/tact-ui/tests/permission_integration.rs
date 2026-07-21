//! Permission + RequestSelect scenarios via the driver harness.

mod harness;

use harness::{
    bash_tool_use, edit_file_tool_use, mock_turn, run_single_task_with_permission_choice,
};
use tact::{permission::PermissionMode, tool::test_support::write_workspace_file};
use tact_llm::{MockClient, StopReason};
use tact_protocol::{AgentUpdate, StepStatus};

#[tokio::test]
async fn default_mode_allow_once_runs_edit_file() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![edit_file_tool_use("e1", "lib.rs", "fn old()", "fn new()")],
            StopReason::ToolUse,
        ),
        mock_turn(
            vec![harness::text_block("Edit complete.")],
            StopReason::EndTurn,
        ),
    ]);

    let (updates, work_dir) = run_single_task_with_permission_choice(
        mock,
        "edit lib.rs",
        PermissionMode::Default,
        Some(0),
        |dir| write_workspace_file(dir, "lib.rs", "fn old() {}"),
    )
    .await;

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { tool_id: id, result, .. }
                    if id == "e1"
                        && result.tool == "edit_file"
                        && matches!(result.status, StepStatus::Success)
            )
        }),
        "allow-once should run edit_file, got: {updates:?}"
    );
    let content = std::fs::read_to_string(work_dir.join("lib.rs")).unwrap();
    assert!(content.contains("fn new()"));
}

#[tokio::test]
async fn default_mode_deny_blocks_edit_file() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![edit_file_tool_use("e1", "lib.rs", "fn old()", "fn new()")],
            StopReason::ToolUse,
        ),
        mock_turn(
            vec![harness::text_block("Edit denied.")],
            StopReason::EndTurn,
        ),
    ]);

    let (updates, work_dir) = run_single_task_with_permission_choice(
        mock,
        "edit lib.rs",
        PermissionMode::Default,
        Some(1),
        |dir| write_workspace_file(dir, "lib.rs", "fn old() {}"),
    )
    .await;

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFailed { tool_id: id, error: msg, .. }
                    if id == "e1" && msg.contains("denied")
            )
        }),
        "deny should StepFailed edit_file, got: {updates:?}"
    );
    let content = std::fs::read_to_string(work_dir.join("lib.rs")).unwrap();
    assert!(content.contains("fn old()"));
    assert!(!content.contains("fn new()"));
}

#[tokio::test]
async fn high_risk_bash_shell_guard_blocks_after_permission_allow() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![bash_tool_use("bash_sudo", "sudo echo ok")],
            StopReason::ToolUse,
        ),
        mock_turn(vec![harness::text_block("Done.")], StopReason::EndTurn),
    ]);

    let (updates, _work_dir) = run_single_task_with_permission_choice(
        mock,
        "run sudo echo",
        PermissionMode::Default,
        Some(0),
        |_| {},
    )
    .await;

    assert!(
        updates.iter().any(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { tool_id: id, result, .. }
                    if id == "bash_sudo"
                        && matches!(result.status, StepStatus::Failed)
                        && result.message.contains("Dangerous command blocked")
                        && result.permission_label.as_deref() == Some("Allow once")
            )
        }),
        "shell guard should block even after user allow, got: {updates:?}"
    );
}

#[tokio::test]
async fn always_allow_skips_second_permission_prompt() {
    let mock = MockClient::new(vec![
        mock_turn(
            vec![edit_file_tool_use("e1", "a.rs", "v1", "v2")],
            StopReason::ToolUse,
        ),
        mock_turn(
            vec![edit_file_tool_use("e2", "a.rs", "v2", "v3")],
            StopReason::ToolUse,
        ),
        mock_turn(
            vec![harness::text_block("Both edits.")],
            StopReason::EndTurn,
        ),
    ]);

    let (updates, work_dir) = run_single_task_with_permission_choice(
        mock,
        "edit twice",
        PermissionMode::Default,
        Some(2),
        |dir| write_workspace_file(dir, "a.rs", "v1"),
    )
    .await;

    let request_selects = updates
        .iter()
        .filter(|u| {
            matches!(
                u,
                AgentUpdate::StepFinished { result, .. }
                    if result.permission_label.as_deref() == Some("Always allow this tool")
            )
        })
        .count();
    assert_eq!(
        request_selects, 1,
        "always-allow label should appear once, got: {updates:?}"
    );
    assert_eq!(
        std::fs::read_to_string(work_dir.join("a.rs")).unwrap(),
        "v3"
    );
}
