//! The one session bootstrap, shared by both frontends.
//!
//! `tact-ui headless` and the interactive TUI build the same agent: the same
//! LLM client, permission setup, five session managers, MCP router, native
//! toolset, sandbox resolution, tool context and hook pass. They differed only
//! in *where output goes*, which had been copied into both functions along with
//! the ~50 lines of construction around it.
//!
//! The two axes that genuinely differ are stated as arguments:
//!
//! - [`Notices`] — a process with no UI writes startup problems to stderr; the
//!   TUI sends them into a frame it is already drawing.
//! - [`UiWiring`] — the TUI registers an `AgentUpdate` channel and hands the
//!   agent the `UiResponder` its prompts are answered through; headless has
//!   neither.

use std::path::PathBuf;
use std::sync::Arc;

use tact::{
    Agent, AgentSystemPrompt,
    background::{BackgroundManager, SharedBackgroundManager},
    config::CliArgs,
    consts::TactPath,
    mcp::load_mcp_router_with_report,
    memory::memory_manager,
    permission::{PermissionManager, PermissionMode, settings::PermissionSettings},
    skill::SharedSkillRegistry,
    store::DynSessionStore,
    subagent::{SharedSubagentManager, SubagentManager},
    task::{SharedTaskManager, TaskManager},
    team::{SharedTeammateManager, TeammateManager},
    tool::{ToolContext, toolset_with_memory},
    ui_responder::UiResponder,
    worktree::{SharedWorktreeManager, WorktreeManager},
};
use tact_llm::get_llm_client;
use tact_protocol::AgentUpdate;
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    permission::permission_mode_from_config,
    session_lock::{SessionLockGuard, SessionLockRegistry},
};

/// Where a session's startup notices go.
///
/// Both variants carry the same finding; they differ in what a viewer can
/// tolerate. Naming this rather than passing a `bool` is what let the two
/// bootstrap bodies collapse into one.
pub enum Notices {
    /// `[tag] message` on stderr, for a process with no UI.
    ///
    /// Headless has no frame to protect and no screen to draw on, so stderr is
    /// the only place an operator can learn that an MCP server failed to
    /// connect, that the sandbox was not honoured, or that a repository hook
    /// was left untrusted.
    Stderr,
    /// An `Info` row in the TUI.
    ///
    /// The TUI owns the alternate screen by the time it starts building the
    /// agent, so a stray `eprintln!` would land in the middle of a frame.
    Ui(UnboundedSender<AgentUpdate>),
}

impl Notices {
    /// Report one startup finding.
    ///
    /// `tag` is dropped on the UI path: a row is already styled and reads on
    /// its own there, while a bare stderr line has nothing to say which of the
    /// startup steps produced it.
    fn notice(&self, tag: &str, message: &str) {
        match self {
            Self::Ui(tx) => {
                let _ = tx.send(AgentUpdate::Info(message.to_string()));
            }
            Self::Stderr => eprintln!("{}", stderr_line(tag, message)),
        }
    }

    /// Report the permission mode the session resolved to.
    ///
    /// Only headless takes this up: the TUI's status bar already shows the
    /// mode, so a row repeating it would be noise. The permission setup is
    /// shared, so this is a method the frontend opts into rather than a special
    /// case at the call site — the shared code is what knows *when* the mode is
    /// settled, and the frontend is what decides whether that is news.
    fn permission_mode(&self, mode: PermissionMode) {
        if let Self::Stderr = self {
            eprintln!("[permission: {mode}]");
        }
    }
}

/// The stderr line for one finding.
///
/// Split out from [`Notices::notice`] so the format itself is testable: this
/// text is what a headless operator reads, and it is the only part of this
/// module a user ever sees.
fn stderr_line(tag: &str, message: &str) -> String {
    format!("[{tag}] {message}")
}

/// The interactive frontend's channel into the agent.
///
/// Both halves are part of one decision — "there is a UI to talk to" — so they
/// travel together: a `ui_tx` with no responder would leave approval prompts
/// and `ask_user` unanswered, and a responder with no channel would never see
/// the prompts it is meant to answer.
pub struct UiWiring {
    pub tx: UnboundedSender<AgentUpdate>,
    pub responder: UiResponder,
}

/// Resolve (or start) this run's session, and take its lock.
///
/// Both frontends do this as their first act, and the steps are not separable:
/// the row has to exist before the lock can name it, the lock has to be held
/// before anything writes, and the touch is what makes `--resume-last` find
/// this session next time. Returns the id and the *held* lock; releasing it is
/// the caller's, because when a run is over is the one thing the two frontends
/// disagree about.
pub async fn open_session(
    args: &CliArgs,
    tact_path: &TactPath,
    session_store: &DynSessionStore,
    lock_registry: &SessionLockRegistry,
) -> anyhow::Result<(String, Arc<SessionLockGuard>)> {
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
    Ok((session_id, session_lock))
}

