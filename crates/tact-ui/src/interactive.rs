use std::sync::Arc;

use tact::{
    Agent, AgentSystemPrompt,
    background::{BackgroundManager, SharedBackgroundManager},
    config::CliArgs,
    consts::TactPath,
    hook::HookControl,
    mcp::load_mcp_router,
    memory::memory_manager,
    permission::{PermissionManager, settings::PermissionSettings},
    store::DynSessionStore,
    subagent::{SharedSubagentManager, SubagentManager},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_llm::get_llm_client;
use tact_protocol::{AccountUpdate, AgentErrorKind, AgentUpdate};

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

    let work_dir = tact_path.workdir().to_path_buf();
    let tui_work_dir = work_dir.clone();
    let image_work_dir = work_dir.clone();
    let skill_registry = tact::skill::shared_skill_registry(tact_path.workdir())?;

    // Bring the TUI up as early as possible so users are not staring at a
    // blank terminal while the slower startup steps (LLM client, MCP server
    // connect/handshake, session hooks) run below. User commands typed while
    // the driver has not started yet are buffered by the unbounded channel
    // and processed once the driver task is spawned — the UI never blocks on
    // MCP readiness, and nothing is dropped.
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

    // History saver owns a clone of the session store; the TUI closure below
    // moves the original. Clone first.
    let history_store = session_store.clone();
    let (history_save_tx, mut history_save_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, String)>();
    tokio::spawn(async move {
        while let Some((session_id, entry)) = history_save_rx.recv().await {
            let _ = history_store
                .append_input_history(&session_id, &entry)
                .await;
        }
    });

    // The agent built below also needs these after the TUI closure moves the
    // originals, so take clones now.
    let agent_skill_registry = skill_registry.clone();
    let agent_session_id = session_id.clone();
    let agent_session_store = session_store.clone();

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

    // ── Slower startup steps now run with the TUI already visible. ──────────
    // None of these are needed for the UI to render, so they are deliberately
    // deferred past the TUI spawn above. Because the TUI is already in raw
    // mode / alternate screen at this point, a failure here must NOT propagate
    // via `?` (that would leave the terminal unrestored with a detached TUI
    // task). Instead the error is delivered into the running TUI as an
    // `AgentUpdate::Error`, the user quits normally, and `run_tui` restores
    // the terminal.
    let driver = match build_agent_for_interactive(
        tact_path,
        agent_tx.clone(),
        agent_skill_registry,
        agent_session_id,
        agent_session_store,
        work_dir,
    )
    .await
    {
        Ok(agent) => Some(tokio::spawn(run_command_loop_with_account(
            agent,
            user_cmd_rx,
            image_work_dir,
            Some(account_tx),
        ))),
        Err(err) => {
            // Deliver the failure into the already-running TUI instead of
            // propagating it past the raw-mode boundary. The TUI shows the
            // error and the user quits normally (restoring the terminal).
            let _ = agent_tx.send(AgentUpdate::Error(AgentErrorKind::Other(format!(
                "startup failed: {err:#}"
            ))));
            None
        }
    };

    tui_handle.await??;
    // If startup failed, there is no driver task to await; the error was
    // already surfaced in the TUI above and the terminal has been restored.
    if let Some(driver) = driver {
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
    }

    Ok(())
}

/// Run the deferred, fallible startup steps needed to construct the main agent:
/// LLM client, permission setup, session managers, MCP server connect, native
/// toolset, tool context, and the session-start hooks. Invoked after the TUI is
/// already visible; any error is surfaced in the TUI rather than propagated
/// through the raw-mode boundary (see the caller).
async fn build_agent_for_interactive(
    tact_path: TactPath,
    agent_tx: tokio::sync::mpsc::UnboundedSender<AgentUpdate>,
    skill_registry: tact::skill::SharedSkillRegistry,
    session_id: String,
    session_store: DynSessionStore,
    work_dir: std::path::PathBuf,
) -> anyhow::Result<Agent> {
    let client = get_llm_client().await?;
    let mode = permission_mode_from_config();
    let settings = PermissionSettings::load(&tact_path);
    let permission_manager = PermissionManager::try_new_with_settings(mode, settings)?;
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
    // Memory is user-global (`~/.tact/memory`, like Claude Code's `~/.claude`)
    // so it persists across projects. Project-local `.tact/memory` is only the
    // fallback when `$HOME` is unset.
    let memory_manager = Arc::new(std::sync::Mutex::new(memory_manager(
        TactPath::home_memory_dir().unwrap_or_else(|| tact_path.memory_dir()),
    )?));
    let mcp_router = load_mcp_router().await?;

    let mut tools = toolset();
    // Annotate `spawn_subagent` with the current subagent skill-card catalog
    // so the main agent can discover valid `skill:` names.
    tact::tool::annotate_spawn_subagent_skill_catalog(&mut tools);
    let tool_context = ToolContext {
        skill_registry: skill_registry.clone(),
        subagent_start_hooks: tact::plugin::plugin_subagent_start_hooks(tact_path.workdir())?,
        subagent_stop_hooks: tact::plugin::plugin_subagent_stop_hooks(tact_path.workdir())?,
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
    .with_session(session_id, session_store)
    .with_provider_kind(provider_kind)
    .with_session_start(|_at| Box::pin(async move { Ok(HookControl::Continue) }))
    .with_pre_tool(|_at, _tool_use| Box::pin(async move { Ok(HookControl::Continue) }))
    .with_post_tool(tact::hook::rtk_filter::create_rtk_post_tool_hook());
    // Claude plugin command hooks (SessionStart / UserPromptSubmit /
    // PreToolUse / PostToolUse) from every installed plugin.
    agent = tact::plugin::apply_plugin_hooks(agent, tact_path.workdir())?;
    // SessionStart hooks fire once per session, right after initialization.
    agent.dispatch_session_start_hooks().await?;

    Ok(agent)
}
