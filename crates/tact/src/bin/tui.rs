use std::sync::Arc;

use anthropic_ai_sdk::types::message::{Message, Role::User};
use tact::{
    Agent, AgentSystemPrompt,
    background::SharedBackgroundManager,
    consts::TactPath,
    cron::{CronScheduler, SharedCronScheduler},
    extract_text, get_llm_client,
    mcp::load_mcp_router,
    memory::get_memory_manager,
    permission::{PermissionManager, PermissionMode},
    skill::get_skill_registry,
    store::StoreRoot,
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_core::{AgentErrorKind, AgentUpdate, PlanStep, UserCommand};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize config: CLI args > env vars > TOML config file
    tact::config::init();

    if std::env::var("TOKIO_CONSOLE").is_ok() {
        console_subscriber::init();
        eprintln!("[tokio-console] listening on http://127.0.0.1:6669");
    }
    let client = get_llm_client()?;

    let permission_manager = PermissionManager::try_new(PermissionMode::Default)?;

    let tact_path = TactPath::from_cwd()?;
    let work_dir = tact_path.workdir().to_path_buf();
    let tui_work_dir = work_dir.clone();
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

    // Clone a copy for background tasks like balance query at startup
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

    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_ui_channel(agent_tx);

    let tui_handle = tokio::spawn(Box::pin(async move {
        tui::run_tui(agent_rx, user_cmd_tx, tui_work_dir).await
    }));

    // Query DeepSeek balance at startup
    if tact::llm::is_deepseek() {
        let balance_tx = agent_tx2;
        tokio::spawn(async move {
            match tact::llm::query_deepseek_balance().await {
                Ok(balance) => {
                    let _ = balance_tx.send(AgentUpdate::Balance(balance));
                }
                Err(e) => {
                    let _ = balance_tx.send(AgentUpdate::Error(
                        AgentErrorKind::BalanceQueryFailed(e.to_string()),
                    ));
                }
            }
        });
    }

    while let Some(cmd) = user_cmd_rx.recv().await {
        match cmd {
            UserCommand::SubmitTask(task) => {
                agent.runtime.context.push(Message::new_text(User, task));
                agent.tool_use_counter = 0;

                agent.emit_update(AgentUpdate::PlanGenerated(vec![PlanStep {
                    description: "Processing request...".to_string(),
                    tool: "agent_loop".to_string(),
                    args: std::collections::HashMap::new(),
                    need_approval: false,
                    output: None,
                }]));

                if let Err(e) = agent.agent_loop().await {
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
                if tact::llm::is_deepseek() {
                    match tact::llm::query_deepseek_balance().await {
                        Ok(balance) => {
                            agent.emit_update(AgentUpdate::Balance(balance));
                        }
                        Err(e) => {
                            agent.emit_update(AgentUpdate::Error(
                                AgentErrorKind::BalanceQueryFailed(e.to_string()),
                            ));
                        }
                    }
                } else {
                    //eprintln!("Only supported on DeepSeek");
                    agent.emit_update(AgentUpdate::Error(AgentErrorKind::BalanceNotSupported));
                }
            }
        }
    }

    tui_handle.await??;
    Ok(())
}
