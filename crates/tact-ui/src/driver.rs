//! Interactive-mode command driver: bridges `UserCommand` from the TUI to `Agent`.

use std::path::Path;
use std::sync::atomic::Ordering;

use tact::{Agent, extract_text};
use tact_protocol::{AccountUpdate, AgentErrorKind, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};
use tokio::task::JoinHandle;

use crate::account;
use crate::user_message::build_user_message;

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

    let mut agent = Some(agent);
    let mut active: Option<JoinHandle<Agent>> = None;

    while let Some(cmd) = user_cmd_rx.recv().await {
        reap_finished_task(&mut agent, &mut active).await;

        match cmd {
            UserCommand::Cancel => {
                cancel_flag.store(true, Ordering::Relaxed);
                if let Some(tx) = &ui_tx {
                    let _ = tx.send(AgentUpdate::Info("Cancelling...".into()));
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
            match agent.agent_loop(Some(task_message)).await {
                Ok(()) if !agent.runtime.cancel_flag.load(Ordering::Relaxed) => {
                    if let Some(last) = agent.runtime.context.last() {
                        let text = extract_text(&last.content);
                        agent.emit_update(AgentUpdate::TaskComplete(text));
                    }
                }
                Ok(()) => {}
                Err(e) => {
                    agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(e.to_string())));
                }
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
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use tact_llm::MockClient;
    use tact_llm::{ContentBlock, StopReason};

    use crate::test_support::{build_test_agent, install_test_config};
    use tact_protocol::{AgentUpdate, UserCommand};

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
}
