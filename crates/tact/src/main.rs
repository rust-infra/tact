use std::sync::Arc;

use anthropic_ai_sdk::types::message::{Message, Role::User};
use chrono::{DateTime, Utc};

use tact::{
    session_store::open_sqlite_session_store,
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

    let tact_path = TactPath::from_cwd()?;
    let store_root = StoreRoot::new(tact_path.claude_dir())?;
    let db_path = tact_path.claude_dir().join("tact.db");
    let session_store = open_sqlite_session_store(&db_path).await?;

    // --list-sessions: print recent sessions and exit
    if args.list_sessions {
        let sessions = session_store.list_sessions().await?;
        if sessions.is_empty() {
            println!("No sessions found.");
        } else {
            println!("{:<36}  {:>4}  {:<20}  {:<40}", "SESSION ID", "MSGS", "UPDATED", "TITLE");
            println!("{}", "-".repeat(110));
            for s in &sessions {
                let updated = format_timestamp(s.updated_at);
                let title = s.title.as_deref().unwrap_or("(untitled)");
                println!(
                    "{:<36}  {:>4}  {:<20}  {:.40}",
                    s.id, s.message_count, updated, title
                );
            }
        }
        return Ok(());
    }

    // Resolve session_id: --session takes priority, then --resume-last
    let session_id = if let Some(ref id) = args.session {
        Some(id.clone())
    } else if args.resume_last {
        let sessions = session_store.list_sessions().await?;
        sessions.into_iter().next().map(|s| s.id)
    } else {
        None
    };

    if session_id.is_some() {
        eprintln!("[session: {}]", session_id.as_deref().unwrap_or("new"));
    }

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
        work_dir,
        task_manager,
        background_manager,
        cron_scheduler,
        teammate_manager,
        worktree_manager,
        ui_tx: None,
    };

    let is_new_session = session_id.is_none();
    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_session(session_id, session_store);

    // Materialise the session (create or resume) so we can set title.
    let _ = agent.ensure_session().await?;

    // Auto-title new sessions from first line of prompt
    if is_new_session {
        let title = args.prompt.lines().next().unwrap_or("").trim();
        if !title.is_empty() {
            let title = if title.chars().count() > 80 {
                format!("{}…", title.chars().take(77).collect::<String>())
            } else {
                title.to_string()
            };
            agent.set_session_title(Some(&title)).await?;
        }
    }

    let prompt_message = Message::new_text(User, args.prompt);
    agent.agent_loop(Some(prompt_message)).await?;

    // Print session ID so user can resume later
    if let Some(ref sid) = agent.runtime.session_id {
        eprintln!("[session id: {sid}]");
    }

    // Print session statistics
    eprintln!("{}", agent.runtime.stats.summary());

    let Some(final_content) = agent.runtime.context.last() else {
        return Ok(());
    };
    let text = extract_text(&final_content.content);
    println!("{text}");

    // Send a desktop notification with the final result summary
    let summary = text.chars().take(200).collect::<String>();
    let _ = tact::notifications::notify_task_complete(&summary);

    Ok(())
}

fn format_timestamp(dt: DateTime<Utc>) -> String {
    dt.format("%Y-%m-%d %H:%M:%S").to_string()
}
