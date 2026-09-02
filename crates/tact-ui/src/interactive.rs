use std::sync::Arc;

use tact::{
    Agent, AgentSystemPrompt,
    background::{BackgroundManager, SharedBackgroundManager},
    config::CliArgs,
    consts::TactPath,
    hook::HookControl,
    mcp::load_mcp_router,
    memory::get_memory_manager,
    permission::{PermissionManager, settings::PermissionSettings},
    store::DynSessionStore,
    subagent::{SharedSubagentManager, SubagentManager},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_llm::get_llm_client;
use tact_protocol::AccountUpdate;

use crate::{
    account,
    driver::run_command_loop_with_account,
    permission::permission_mode_from_config,
    session_lock::{SessionLockGuard, SessionLockRegistry},
};

pub async fn run_interactive(
    args: CliArgs,
    tact_path: TactPath,
    session_store: DynSessionStore,
    lock_registry: Arc<SessionLockRegistry>,
) -> anyhow::Result<()> {
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
        .ensure_session_row(&session_id, &root_dir, "")
        .await?;
    let session_lock = SessionLockGuard::acquire(session_store.clone(), &session_id).await?;
    lock_registry.register(session_lock.clone()).await;
    session_store.touch_session(&session_id, &root_dir).await?;

    let run_result = run_interactive_locked(
        args,
        tact_path,
        session_store,
        session_id,
        session_lock.clone(),
    )
    .await;

    session_lock.release().await?;
    run_result
}

async fn run_interactive_locked(
    _args: CliArgs,
    tact_path: TactPath,
    session_store: DynSessionStore,
    session_id: String,
    session_lock: Arc<SessionLockGuard>,
) -> anyhow::Result<()> {
    let _keep_lock = session_lock;
    let input_history = session_store.load_input_history(&session_id).await?;

    let client = get_llm_client().await?;
    let mode = permission_mode_from_config();
    let settings = PermissionSettings::load(&tact_path);
    let permission_manager = PermissionManager::try_new_with_settings(mode, settings)?;
    eprintln!("[permission: {mode}]");

    let work_dir = tact_path.workdir().to_path_buf();
    let tui_work_dir = work_dir.clone();
    let image_work_dir = work_dir.clone();
    let skill_registry = tact::skill::shared_skill_registry(tact_path.workdir())?;
    let task_manager =
        SharedTaskManager::new(TaskManager::new(&tact_path.session_db_path()).await?);
    let background_manager =
        SharedBackgroundManager::new(BackgroundManager::new(&tact_path.session_db_path()).await?);
    let teammate_manager =
        SharedTeammateManager::new(TeammateManager::new(&tact_path.session_db_path()).await?);
    let worktree_manager = SharedWorktreeManager::new(
        WorktreeManager::new(&tact_path.session_db_path(), work_dir.clone()).await?,
    );
    let subagent_manager =
        SharedSubagentManager::new(SubagentManager::new(&tact_path.session_db_path()).await?);
    let memory_manager = Arc::new(std::sync::Mutex::new(get_memory_manager(
        tact_path.memory_dir(),
    )?));
    let mcp_router = load_mcp_router().await?;

    let (agent_tx, agent_rx) = tokio::sync::mpsc::unbounded_channel();
    let (account_tx, account_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plugin_tx, plugin_request_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plugin_event_tx, plugin_rx) = tokio::sync::mpsc::unbounded_channel();
    let (user_cmd_tx, user_cmd_rx) = tokio::sync::mpsc::unbounded_channel();
    let _plugin_worker = match tact::consts::PluginHome::from_environment() {
        Some(plugin_home) => {
            tact::plugin::spawn_worker(plugin_home, plugin_request_rx, plugin_event_tx)
        }
        None => tact::plugin::spawn_unavailable_worker(plugin_request_rx, plugin_event_tx),
    };

    let tools = toolset();
    let tool_context = ToolContext {
        skill_registry: skill_registry.clone(),
        agent_registry: tact::agent_def::shared_agent_definition_registry(tact_path.workdir())?,
        subagent_start_hooks: tact::plugin::plugin_subagent_start_hooks(tact_path.workdir())?,
        memory_manager,
        work_dir,
        task_manager,
        background_manager,
        teammate_manager,
        worktree_manager,
        subagent_manager,
        ui_tx: Some(agent_tx.clone()),
        ui_responder: tact::ui_responder::UiResponder::new(),
        progress_reporter: tact::tool::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bash_timeout_secs: tact::config::settings().tools.bash_timeout_secs,
        bash_nice: tact::config::settings().tools.bash_nice,
        session_id: None,
        session_store: None,
        permission_snapshot: None,
        subagent_results: None,
    };

    let history_store = session_store.clone();
    // Responses compaction routing depends on the effective provider: OpenAI
    // uses native `/responses/compact`, DeepSeek (including an OpenAI entry
    // pointed at a DeepSeek endpoint) falls back to local summary compaction.
    let provider_kind = if tact_llm::is_deepseek() {
        tact_llm::ProviderKind::DeepSeek
    } else {
        tact_llm::get_provider().provider
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
    .with_session(session_id.clone(), session_store.clone())
    .with_provider_kind(provider_kind)
    .with_session_start(|_at| Box::pin(async move { Ok(HookControl::Continue) }))
    .with_pre_tool(|_at, _tool_use| Box::pin(async move { Ok(HookControl::Continue) }))
    .with_post_tool(tact::hook::rtk_filter::create_rtk_post_tool_hook());
    // Claude plugin command hooks (SessionStart / UserPromptSubmit /
    // PreToolUse / PostToolUse) from every installed plugin.
    agent = tact::plugin::apply_plugin_hooks(agent, tact_path.workdir())?;
    // SessionStart hooks fire once per session, right after initialization.
    agent.dispatch_session_start_hooks().await?;

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
    let model_context_window = tact::config::settings().agent.model_context_window;
    let model_name = tact::config::settings().agent.model.clone();
    let model_max_tokens = tact::config::settings().agent.max_tokens;
    let model_thinking_budget = tact::config::settings().agent.thinking_budget;
    let account_enabled = account::is_supported();
    let tui_handle = tokio::spawn(Box::pin(async move {
        let account_rx = if account_enabled {
            Some(account_rx)
        } else {
            None
        };
        tui::run_tui(tui::TuiConfig {
            agent_rx,
            account_rx,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            work_dir: tui_work_dir,
            input_history_entries: input_history,
            session_id,
            session_store,
            history_save_tx,
            theme,
            model_context_window,
            model_name,
            model_max_tokens,
            model_thinking_budget,
            permission_mode: match tact::config::settings().permission_mode.as_deref() {
                Some("plan") => "plan",
                Some("default") => "default",
                _ => "auto",
            }
            .to_string(),
            skills_description: {
                let reg = tact::skill::lock_skills(&skill_registry);
                reg.describe_available()
            },
            skills_data: {
                let reg = tact::skill::lock_skills(&skill_registry);
                reg.skills()
                    .values()
                    .map(|doc| tui::SkillEntry {
                        name: doc.manifest.name.clone(),
                        description: doc.manifest.description.clone(),
                        body: doc.body.clone(),
                    })
                    .collect()
            },
            skill_registry,
            voice: tact::config::settings().voice.clone(),
            voice_parsed_keybind: tact::config::settings()
                .voice
                .voice_keybind
                .as_deref()
                .and_then(tui::parse_voice_keybind),
        })
        .await
    }));

    if account_enabled {
        // Initial query on startup so the bottom bar can show data immediately.
        let startup_tx = account_tx.clone();
        tokio::spawn(async move {
            match account::query_once().await {
                Ok(result) => {
                    let _ = startup_tx.send(account::into_update(result));
                }
                Err(err) => {
                    let _ = startup_tx.send(AccountUpdate::Error(err));
                }
            }
        });
        account::spawn_poller(account_tx.clone());
    }

    let driver = tokio::spawn(run_command_loop_with_account(
        agent,
        user_cmd_rx,
        image_work_dir,
        Some(account_tx),
    ));

    tui_handle.await??;
    let agent = driver.await.expect("command driver task panicked");

    if let Some(sid) = agent.runtime.session_id.as_ref() {
        eprintln!("[session id: {sid}]");
    }

    eprintln!(
        "{}",
        agent
            .runtime
            .stats
            .read()
            .expect("session stats lock poisoned")
            .summary()
    );

    Ok(())
}