/// Build the main agent: everything both frontends do identically.
///
/// `work_dir` is the workspace root the session is scoped to — a parameter
/// rather than derived from `tact_path` because the TUI already holds it and
/// passes it to the task manager before this runs.
///
/// The two frontends call this with the same arguments bar the last two, which
/// are the only things that genuinely differed between them.
///
/// The returned agent has not had `ensure_session` called on it: headless must
/// run it before the first turn, the TUI lets `agent_loop` do it. That is the
/// one step where the frontends' turn models differ, and it stays with them.
pub async fn bootstrap_session(
    tact_path: &TactPath,
    work_dir: PathBuf,
    skill_registry: SharedSkillRegistry,
    session_id: String,
    session_store: DynSessionStore,
    ui: Option<UiWiring>,
    notices: Notices,
) -> anyhow::Result<Agent> {
    let client = get_llm_client().await?;
    let mode = permission_mode_from_config();
    let settings = PermissionSettings::load(tact_path);
    let permission_manager = PermissionManager::try_new_with_settings(mode, settings)?;
    notices.permission_mode(mode);

    let db_path = tact_path.session_db_path();
    let task_manager = SharedTaskManager::new(TaskManager::new(&db_path).await?);
    let background_manager = SharedBackgroundManager::new(BackgroundManager::new(&db_path).await?);
    let teammate_manager = SharedTeammateManager::new(TeammateManager::new(&db_path).await?);
    let worktree_manager =
        SharedWorktreeManager::new(WorktreeManager::new(&db_path, work_dir.clone()).await?);
    let subagent_manager = SharedSubagentManager::new(SubagentManager::new(&db_path).await?);
    // Memory is user-global (`~/.tact/memory`) so it persists across projects.
    // Project-local `.tact/memory` is only the fallback when `$HOME` is unset.
    let memory_manager = Arc::new(std::sync::Mutex::new(memory_manager(
        TactPath::home_memory_dir().unwrap_or_else(|| tact_path.memory_dir()),
    )?));
    let (mcp_router, mcp_report) = load_mcp_router_with_report().await?;
    // MCP problems are collected, never fatal (one broken server must not stop
    // startup), so they have to be surfaced here — otherwise a typo'd command
    // or an ignored override would be silent. A clean load emits nothing.
    for line in mcp_report.notice_lines() {
        notices.notice("mcp", &line);
    }

    let mut tools = toolset_with_memory(tact::config::settings().agent.memory_enabled);
    // Annotate `spawn_subagent` with the current subagent skill-card catalog
    // so the main agent can discover valid `skill:` names.
    tact::tool::annotate_spawn_subagent_skill_catalog(&mut tools);
    // Opt-in sandbox, resolved once: it is constant for the session lifetime, so
    // the bash description below can state the sandbox semantics truthfully. A
    // switch that cannot be honoured degrades to unsandboxed and is announced.
    let (sandbox, sandbox_degraded) =
        tact::sandbox::resolve(tact::config::settings().tools.sandbox, &work_dir);
    if let Some(degraded) = &sandbox_degraded {
        notices.notice("sandbox", &degraded.reason);
    }
    if sandbox.is_some() {
        tools.set_tool_description("bash", tact::tool::SANDBOXED_BASH_DESCRIPTION);
    }

    let (ui_tx, ui_responder) = match &ui {
        Some(wiring) => (Some(wiring.tx.clone()), wiring.responder.clone()),
        // An empty responder is the honest value for "nobody can answer": a
        // tool that asks fails cleanly instead of hanging on a prompt that has
        // no one behind it.
        None => (None, UiResponder::new()),
    };
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
        ui_tx,
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
        client,
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
    )
    .with_session(session_id, session_store)
    .with_provider_kind(provider_kind);
    if let Some(wiring) = ui {
        agent = agent.with_ui_channel(wiring.tx);
    }
    // RTK filter is opt-in — `with_post_tool` no-ops unless the
    // `tools.rtk_filter` setting is enabled.
    agent = agent.with_post_tool(tact::hook::rtk_filter::create_rtk_post_tool_hook());

    // Command hooks (SessionStart / UserPromptSubmit / PreToolUse /
    // PostToolUse / …) from installed plugins, `~/.tact/hooks.json` and
    // `.tact/hooks.json`. A hook whose definition has not been reviewed is not
    // registered, and the report names it here — a repository that ships hooks
    // must not be able to run them silently.
    let (hooked, hook_report) =
        tact::plugin::apply_plugin_hooks_with_report(agent, tact_path.workdir())?;
    agent = hooked;
    for line in hook_report.notice_lines() {
        notices.notice("hooks", &line);
    }
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc::unbounded_channel;

    /// The UI path carries the finding and drops the tag.
    ///
    /// The tag exists so a bare stderr line says which startup step produced
    /// it; a TUI row has its own place on screen and reads on its own.
    #[tokio::test]
    async fn ui_notices_arrive_as_an_info_row_without_the_tag() {
        let (tx, mut rx) = unbounded_channel();
        Notices::Ui(tx).notice("mcp", "server demo did not answer");

        let update = rx.try_recv().expect("a notice");
        assert!(
            matches!(&update, AgentUpdate::Info(message) if message == "server demo did not answer"),
            "{update:?}"
        );
    }

    /// The permission mode is the one notice the two frontends do not share.
    ///
    /// Headless has nothing else that says it; the TUI's status bar already
    /// shows the mode, so a log row would be a second copy. This is the
    /// asymmetry the shared bootstrap exists to keep honest — it is asserted
    /// rather than assumed.
    #[tokio::test]
    async fn the_permission_mode_is_reported_only_where_nothing_else_shows_it() {
        let (tx, mut rx) = unbounded_channel();
        Notices::Ui(tx).permission_mode(PermissionMode::Plan);
        assert!(
            rx.try_recv().is_err(),
            "the UI path must not repeat what its status bar already shows"
        );
    }

    /// The stderr format a headless operator reads.
    #[test]
    fn stderr_lines_are_tagged_by_startup_step() {
        assert_eq!(
            stderr_line("mcp", "server demo did not answer"),
            "[mcp] server demo did not answer"
        );
    }
}
