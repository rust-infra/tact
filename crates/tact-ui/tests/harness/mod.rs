//! Shared helpers for tact-ui integration tests.

use anthropic_ai_sdk::types::message::{ContentBlock, StopReason};
use tact::permission::PermissionMode;
use tact_ui::driver::run_command_loop;
use tact_ui::test_support::{
    build_test_agent_with_mode, collect_updates_after, install_test_config, user_command_channels,
};
use tact_llm::MockClient;
use tact_protocol::{AgentUpdate, TokenUsageInfo, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};
use tokio::task::JoinHandle;

pub fn text_block(content: &str) -> ContentBlock {
    ContentBlock::Text {
        text: content.to_string(),
    }
}

pub fn read_file_tool_use(id: &str, path: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "read_file".to_string(),
        input: serde_json::json!({ "path": path }),
    }
}

pub fn write_file_tool_use(id: &str, path: &str, content: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "write_file".to_string(),
        input: serde_json::json!({ "path": path, "content": content }),
    }
}

pub fn edit_file_tool_use(id: &str, path: &str, old_text: &str, new_text: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "edit_file".to_string(),
        input: serde_json::json!({
            "path": path,
            "old_text": old_text,
            "new_text": new_text,
        }),
    }
}

pub fn bash_tool_use(id: &str, command: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "bash".to_string(),
        input: serde_json::json!({ "command": command }),
    }
}

pub fn sample_token_usage() -> TokenUsageInfo {
    TokenUsageInfo {
        prompt: 120,
        completion: 30,
        total: 150,
        prompt_cache_hit_tokens: 40,
        prompt_cache_miss_tokens: 80,
        reasoning_tokens: 5,
    }
}

/// Auto-respond to [`AgentUpdate::RequestSelect`] with `choice` (`0` = allow once).
pub fn wire_permission_responder(
    agent_rx: UnboundedReceiver<AgentUpdate>,
    choice: Option<usize>,
) -> UnboundedReceiver<AgentUpdate> {
    let (collect_tx, collect_rx) = unbounded_channel();
    tokio::spawn(async move {
        let mut agent_rx = agent_rx;
        while let Some(update) = agent_rx.recv().await {
            match update {
                AgentUpdate::RequestSelect { respond, .. } => {
                    let _ = respond.send(choice);
                }
                other => {
                    let _ = collect_tx.send(other);
                }
            }
        }
    });
    collect_rx
}

/// Run a single SubmitTask through the driver and collect agent updates.
pub async fn run_single_task(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
) -> (Vec<AgentUpdate>, std::path::PathBuf) {
    run_single_task_with_setup(mock, task, permission_mode, |_| {}).await
}

/// Like [`run_single_task`], but runs `setup` on the workspace before submitting.
pub async fn run_single_task_with_setup(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
    setup: impl FnOnce(&std::path::Path),
) -> (Vec<AgentUpdate>, std::path::PathBuf) {
    run_single_task_with_permission_choice(mock, task, permission_mode, None, setup).await
}

/// Like [`run_single_task_with_setup`], but auto-responds to permission prompts.
pub async fn run_single_task_with_permission_choice(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
    permission_choice: Option<usize>,
    setup: impl FnOnce(&std::path::Path),
) -> (Vec<AgentUpdate>, std::path::PathBuf) {
    install_test_config();
    let (agent_tx, agent_rx) = unbounded_channel();
    let collect_rx = wire_permission_responder(agent_rx, permission_choice);
    let (agent, work_dir) = build_test_agent_with_mode(mock, Some(agent_tx), permission_mode);
    setup(&work_dir);
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir.clone()));

    user_cmd_tx
        .send(UserCommand::SubmitTask(task.into()))
        .unwrap();
    drop(user_cmd_tx);

    driver.await.unwrap();
    let updates = collect_updates_after(collect_rx).await;
    (updates, work_dir)
}

/// Drive the command loop with custom commands; returns updates and work dir.
pub async fn run_commands<F>(
    mock: MockClient,
    permission_mode: PermissionMode,
    send_cmds: F,
) -> (Vec<AgentUpdate>, std::path::PathBuf)
where
    F: FnOnce(UnboundedSender<UserCommand>) -> JoinHandle<()>,
{
    run_commands_with_permission_choice(mock, permission_mode, None, send_cmds).await
}

/// Like [`run_commands`], but auto-responds to permission prompts.
pub async fn run_commands_with_permission_choice<F>(
    mock: MockClient,
    permission_mode: PermissionMode,
    permission_choice: Option<usize>,
    send_cmds: F,
) -> (Vec<AgentUpdate>, std::path::PathBuf)
where
    F: FnOnce(UnboundedSender<UserCommand>) -> JoinHandle<()>,
{
    install_test_config();
    let (agent_tx, agent_rx) = unbounded_channel();
    let collect_rx = wire_permission_responder(agent_rx, permission_choice);
    let (agent, work_dir) = build_test_agent_with_mode(mock, Some(agent_tx), permission_mode);
    let (user_cmd_tx, user_cmd_rx) = user_command_channels();

    let driver = tokio::spawn(run_command_loop(agent, user_cmd_rx, work_dir.clone()));
    let cmd_handle = send_cmds(user_cmd_tx);
    cmd_handle.await.unwrap();
    driver.await.unwrap();

    let updates = collect_updates_after(collect_rx).await;
    (updates, work_dir)
}

pub fn step_finished_ids(updates: &[AgentUpdate]) -> Vec<String> {
    updates
        .iter()
        .filter_map(|u| match u {
            AgentUpdate::StepFinished(_, id, _) => Some(id.clone()),
            _ => None,
        })
        .collect()
}

pub fn first_index(updates: &[AgentUpdate], pred: impl Fn(&AgentUpdate) -> bool) -> Option<usize> {
    updates.iter().position(pred)
}

pub fn mock_turn(
    blocks: Vec<ContentBlock>,
    stop: StopReason,
) -> (Vec<ContentBlock>, Option<StopReason>) {
    (blocks, Some(stop))
}

pub fn mock_turn_with_usage(
    blocks: Vec<ContentBlock>,
    stop: StopReason,
    usage: TokenUsageInfo,
) -> (Vec<ContentBlock>, Option<StopReason>, TokenUsageInfo) {
    (blocks, Some(stop), usage)
}
