//! Shared helpers for tact-ui integration tests.
#![allow(dead_code, unused_imports)]

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tact::permission::PermissionMode;
use tact_llm::MockClient;
use tact_llm::{ContentBlock, StopReason};
use tact_protocol::{AgentUpdate, TokenUsageInfo, UserCommand};
use tact_ui::driver::run_command_loop;
use tact_ui::test_support::{
    build_test_agent_with_config, build_test_agent_with_mcp, build_test_agent_with_mode,
    collect_updates_after, install_test_config, install_test_config_with, user_command_channels,
};
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

/// Run a task with a custom MCP router injected into the agent.
pub async fn run_single_task_with_mcp(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
    permission_choice: Option<usize>,
    mcp_router: tact::mcp::MCPToolRouter,
) -> (Vec<AgentUpdate>, std::path::PathBuf) {
    install_test_config();
    let (agent_tx, agent_rx) = unbounded_channel();
    let collect_rx = wire_permission_responder(agent_rx, permission_choice);
    let (agent, work_dir) =
        build_test_agent_with_mcp(mock, Some(agent_tx), permission_mode, mcp_router);
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

/// Like [`run_single_task_with_setup`], but installs a custom `tact::config`.
pub async fn run_single_task_with_config(
    mock: MockClient,
    task: &str,
    permission_mode: PermissionMode,
    config: tact::config::ResolvedConfig,
    setup: impl FnOnce(&std::path::Path),
) -> (Vec<AgentUpdate>, std::path::PathBuf) {
    let (agent_tx, agent_rx) = unbounded_channel();
    let collect_rx = wire_permission_responder(agent_rx, None);
    let (agent, work_dir) =
        build_test_agent_with_config(mock, Some(agent_tx), permission_mode, &config);
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
            AgentUpdate::StepFinished { tool_id: id, .. } => Some(id.clone()),
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

// ── Additional tool-use builders ─────────────────────────────────────

pub fn apply_patch_tool_use(id: &str, patch: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "apply_patch".to_string(),
        input: serde_json::json!({ "patch": patch }),
    }
}

pub fn batch_read_tool_use(id: &str, paths: &[&str]) -> ContentBlock {
    let files: Vec<serde_json::Value> = paths
        .iter()
        .map(|p| serde_json::json!({ "path": p }))
        .collect();
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "batch_read".to_string(),
        input: serde_json::json!({ "files": files }),
    }
}

pub fn batch_edit_tool_use(id: &str, edits: &[(&str, &str, &str)]) -> ContentBlock {
    let edits: Vec<serde_json::Value> = edits
        .iter()
        .map(|(path, old, new)| {
            serde_json::json!({
                "file_path": path,
                "old_string": old,
                "new_string": new,
            })
        })
        .collect();
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "batch_edit".to_string(),
        input: serde_json::json!({ "edits": edits }),
    }
}

pub fn search_code_tool_use(id: &str, query: &str, path: Option<&str>) -> ContentBlock {
    let mut input = serde_json::json!({ "query": query });
    if let Some(p) = path {
        input["path"] = serde_json::Value::String(p.to_string());
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "search_code".to_string(),
        input,
    }
}

pub fn lsp_tool_use(
    id: &str,
    action: &str,
    file: &str,
    line: Option<u32>,
    column: Option<u32>,
) -> ContentBlock {
    let mut input = serde_json::json!({
        "action": action,
        "file": file,
    });
    if let Some(l) = line {
        input["line"] = serde_json::Value::Number(l.into());
    }
    if let Some(c) = column {
        input["column"] = serde_json::Value::Number(c.into());
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "lsp".to_string(),
        input,
    }
}

pub fn web_search_tool_use(id: &str, query: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "web_search".to_string(),
        input: serde_json::json!({ "query": query }),
    }
}

pub fn web_fetch_tool_use(id: &str, url: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "web_fetch".to_string(),
        input: serde_json::json!({ "url": url }),
    }
}

pub fn task_tool_use(id: &str, prompt: &str, description: Option<&str>) -> ContentBlock {
    let mut input = serde_json::json!({ "prompt": prompt });
    if let Some(d) = description {
        input["description"] = serde_json::Value::String(d.to_string());
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "task".to_string(),
        input,
    }
}

