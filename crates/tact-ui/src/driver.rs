//! Interactive-mode command driver: bridges `UserCommand` from the TUI to `Agent`.

use std::{path::Path, sync::atomic::Ordering};

use tact::{Agent, extract_text};
use tact_protocol::{AccountUpdate, AgentErrorKind, AgentUpdate, UserCommand};
use tokio::{
    sync::mpsc::{UnboundedReceiver, UnboundedSender},
    task::JoinHandle,
};

use crate::{account, user_message::build_user_message};

/// Process `UserCommand`s until the channel closes, then shut down MCP.
///
/// `SubmitTask` runs in a background task so `Cancel` can set `cancel_flag`
/// while `agent_loop` is in progress. Integration tests drive this with a fake TUI.
///
/// This convenience wrapper does **not** wire an account-update channel; balance
/// queries initiated through it are dropped. Use
/// [`run_command_loop_with_account`] when the caller wants to receive
/// [`AccountUpdate`] messages.
pub async fn run_command_loop(
    agent: Agent,
    user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
) -> Agent {
    run_command_loop_with_account(agent, user_cmd_rx, image_work_dir, None).await
}

/// Like [`run_command_loop`], but forwards balance / usage quota results to the
/// provided account-update channel instead of mixing them into agent updates.
pub async fn run_command_loop_with_account(
    agent: Agent,
    mut user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
    account_tx: Option<UnboundedSender<AccountUpdate>>,
) -> Agent {
    let image_work_dir = image_work_dir.as_ref().to_path_buf();
    let cancel_flag = agent.runtime.cancel_flag.clone();
    let ui_tx = agent.runtime.ui_tx.clone();
    // Shared stats snapshot: QueryStats can read it without awaiting the
    // in-flight task (the Agent itself is exclusively owned by that task).
    let stats = agent.runtime.stats.clone();
    // Shared UI responder: routes TUI select responses to the waiter (parent
    // or subagent) even while the Agent is owned by the in-flight task.
    let ui_responder = agent.tool_context.ui_responder.clone();

    let mut agent = Some(agent);
    let mut active: Option<JoinHandle<Agent>> = None;

    while let Some(cmd) = user_cmd_rx.recv().await {
        reap_finished_task(&mut agent, &mut active).await;

        match cmd {
            UserCommand::UiResponse(response) => {
                // Never await the in-flight task: the agent may be blocked
                // waiting for exactly this answer.
                ui_responder.handle_response(response);
            }
            UserCommand::Cancel => {
                cancel_flag.store(true, Ordering::Relaxed);
                if let Some(tx) = &ui_tx {
                    let _ = tx.send(AgentUpdate::Info("Cancelling...".into()));
                }
            }
            UserCommand::QueryStats => {
                // Immediate snapshot: does NOT wait for the running task —
                // stats live in an Arc<RwLock<SessionStats>> shared with the
                // agent, so /stats responds instantly even mid-run.
                let stats_text = stats.read().expect("session stats lock poisoned").summary();
                if let Some(tx) = &ui_tx {
                    let _ = tx.send(AgentUpdate::SessionStats(stats_text));
                }
            }
            UserCommand::SubmitTask(task) => {
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("submit task join panicked"));
                }
                let work_dir = image_work_dir.clone();
                let mut task_agent = agent.take().expect("agent available for submit");
                active = Some(tokio::spawn(async move {
                    handle_user_command(&mut task_agent, UserCommand::SubmitTask(task), &work_dir)
                        .await;
                    task_agent
                }));
            }
            other => {
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("command join panicked"));
                }
                if let Some(mut a) = agent.take() {
                    handle_user_command_with_account(
                        &mut a,
                        other,
                        &image_work_dir,
                        account_tx.as_ref(),
                    )
                    .await;
                    agent = Some(a);
                }
            }
        }
    }

    // The UI is gone: unblock any in-flight select waiter so the task can
    // finish instead of deadlocking on an answer that will never arrive.
    ui_responder.shutdown();

    if let Some(handle) = active.take() {
        agent = Some(handle.await.expect("final task join panicked"));
    }

    let mut agent = agent.expect("agent should be available after command loop");
    agent.shutdown_mcp().await;
    agent
}

async fn reap_finished_task(agent: &mut Option<Agent>, active: &mut Option<JoinHandle<Agent>>) {
    if let Some(handle) = active.as_mut()
        && handle.is_finished()
    {
        *agent = Some(handle.await.expect("finished task join panicked"));
        *active = None;
    }
}

/// Handle a single user command (shared by the loop and tests).
///
/// This wrapper discards any account-related updates; tests that need to
/// observe them should use [`run_command_loop_with_account`].
pub async fn handle_user_command(agent: &mut Agent, cmd: UserCommand, image_work_dir: &Path) {
    handle_user_command_with_account(agent, cmd, image_work_dir, None).await;
}

