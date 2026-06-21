use std::path::Path;
use std::sync::Arc;

use anthropic_ai_sdk::types::message::{ContentBlock, ImageSource, Message, Role::User};
use base64::Engine as _;
use regex::Regex;
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
use tact_core::{AgentErrorKind, AgentUpdate, PlanStep, UserCommand};

/// Parse inline markdown image references like `![alt](path.png)` out of the
/// user's task, resolve them relative to the working directory, base64-encode
/// them, and build a multi-part user message for vision-capable models.
///
/// References that cannot be resolved are left in the text unchanged.
async fn build_user_message(task: &str, work_dir: &Path) -> Message {
    static IMAGE_RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    let re = IMAGE_RE.get_or_init(|| Regex::new(r"!\[([^\]]*)\]\(([^)]+)\)").unwrap());

    let mut blocks = Vec::new();
    let mut last_end = 0;

    for cap in re.captures_iter(task) {
        let whole = cap.get(0).unwrap();
        let alt = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let path_str = cap.get(2).map(|m| m.as_str()).unwrap_or("");

        // Text before this image reference.
        if whole.start() > last_end {
            blocks.push(ContentBlock::Text {
                text: task[last_end..whole.start()].to_string(),
            });
        }

        let resolved = work_dir.join(path_str);
        match load_image_block(&resolved).await {
            Some(source) => {
                // Keep the alt text so the model still sees the description.
                if !alt.is_empty() {
                    blocks.push(ContentBlock::Text {
                        text: format!("({})", alt),
                    });
                }
                blocks.push(ContentBlock::Image { source });
            }
            None => {
                // Leave the original markdown in place if the file cannot be loaded.
                blocks.push(ContentBlock::Text {
                    text: whole.as_str().to_string(),
                });
            }
        }

        last_end = whole.end();
    }

    if last_end < task.len() {
        blocks.push(ContentBlock::Text {
            text: task[last_end..].to_string(),
        });
    }

    if blocks.is_empty() {
        // Fallback for empty input.
        return Message::new_text(User, "");
    }

    Message::new_blocks(User, blocks)
}

async fn load_image_block(path: &Path) -> Option<ImageSource> {
    if !path.is_file() {
        return None;
    }
    let bytes = tokio::fs::read(path).await.ok()?;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let media_type = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "tiff" | "tif" => "image/tiff",
        _ => "image/png",
    };
    Some(ImageSource {
        type_: "base64".to_string(),
        media_type: media_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(&bytes),
    })
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize config: CLI args > env vars > TOML config file
    let args = tact::config::init();

    if std::env::var("TOKIO_CONSOLE").is_ok() {
        console_subscriber::init();
        eprintln!("[tokio-console] listening on http://127.0.0.1:6669");
    }

    let tact_path = TactPath::from_cwd()?;
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
                let updated = s.updated_at.format("%Y-%m-%d %H:%M:%S").to_string();
                let title = s.title.as_deref().unwrap_or("(untitled)");
                println!(
                    "{:<36}  {:>4}  {:<20}  {:.40}",
                    s.id, s.message_count, updated, title
                );
            }
        }
        return Ok(());
    }

    let client = get_llm_client()?;

    let permission_manager = PermissionManager::try_new(PermissionMode::Default)?;

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

    // Resolve session_id: --session takes priority, then --resume-last
    let session_id = if let Some(ref id) = args.session {
        Some(id.clone())
    } else if args.resume_last {
        let sessions = session_store.list_sessions().await?;
        sessions.into_iter().next().map(|s| s.id)
    } else {
        None
    };

    let mut agent = Agent::new(
        client.clone(),
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_ui_channel(agent_tx)
    .with_session(session_id, session_store);

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
                Err(_) => {
                    //  let _ = balance_tx.send(AgentUpdate::Error(
                    //     AgentErrorKind::BalanceQueryFailed(e.to_string()),
                    // ));
                }
            }
        });
    }

    while let Some(cmd) = user_cmd_rx.recv().await {
        match cmd {
            UserCommand::SubmitTask(task) => {
                agent.tool_use_counter = 0;

                agent.emit_update(AgentUpdate::PlanGenerated(vec![PlanStep {
                    description: "Processing request...".to_string(),
                    tool: "agent_loop".to_string(),
                    args: std::collections::HashMap::new(),
                    need_approval: false,
                    output: None,
                }]));

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