pub fn spawn_teammate_tool_use(id: &str, name: &str, role: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "spawn_teammate".to_string(),
        input: serde_json::json!({ "name": name, "role": role }),
    }
}

pub fn send_message_tool_use(id: &str, from: &str, to: &str, body: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "send_message".to_string(),
        input: serde_json::json!({ "from": from, "to": to, "body": body }),
    }
}

pub fn broadcast_tool_use(id: &str, from: &str, body: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "broadcast".to_string(),
        input: serde_json::json!({ "from": from, "body": body }),
    }
}

pub fn read_inbox_tool_use(id: &str, owner: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "read_inbox".to_string(),
        input: serde_json::json!({ "owner": owner }),
    }
}

pub fn plan_approval_tool_use(id: &str, from: &str, to: &str, body: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "plan_approval".to_string(),
        input: serde_json::json!({ "from": from, "to": to, "body": body }),
    }
}

pub fn save_memory_tool_use(
    id: &str,
    name: &str,
    memory_type: &str,
    description: &str,
    content: &str,
) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "save_memory".to_string(),
        input: serde_json::json!({
            "name": name,
            "type": memory_type,
            "description": description,
            "content": content,
        }),
    }
}

pub fn load_skill_tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "load_skill".to_string(),
        input: serde_json::json!({ "name": name }),
    }
}

pub fn worktree_create_tool_use(id: &str, name: &str, base_ref: Option<&str>) -> ContentBlock {
    let mut input = serde_json::json!({ "name": name });
    if let Some(r) = base_ref {
        input["base_ref"] = serde_json::Value::String(r.to_string());
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "worktree_create".to_string(),
        input,
    }
}

pub fn worktree_list_tool_use(id: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "worktree_list".to_string(),
        input: serde_json::json!({}),
    }
}

pub fn worktree_status_tool_use(id: &str, name: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "worktree_status".to_string(),
        input: serde_json::json!({ "name": name }),
    }
}

pub fn cron_create_tool_use(id: &str, cron: &str, prompt: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "cron_create".to_string(),
        input: serde_json::json!({ "cron": cron, "prompt": prompt }),
    }
}

pub fn cron_list_tool_use(id: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "cron_list".to_string(),
        input: serde_json::json!({}),
    }
}

pub fn cron_delete_tool_use(id: &str, cron_id: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "cron_delete".to_string(),
        input: serde_json::json!({ "id": cron_id }),
    }
}

pub fn ask_user_tool_use(id: &str, question: &str, options: Option<&[&str]>) -> ContentBlock {
    let mut input = serde_json::json!({ "question": question });
    if let Some(opts) = options {
        input["options"] = serde_json::Value::Array(
            opts.iter()
                .map(|o| serde_json::Value::String(o.to_string()))
                .collect(),
        );
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "ask_user".to_string(),
        input,
    }
}

pub fn sleep_tool_use(id: &str, ms: u64) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "sleep".to_string(),
        input: serde_json::json!({ "ms": ms }),
    }
}

pub fn background_run_tool_use(id: &str, command: &str) -> ContentBlock {
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "background_run".to_string(),
        input: serde_json::json!({ "command": command }),
    }
}

pub fn check_background_tool_use(id: &str, task_id: Option<&str>) -> ContentBlock {
    let mut input = serde_json::json!({});
    if let Some(t) = task_id {
        input["task_id"] = serde_json::Value::String(t.to_string());
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "check_background".to_string(),
        input,
    }
}

pub fn compact_tool_use(id: &str, focus: Option<&str>) -> ContentBlock {
    let mut input = serde_json::json!({});
    if let Some(f) = focus {
        input["focus"] = serde_json::Value::String(f.to_string());
    }
    ContentBlock::ToolUse {
        id: id.to_string(),
        name: "compact".to_string(),
        input,
    }
}

// ── Permission responders ────────────────────────────────────────────

