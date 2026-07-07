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
use tact_llm::get_llm_client;

use crate::permission::permission_mode_from_config;
use crate::session_lock::{SessionLockGuard, SessionLockRegistry};
use crate::user_message::build_user_message;

pub(crate) async fn run_headless(
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
        Some(id.clone())
    } else if args.resume_last {
        let sessions = session_store.list_sessions(Some(&root_dir)).await?;
        sessions.into_iter().next().map(|s| s.id)
    } else {
        None
    };

    if let Some(ref sid) = session_id {
        eprintln!("[session: {sid}]");
    }

    let client = get_llm_client()?;
    let mode = permission_mode_from_config();
    let permission_manager = PermissionManager::try_new(mode)?;
    eprintln!("[permission: {mode}]");

    let store_root = StoreRoot::new(tact_path.claude_dir())?;
    let work_dir = tact_path.workdir().to_path_buf();
    let skill_registry = Arc::new(get_skill_registry(tact_path.skills_dir())?);
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
    };

    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_session(session_id, session_store.clone());

    let sid = agent.ensure_session().await?;

    let session_lock = {
        let lock = SessionLockGuard::acquire(session_store.clone(), &sid).await?;
        lock_registry.register(lock.clone()).await;
        session_store.touch_session(&sid, &root_dir).await?;
        Some(lock)
    };

    let run_result = async {
        let prompt_message = build_user_message(&prompt, &work_dir).await;
        agent.agent_loop(Some(prompt_message)).await?;

        eprintln!("[session id: {sid}]");
        eprintln!("{}", agent.runtime.stats.summary());

        if let Some(final_content) = agent.runtime.context.last() {
            let text = extract_text(&final_content.content);
            println!("{text}");

            let summary = text.chars().take(200).collect::<String>();
            let _ = tact::notifications::notify_task_complete(&summary);
        }

        agent.shutdown_mcp().await;
        Ok::<(), anyhow::Error>(())
    }
    .await;

    if let Some(lock) = session_lock {
        lock.release().await?;
    }
    run_result
}
