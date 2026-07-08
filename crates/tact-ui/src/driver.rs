//! Interactive-mode command driver: bridges `UserCommand` from the TUI to `Agent`.

use std::path::Path;
use std::sync::atomic::Ordering;

use tact::{Agent, extract_text};
use tact_llm::{is_deepseek, is_kimi, query_deepseek_balance, query_kimi_balance};
use tact_protocol::{AgentErrorKind, AgentUpdate, UserCommand};
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

use crate::user_message::build_user_message;

/// Process `UserCommand`s until the channel closes, then shut down MCP.
///
/// `SubmitTask` runs in a background task so `Cancel` can set `cancel_flag`
/// while `agent_loop` is in progress. Integration tests drive this with a fake TUI.
pub async fn run_command_loop(
    agent: Agent,
    mut user_cmd_rx: UnboundedReceiver<UserCommand>,
    image_work_dir: impl AsRef<Path>,
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
                    handle_user_command(
                        &mut task_agent,
                        UserCommand::SubmitTask(task),
                        &work_dir,
                    )
                    .await;
                    task_agent
                }));
            }
            other => {
                if let Some(handle) = active.take() {
                    agent = Some(handle.await.expect("command join panicked"));
                }
                if let Some(mut a) = agent.take() {
                    handle_user_command(&mut a, other, &image_work_dir).await;
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
        *agent = Some(
            handle
                .await
                .expect("finished task join panicked"),
        );
        *active = None;
    }
}

/// Handle a single user command (shared by the loop and tests).
pub async fn handle_user_command(agent: &mut Agent, cmd: UserCommand, image_work_dir: &Path) {
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
        UserCommand::Cancel => {
            agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
            agent.emit_update(AgentUpdate::Info("Cancelling...".into()));
        }
        UserCommand::QueryBalance => {
            let result = if is_deepseek() {
                query_deepseek_balance().await
            } else if is_kimi() {
                query_kimi_balance().await
            } else {
                Err(anyhow::anyhow!("balance query not supported for current provider"))
            };
            match result {
                Ok(balance) => {
                    agent.emit_update(AgentUpdate::Balance(balance));
                }
                Err(e) => {
                    agent.emit_update(AgentUpdate::Error(
                        AgentErrorKind::BalanceQueryFailed(e.to_string()),
                    ));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use anthropic_ai_sdk::types::message::{ContentBlock, StopReason};
    use tact_llm::MockClient;
    use tact_protocol::{AgentErrorKind, AgentUpdate, UserCommand};

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
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(&mut agent, UserCommand::Cancel, &work_dir).await;

        assert!(agent.runtime.cancel_flag.load(Ordering::Relaxed));
        let update = agent_rx.try_recv().expect("expected Cancelling info");
        assert!(matches!(update, AgentUpdate::Info(msg) if msg.contains("Cancelling")));
    }

    #[tokio::test]
    async fn submit_clears_cancel_flag_on_new_task() {
        install_test_config();
        let mock = MockClient::new(vec![(
            vec![text_block("done")],
            Some(StopReason::EndTurn),
        )]);
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(mock, Some(agent_tx));

        agent.runtime.cancel_flag.store(true, Ordering::Relaxed);
        super::handle_user_command(
            &mut agent,
            UserCommand::SubmitTask("go".into()),
            &work_dir,
        )
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
    async fn query_balance_emits_balance_query_failed() {
        install_test_config();
        let (agent_tx, mut agent_rx) = tokio::sync::mpsc::unbounded_channel();
        let (mut agent, work_dir) = build_test_agent(MockClient::new(vec![]), Some(agent_tx));

        super::handle_user_command(&mut agent, UserCommand::QueryBalance, &work_dir).await;

        let update = agent_rx.try_recv().expect("expected balance error");
        assert!(matches!(
            update,
            AgentUpdate::Error(AgentErrorKind::BalanceQueryFailed(_))
        ));
    }
}