/// Auto-respond to permission prompts using a sequence of choices.
///
/// `choices[idx]` is sent for the `idx`-th `RequestSelect`. If the sequence
/// is exhausted, subsequent prompts are denied (`None`).
pub fn wire_permission_responder_with_choices(
    agent_rx: UnboundedReceiver<AgentUpdate>,
    choices: Vec<Option<usize>>,
) -> UnboundedReceiver<AgentUpdate> {
    let (collect_tx, collect_rx) = unbounded_channel();
    tokio::spawn(async move {
        let mut agent_rx = agent_rx;
        let mut choices = choices.into_iter();
        while let Some(update) = agent_rx.recv().await {
            match update {
                AgentUpdate::RequestSelect { respond, .. } => {
                    let choice = choices.next().unwrap_or(None);
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

// ── Assertion helpers ────────────────────────────────────────────────

pub fn step_result<'a>(
    updates: &'a [AgentUpdate],
    id: &str,
) -> Option<&'a tact_protocol::StepResult> {
    updates.iter().find_map(|u| match u {
        AgentUpdate::StepFinished {
            tool_id, result, ..
        } if tool_id == id => Some(result),
        _ => None,
    })
}

pub fn step_succeeded(updates: &[AgentUpdate], id: &str) -> bool {
    step_result(updates, id)
        .map(|r| matches!(r.status, tact_protocol::StepStatus::Success))
        .unwrap_or(false)
}

pub fn step_failed(updates: &[AgentUpdate], id: &str) -> bool {
    updates
        .iter()
        .any(|u| matches!(u, AgentUpdate::StepFailed { tool_id, .. } if tool_id == id))
}

pub fn task_completed_with(updates: &[AgentUpdate], substring: &str) -> bool {
    updates.iter().any(|u| match u {
        AgentUpdate::TaskComplete(text) => text.contains(substring),
        _ => false,
    })
}

pub fn request_select_count(updates: &[AgentUpdate]) -> usize {
    updates
        .iter()
        .filter(|u| matches!(u, AgentUpdate::RequestSelect { .. }))
        .count()
}

pub fn token_usage_total(updates: &[AgentUpdate]) -> tact_protocol::TokenUsageInfo {
    let mut total = tact_protocol::TokenUsageInfo {
        prompt: 0,
        completion: 0,
        total: 0,
        prompt_cache_hit_tokens: 0,
        prompt_cache_miss_tokens: 0,
        reasoning_tokens: 0,
    };
    for u in updates {
        if let AgentUpdate::TokenUsage(usage) = u {
            total.prompt += usage.prompt;
            total.completion += usage.completion;
            total.total += usage.total;
            total.prompt_cache_hit_tokens += usage.prompt_cache_hit_tokens;
            total.prompt_cache_miss_tokens += usage.prompt_cache_miss_tokens;
            total.reasoning_tokens += usage.reasoning_tokens;
        }
    }
    total
}

/// Assert that update `a` appears before update `b` in the stream.
pub fn assert_update_before(
    updates: &[AgentUpdate],
    a_pred: impl Fn(&AgentUpdate) -> bool,
    b_pred: impl Fn(&AgentUpdate) -> bool,
    msg: &str,
) {
    let a = updates.iter().position(&a_pred);
    let b = updates.iter().position(&b_pred);
    assert!(
        a.is_some() && b.is_some() && a < b,
        "{msg}: positions a={a:?}, b={b:?}",
    );
}

/// Load persisted messages from a SQLite session store for verification.
pub async fn load_session_messages(
    store: &tact::store::DynSessionStore,
    session_id: &str,
) -> Vec<tact_llm::Message> {
    store
        .load_session(session_id)
        .await
        .expect("load session messages")
}

/// Like [`wire_permission_responder_with_choices`], but also returns an atomic
/// counter that is incremented for every `RequestSelect` that is handled.
pub fn wire_permission_responder_with_counter(
    agent_rx: UnboundedReceiver<AgentUpdate>,
    choices: Vec<Option<usize>>,
) -> (UnboundedReceiver<AgentUpdate>, Arc<AtomicUsize>) {
    let (collect_tx, collect_rx) = unbounded_channel();
    let counter = Arc::new(AtomicUsize::new(0));
    let counter_clone = counter.clone();
    tokio::spawn(async move {
        let mut agent_rx = agent_rx;
        let mut choices = choices.into_iter();
        while let Some(update) = agent_rx.recv().await {
            match update {
                AgentUpdate::RequestSelect { respond, .. } => {
                    counter_clone.fetch_add(1, Ordering::Relaxed);
                    let choice = choices.next().unwrap_or(None);
                    let _ = respond.send(choice);
                }
                other => {
                    let _ = collect_tx.send(other);
                }
            }
        }
    });
    (collect_rx, counter)
}
