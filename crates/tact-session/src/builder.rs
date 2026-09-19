//! Construct the main agent for an interactive session.
//!
//! Startup is deliberately deferred and fallible: front ends bring their window
//! or terminal up first, then call [`build_agent`], and surface any error into
//! the running UI instead of aborting before the first frame. Everything here is
//! transport-neutral — the caller supplies the `AgentUpdate` channel and the
//! [`UiResponder`] that answers select prompts.

use std::sync::Arc;

use tact::{
    Agent, AgentSystemPrompt,
    background::{BackgroundManager, SharedBackgroundManager},
    consts::TactPath,
    hook::HookControl,
    mcp::load_mcp_router_with_report,
    memory::memory_manager,
    permission::{PermissionManager, PermissionMode, settings::PermissionSettings},
    store::DynSessionStore,
    subagent::{SharedSubagentManager, SubagentManager},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset},
    ui_responder::UiResponder,
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_llm::get_llm_client;
use tact_protocol::AgentUpdate;

/// Install process settings for a front end that owns its own argv.
///
/// Call this exactly once before starting a session. It loads the TOML config
/// and installs the resolved settings, which is what lets `get_llm_client()`
/// report a missing API key as an `Err` instead of panicking on an
/// uninitialized provider. Host applications must not use
/// `tact::config::init()`: that would parse *their* command line.
pub fn init_config() -> anyhow::Result<tact::config::CliArgs> {
    tact::config::init_for_embedder()
}

/// The permission mode the process config asks for.
///
/// Only `plan` and `default` are spelled explicitly in config; every other
/// value (including an unset one) means the agent runs unattended.
pub fn permission_mode_from_config() -> PermissionMode {
    match tact::config::settings().permission_mode.as_deref() {
        Some("plan") => PermissionMode::Plan,
        Some("default") => PermissionMode::Default,
        _ => PermissionMode::Auto,
    }
}

/// Build the session's agent: LLM client, permission manager, session
/// managers, MCP servers, native toolset, tool context, and session-start
/// hooks.
///
/// MCP and sandbox problems are reported as `AgentUpdate::Info` on `agent_tx`
/// rather than failing startup — one broken server must not stop the session.
pub async fn build_agent(
    tact_path: TactPath,
    agent_tx: tokio::sync::mpsc::UnboundedSender<AgentUpdate>,
    skill_registry: tact::skill::SharedSkillRegistry,
    session_id: String,
    session_store: DynSessionStore,
    work_dir: std::path::PathBuf,
    ui_responder: UiResponder,
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
    // Memory is user-global (`~/.tact/memory`) so it persists across projects.
    // Project-local `.tact/memory` is only the fallback when `$HOME` is unset.
    let memory_manager = Arc::new(std::sync::Mutex::new(memory_manager(
        TactPath::home_memory_dir().unwrap_or_else(|| tact_path.memory_dir()),
    )?));
    let (mcp_router, mcp_report) = load_mcp_router_with_report().await?;
    // MCP problems are collected, never fatal (one broken server must not stop
    // startup), so surface them here — otherwise a typo'd command or an
    // ignored override would be silent. A clean load emits nothing.
    for line in mcp_report.notice_lines() {
        let _ = agent_tx.send(AgentUpdate::Info(line));
    }

    let mut tools = toolset();
    // Annotate `spawn_subagent` with the current subagent skill-card catalog
    // so the main agent can discover valid `skill:` names.
    tact::tool::annotate_spawn_subagent_skill_catalog(&mut tools);
    // Opt-in sandbox, resolved once: it is constant for the session lifetime, so
    // the bash description below can state the sandbox semantics truthfully.
    // A switch that cannot be honoured degrades to unsandboxed and is announced.
    let (sandbox, sandbox_degraded) =
        tact::sandbox::resolve(tact::config::settings().tools.sandbox, &work_dir);
    if let Some(degraded) = &sandbox_degraded {
        let _ = agent_tx.send(AgentUpdate::Info(degraded.reason.clone()));
    }
    if sandbox.is_some() {
        tools.set_tool_description("bash", tact::tool::SANDBOXED_BASH_DESCRIPTION);
    }
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
        ui_responder,
        progress_reporter: tact::tool::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bash_timeout_secs: tact::config::settings().tools.bash_timeout_secs,
        bash_nice: tact::config::settings().tools.bash_nice,
        sandbox,
        sandbox_degraded,
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
