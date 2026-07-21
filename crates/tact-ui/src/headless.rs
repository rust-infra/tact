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
    store::{DynSessionStore, StoreRoot},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_llm::get_llm_client;

use crate::{
    permission::permission_mode_from_config,
    session_lock::{SessionLockGuard, SessionLockRegistry},
    user_message::build_user_message,
};

pub async fn run_headless(
    args: CliArgs,
    prompt: String,
    tact_path: TactPath,
    session_store: DynSessionStore,
    lock_registry: Arc<SessionLockRegistry>,
) -> anyhow::Result<()> {
    if prompt.trim().is_empty() {
        eprintln!("Usage: tact-ui headless <PROMPT>");
        eprintln!("Try 'tact-ui headless --help' for more information.");
        std::process::exit(1);
    }

    let root_dir = tact_path.workdir().display().to_string();
    let session_id = if let Some(ref id) = args.session {
        id.clone()
    } else if args.resume_last {
        let sessions = session_store.list_sessions(Some(&root_dir)).await?;
        sessions
            .into_iter()
            .next()
            .map(|s| s.id)
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string())
    } else {
        uuid::Uuid::new_v4().to_string()
    };

    session_store
        .ensure_session_row(&session_id, &root_dir)
        .await?;
    let session_lock = SessionLockGuard::acquire(session_store.clone(), &session_id).await?;
    lock_registry.register(session_lock.clone()).await;
    session_store.touch_session(&session_id, &root_dir).await?;

    eprintln!("[session: {session_id}]");

    let run_result = run_headless_locked(args, prompt, tact_path, session_store, session_id).await;

    session_lock.release().await?;
    run_result
}

async fn run_headless_locked(
    _args: CliArgs,
    prompt: String,
    tact_path: TactPath,
    session_store: DynSessionStore,
    session_id: String,
) -> anyhow::Result<()> {
    let client = get_llm_client()?;
    let mode = permission_mode_from_config();
    let permission_manager = PermissionManager::try_new(mode)?;
    eprintln!("[permission: {mode}]");

    let store_root = StoreRoot::new(tact_path.claude_dir())?;
    let work_dir = tact_path.workdir().to_path_buf();
    let skill_registry = tact::skill::shared_skill_registry(tact_path.workdir())?;
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

    let tools = toolset();
    let tool_context = ToolContext {
        skill_registry: skill_registry.clone(),
        memory_manager,
        work_dir: work_dir.clone(),
        task_manager,
        background_manager,
        cron_scheduler,
        teammate_manager,
        worktree_manager,
        ui_tx: None,
        progress_reporter: tact::tool::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bash_timeout_secs: tact::config::settings().tools.bash_timeout_secs,
    };

    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_session(session_id.clone(), session_store);

    // Restore any prior messages for resumed sessions.
    agent.ensure_session().await?;

    let prompt_message = build_user_message(&prompt, &work_dir).await;
    agent.agent_loop(Some(prompt_message)).await?;

    eprintln!("[session id: {session_id}]");
    eprintln!("{}", agent.runtime.stats.summary());

    if let Some(final_content) = agent.runtime.context.last() {
        let text = extract_text(&final_content.content);
        println!("{text}");

        let summary = text.chars().take(200).collect::<String>();
        let _ = tact::notifications::notify_task_complete(&summary);
    }

    agent.shutdown_mcp().await;
    Ok(())
}
