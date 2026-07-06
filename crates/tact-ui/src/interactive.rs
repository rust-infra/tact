use std::sync::Arc;

use tact::{
    Agent, AgentSystemPrompt,
    background::SharedBackgroundManager,
    config::CliArgs,
    consts::TactPath,
    cron::{CronScheduler, SharedCronScheduler},
    extract_text,
    mcp::load_mcp_router,
    memory::get_memory_manager,
    permission::PermissionManager,
    skill::get_skill_registry,
    store::{DynSessionStore, StoreRoot},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_llm::{
    get_llm_client, is_deepseek, is_kimi, query_deepseek_balance, query_kimi_balance,
};
use tact_protocol::{AgentErrorKind, AgentUpdate, UserCommand};

use crate::permission::permission_mode_from_config;
use crate::user_message::build_user_message;

pub(crate) async fn run_interactive(
    args: CliArgs,
    tact_path: TactPath,
    session_store: DynSessionStore,
) -> anyhow::Result<()> {
    let session_id = if let Some(ref id) = args.session {
        id.clone()
    } else if args.resume_last {
        let sessions = session_store.list_sessions().await?;
        sessions
            .into_iter()
            .next()
            .map(|s| s.id)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    session_store.create_session(&session_id).await?;
    let input_history = session_store.load_input_history(&session_id).await?;

    let client = get_llm_client()?;
    let mode = permission_mode_from_config();
    let permission_manager = PermissionManager::try_new(mode)?;
    eprintln!("[permission: {mode}]");

    let work_dir = tact_path.workdir().to_path_buf();
    let tui_work_dir = work_dir.clone();
    let image_work_dir = work_dir.clone();
    let skill_registry = Arc::new(get_skill_registry(tact_path.skills_dir())?);
    let store_root = StoreRoot::new(tact_path.claude_dir())?;
    let task_manager = SharedTaskManager::new(TaskManager::new(&store_root)?);
    let background_manager = SharedBackgroundManager::new(&store_root)?;
    let cron_scheduler = SharedCronScheduler::new(CronScheduler::new(&store_root)?);
    let teammate_manager = SharedTeammateManager::new(TeammateManager::new(&store_root)?);
    let worktree_manager =
        SharedWorktreeManager::new(WorktreeManager::new(&store_root, work_dir.clone())?);
    let memory_manager = Arc::new(std::sync::Mutex::new(get_memory_manager(
        tact_path.memory_dir(),
    )?));
    let mcp_router = load_mcp_router().await?;

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (user_cmd_tx, mut user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let agent_tx2 = agent_tx.clone();

    let tools = toolset();
    let tool_context = ToolContext {
        skill_registry: skill_registry.clone(),
        memory_manager,
        work_dir,
        task_manager,
        background_manager,
        cron_scheduler,
        teammate_manager,
        worktree_manager,
        ui_tx: Some(agent_tx.clone()),
    };

    let history_store = session_store.clone();
    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_ui_channel(agent_tx)
    .with_session(Some(session_id.clone()), session_store);

    let (history_save_tx, mut history_save_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    tokio::spawn(async move {
        while let Some((session_id, entry)) = history_save_rx.recv().await {
            let _ = history_store
                .append_input_history(&session_id, &entry)
                .await;
        }
    });

    let theme = tact::config::settings().ui.theme.clone();
    let balance_polling = is_deepseek() || is_kimi();
    let tui_handle = tokio::spawn(Box::pin(async move {
        tui::run_tui(
            agent_rx,
            user_cmd_tx,
            tui_work_dir,
            input_history,
            session_id,
            history_save_tx,
            theme,
            balance_polling,
        )
        .await
    }));

    if is_deepseek() || is_kimi() {
        let balance_tx = agent_tx2;
        tokio::spawn(async move {
            let result = if is_deepseek() {
                query_deepseek_balance().await
            } else {
                query_kimi_balance().await
            };
            if let Ok(balance) = result {
                let _ = balance_tx.send(AgentUpdate::Balance(balance));
            }
        });
    }

    while let Some(cmd) = user_cmd_rx.recv().await {
        match cmd {
            UserCommand::SubmitTask(task) => {
                agent.tool_use_counter = 0;

                let task_message = build_user_message(&task, &image_work_dir).await;
                if let Err(e) = agent.agent_loop(Some(task_message)).await {
                    agent.emit_update(AgentUpdate::Error(AgentErrorKind::Other(e.to_string())));
                }

                if let Some(last) = agent.runtime.context.last() {
                    let text = extract_text(&last.content);
                    agent.emit_update(AgentUpdate::TaskComplete(text));
                }
            }
            UserCommand::Cancel => {
                agent
                    .runtime
                    .cancel_flag
                    .store(true, std::sync::atomic::Ordering::Relaxed);
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

    tui_handle.await??;

    if let Some(ref sid) = agent.runtime.session_id {
        eprintln!("[session id: {sid}]");
    }

    eprintln!("{}", agent.runtime.stats.summary());

    agent.shutdown_mcp().await;
    Ok(())
}
