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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize config: CLI args > env vars > TOML config file
    let args = tact::config::init();

    if args.prompt.is_empty() {
        eprintln!("Usage: tact <PROMPT>");
        eprintln!("       tact [OPTIONS] <PROMPT>");
        eprintln!("Try 'tact --help' for more information.");
        std::process::exit(1);
    }

    let client = get_llm_client()?;

    // Permission mode: CLI/env/config, default to "auto" for non-interactive MVP
    let mode = match args.permission_mode.as_deref() {
        Some("plan") => PermissionMode::Plan,
        Some("default") => PermissionMode::Default,
        _ => PermissionMode::Auto,
    };
    let permission_manager = PermissionManager::try_new(mode)?;
    eprintln!("[permission: {mode}]");

    let tact_path = TactPath::from_cwd()?;
    let work_dir = tact_path.workdir().to_path_buf();
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
        ui_tx: None,
    };

    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    );

    agent
        .runtime
        .context
        .push(Message::new_text(User, args.prompt));

    agent.agent_loop().await?;

    // Print session statistics
    eprintln!("{}", agent.runtime.stats.summary());

    let Some(final_content) = agent.runtime.context.last() else {
        return Ok(());
    };
    println!("{}", extract_text(&final_content.content));

    Ok(())
}