async fn handle_user_command_with_account(
    agent: &mut Agent,
    cmd: UserCommand,
    image_work_dir: &Path,
    account_tx: Option<&UnboundedSender<AccountUpdate>>,
) {
    match cmd {
        UserCommand::SubmitTask(task) => {
            agent.tool_use_counter = 0;
            agent.runtime.cancel_flag.store(false, Ordering::Relaxed);

            let task_message = build_user_message(&task, image_work_dir).await;

            // DeepSeek V4 and other text-only models reject `image_url` parts.
            // Reject images early rather than sending a broken request to the API.
            if task_message.has_images() && !tact_llm::supports_vision() {
                let model = tact_llm::get_provider().model;
                agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(format!(
                    "Image attachments are not supported by {model}. \
                     The current model does not accept image input."
                ))));
                return;
            }

            match agent.agent_loop(Some(task_message)).await {
                Ok(()) if !agent.runtime.cancel_flag.load(Ordering::Relaxed) => {
                    if let Some(last) = agent.runtime.context.last() {
                        let text = extract_text(&last.content);
                        agent.emit_update(AgentUpdate::TaskComplete(text));
                    }
                }
                Ok(()) => {
                    // Cancelled: clear TUI busy state (Planning/Executing) so
                    // queued (pending) messages are flushed rather than waiting
                    // on a stale busy state.
                    agent.emit_update(AgentUpdate::TaskCancelled);
                }
                Err(e) => {
                    agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(e.to_string())));
                }
            }
        }
        UserCommand::Compact => {
            agent.emit_update(AgentUpdate::Info("[compacting]".into()));
            if let Err(error) = agent.compact_history(None).await {
                agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(format!(
                    "Compaction failed: {error}"
                ))));
            } else {
                agent.emit_update(AgentUpdate::Info("Compaction complete.".into()));
            }
        }
        UserCommand::QueryBalance => {
            let Some(account_tx) = account_tx else {
                return;
            };
            if !account::is_supported() {
                return;
            }
            match account::query_once().await {
                Ok(result) => {
                    let _ = account_tx.send(account::into_update(result));
                }
                Err(err) => {
                    let _ = account_tx.send(AccountUpdate::Error(err));
                }
            }
        }
        UserCommand::QueryStats => {
            // Handled at the loop level (immediate shared-stats snapshot);
            // reaching this arm means the caller bypassed the command loop.
        }
        UserCommand::QueryBackground(task_id) => {
            match agent
                .tool_context
                .background_manager
                .check(task_id.as_deref())
                .await
            {
                Ok(output) => {
                    // Fenced code block keeps the one-line-per-task listing (and
                    // the single-task pretty JSON) aligned and copyable.
                    let md = format!("## ⚙️ Background Tasks\n\n```text\n{output}\n```");
                    agent.emit_update(AgentUpdate::MdInfo(md));
                }
                Err(err) => {
                    agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(format!(
                        "Background check failed: {err}"
                    ))));
                }
            }
        }
        UserCommand::SetPermissionMode(mode) => {
            let parsed = match mode.as_str() {
                "plan" => tact::permission::PermissionMode::Plan,
                "default" => tact::permission::PermissionMode::Default,
                _ => tact::permission::PermissionMode::Auto,
            };
            // TUI already shows the localized confirmation; do not emit Info here.
            agent.runtime.permission_manager.set_mode(parsed);
        }
        UserCommand::SetThinkingBudget(budget) => {
            agent.set_thinking_budget(budget);
        }
        UserCommand::SetReasoningEffort(effort) => {
            let parsed = effort.as_deref().and_then(|raw| raw.parse().ok());
            if effort.is_some() && parsed.is_none() {
                eprintln!("[driver] ignoring unparseable reasoning effort: {effort:?}");
            }
            agent.set_reasoning_effort(parsed);
        }
        UserCommand::SetModel(model) => {
            agent.set_model(model);
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use tact_llm::{ContentBlock, MockClient, StopReason};
    use tact_protocol::{AgentUpdate, UserCommand};

    use crate::test_support::{build_test_agent, install_test_config};

    fn text_block(content: &str) -> ContentBlock {
        ContentBlock::Text {
            text: content.to_string(),
        }
    }

    #[tokio::test]
    async fn cancel_sets_flag_and_emits_info() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (agent, _) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        agent.emit_update(AgentUpdate::Info("Cancelling...".into()));

        assert!(agent.runtime.cancel_flag.load(Ordering::Relaxed));
        let update = agent_rx.try_recv().expect("expected Cancelling info");
        assert!(matches!(update, AgentUpdate::Info(msg) if msg.contains("Cancelling")));
    }

    #[tokio::test]
    async fn submit_clears_cancel_flag_on_new_task() {
        install_test_config();
        let mock = MockClient::new(vec![(vec![text_block("done")], Some(StopReason::EndTurn))]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        super::handle_user_command(&mut agent, UserCommand::SubmitTask("go".into()), &work_dir)
            .await;

        assert!(!agent.runtime.cancel_flag.load(Ordering::Relaxed));
        let mut saw_complete = false;
        while let Ok(update) = agent_rx.try_recv() {
            if matches!(update, AgentUpdate::TaskComplete(_)) {
                saw_complete = true;
            }
        }
        assert!(saw_complete, "SubmitTask should clear cancel and complete");
    }

    #[tokio::test]
    async fn set_thinking_budget_changes_the_next_request() {
        install_test_config();
        let mock = MockClient::with_responder(|request, _| {
            assert_eq!(
                request
                    .thinking
                    .as_ref()
                    .map(|thinking| thinking.budget_tokens),
                Some(64_000)
            );
            Ok((vec![text_block("done")], Some(StopReason::EndTurn), None))
        });
        let (mut agent, work_dir) = build_test_agent(mock, None);

        super::handle_user_command(
            &mut agent,
            UserCommand::SetThinkingBudget(64_000),
            &work_dir,
        )
        .await;
        super::handle_user_command(
            &mut agent,
            UserCommand::SubmitTask("use the new budget".into()),
            &work_dir,
        )
        .await;
    }

    #[tokio::test]
    async fn set_thinking_budget_emits_model_info_for_status_bar() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::SetThinkingBudget(32_000),
            &work_dir,
        )
        .await;

        let mut saw_model_info = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::ModelInfo(params) = update {
                assert_eq!(params.thinking_budget, Some(32_000));
                assert!(params.max_tokens > 32_000);
                saw_model_info = true;
            }
        }
        assert!(
            saw_model_info,
            "SetThinkingBudget must emit ModelInfo so the TUI bar resyncs"
        );
    }

    #[tokio::test]
    async fn set_reasoning_effort_clears_stale_thinking_budget() {
        // Regression: switching an effort-semantic model (openai / deepseek /
        // kimi k3) must not leave a stale thinking budget behind — the bottom
        // bar would otherwise render a meaningless `think high(32K)`.
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::SetThinkingBudget(32_000),
            &work_dir,
        )
        .await;
        super::handle_user_command(
            &mut agent,
            UserCommand::SetReasoningEffort(Some("high".to_string())),
            &work_dir,
        )
        .await;

        let mut last: Option<tact_protocol::ModelCallParams> = None;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::ModelInfo(params) = update {
                last = Some(params);
            }
        }
        let last = last.expect("SetReasoningEffort must emit ModelInfo so the TUI bar resyncs");
        assert_eq!(last.reasoning_effort, Some("high".to_string()));
        assert_eq!(
            last.thinking_budget, None,
            "effort pick must clear stale thinking budget"
        );
    }

    #[tokio::test]
    async fn query_background_emits_mdinfo_listing_when_no_tasks() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(&mut agent, UserCommand::QueryBackground(None), &work_dir).await;

        let mut saw_md = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::MdInfo(md) = update {
                assert!(md.contains("Background Tasks"), "md: {md}");
                assert!(md.contains("No background tasks."), "md: {md}");
                saw_md = true;
            }
        }
        assert!(
            saw_md,
            "QueryBackground must emit MdInfo with the task listing"
        );
    }

    #[tokio::test]
    async fn query_background_unknown_id_emits_error() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(
            &mut agent,
            UserCommand::QueryBackground(Some("deadbeef".into())),
            &work_dir,
        )
        .await;

        let mut saw_error = false;
        while let Ok(update) = agent_rx.try_recv() {
            if let AgentUpdate::Error(err) = update {
                assert!(
                    err.to_string().contains("Unknown background task"),
                    "err: {err}"
                );
                saw_error = true;
            }
        }
        assert!(saw_error, "QueryBackground with unknown id must emit Error");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn query_stats_responds_immediately_while_task_runs() {
        use std::time::Duration;

        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        // The responder spins on an AtomicBool until the test releases it — a
        // deterministic "long running LLM call". A serialized QueryStats
        // (awaiting the task) would hang until the release fires.
        let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let release_rx = release.clone();
        let mock = MockClient::with_responder(move |_request, _| {
            while !release_rx.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok((vec![text_block("done")], Some(StopReason::EndTurn), None))
        });
        let (agent, work_dir) = build_test_agent(mock, Some(agent_tx));
        let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();

        let loop_handle = tokio::spawn(super::run_command_loop(agent, user_cmd_rx, work_dir));

        // Start a task that is now stuck in the responder, then ask for stats.
        user_cmd_tx
            .send(UserCommand::SubmitTask("long task".into()))
            .unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let start = std::time::Instant::now();
        user_cmd_tx.send(UserCommand::QueryStats).unwrap();

        // The stats snapshot must arrive while the task is still blocked.
        let mut saw_stats = false;
        loop {
            match tokio::time::timeout(Duration::from_millis(300), agent_rx.recv()).await {
                Ok(Some(AgentUpdate::SessionStats(_))) => {
                    saw_stats = true;
                    break;
                }
                Ok(Some(_)) => continue,
                Ok(None) | Err(_) => break,
            }
        }
        assert!(
            saw_stats,
            "expected SessionStats while the task is still running"
        );
        assert!(
            start.elapsed() < Duration::from_millis(450),
            "QueryStats must NOT await the in-flight task"
        );

        // Release the blocked task and let the loop drain.
        release.store(true, std::sync::atomic::Ordering::Relaxed);
        drop(user_cmd_tx);
        let _ = tokio::time::timeout(Duration::from_secs(5), loop_handle)
            .await
            .expect("command loop must finish");
    }
}
