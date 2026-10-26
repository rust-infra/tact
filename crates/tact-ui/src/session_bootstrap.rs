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

use anyhow::Context;

use tact::{
    Agent,
    AgentSystemPrompt,
    config::CliArgs,
    consts::TactPath,
    // No domain manager types: each plugin opens its own, so the bootstrap never
    // names one. That is what makes it impossible to hand a tool a handle the
    // registry does not know about.
    goal::{GoalManager, SharedGoalManager},
    mcp::load_mcp_router_with_report,
    permission::{PermissionManager, PermissionMode, settings::PermissionSettings},
    skill::SharedSkillRegistry,
    store::DynSessionStore,
    tool::{ToolContext, toolset_with_memory},
    ui_responder::UiResponder,
};
use tact_llm::get_llm_client;
use tact_protocol::{AgentUpdate, UpdateSink};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    permission::permission_mode_of,
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

/// What a frontend can do with the events a session emits.
///
/// The distinction is not cosmetic: attaching to [`UiWiring::tx`] is a claim
/// that prompts will be answered, so a frontend that cannot answer (headless)
/// must not take that channel. It observes instead — it sees the same events,
/// but permission checks still fall through to the non-interactive path.
pub enum Frontend {
    /// A UI that can answer prompts and draw the events itself (the TUI).
    Interactive(UiWiring),
    /// A frontend that only watches: it receives a copy of every event and
    /// answers nothing. Used by headless, which has no one to ask.
    Observing(UnboundedSender<AgentUpdate>),
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

/// The event sink and prompt responder a frontend gets.
///
/// Split out from [`bootstrap_session`] so the one decision that keeps a
/// headless run from claiming it can answer prompts is testable without an LLM
/// client and a session database. Attaching to [`UiWiring::tx`] is that claim;
/// an observer must get a sink with no answering channel and an empty
/// responder, or every permission check stops taking the non-interactive
/// branch.
fn frontend_wiring(frontend: &Frontend) -> (UpdateSink, UiResponder) {
    match frontend {
        Frontend::Interactive(wiring) => (
            UpdateSink::for_ui(wiring.tx.clone()),
            wiring.responder.clone(),
        ),
        // An empty responder is the honest value for "nobody can answer": a
        // tool that asks fails cleanly instead of hanging on a prompt that has
        // no one behind it.
        Frontend::Observing(_) => (UpdateSink::default(), UiResponder::new()),
    }
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
    frontend: Frontend,
    notices: Notices,
) -> anyhow::Result<Agent> {
    // Read the configuration once and let every later step take it from here.
    // Six separate `settings()` calls in this function meant the bootstrap could
    // not be handed a snapshot at all — there was nowhere to put it — which is
    // the shape that kept `tui` bound to this crate's global.
    let config = tact::config::settings();
    let client = get_llm_client().await?;
    let mode = permission_mode_of(&config);
    let settings = PermissionSettings::load(tact_path);
    let permission_manager = PermissionManager::try_new_with_settings(mode, settings)?;
    notices.permission_mode(mode);

    let db_path = tact_path.session_db_path();
    // Each domain plugin opens its own store. This is deliberate: the plugin owns
    // its domain, so it owns the construction, and the composition root never holds
    // a handle it could hand to a tool behind the registry's back — which is what
    // made the plugins decorative before.
    let plugin_task = Box::new(tact::task::TaskPlugin::open(&db_path).await?);
    let goal_manager = SharedGoalManager::new(GoalManager::new(&db_path).await?);
    let plugin_background = Box::new(tact::background::BackgroundPlugin::open(&db_path).await?);
    let plugin_team = Box::new(tact::team::TeamPlugin::open(&db_path).await?);
    let plugin_worktree =
        Box::new(tact::worktree::WorktreePlugin::open(&db_path, &work_dir).await?);
    let plugin_subagent = Box::new(tact::subagent::SubagentPlugin::open(&db_path).await?);
    // Memory is scoped to the git **repository** (`~/.tact/projects/<slug>/memory`),
    // so a project fact stays inside the project that produced it. Worktrees of
    // one repository share a directory because the slug comes from the common
    // git dir, not the working directory.
    let memory_dir = tact::memory::memory_root(&work_dir);
    // The retired global `~/.tact/memory` is migrated in, once, when the
    // scoped directory is still empty. Never fatal: a failure here costs the
    // import, not the session.
    if let Some(legacy) = TactPath::home_memory_dir()
        && legacy != memory_dir
    {
        match tact::memory::migrate_legacy_memory(&legacy, &memory_dir) {
            Ok(copied) if copied > 0 => notices.notice(
                "memory",
                &format!(
                    "migrated {copied} memories from {} to {}",
                    legacy.display(),
                    memory_dir.display()
                ),
            ),
            Ok(_) => {}
            Err(error) => notices.notice("memory", &format!("memory migration skipped: {error}")),
        }
    }
    let plugin_memory = Box::new(tact::memory::MemoryPlugin::open(&memory_dir));
    let (mcp_router, mcp_report) = load_mcp_router_with_report().await?;
    // MCP problems are collected, never fatal (one broken server must not stop
    // startup), so they have to be surfaced here — otherwise a typo'd command
    // or an ignored override would be silent. A clean load emits nothing.
    for line in mcp_report.notice_lines() {
        notices.notice("mcp", &line);
    }

    let mut tools = toolset_with_memory(config.agent.memory_enabled);
    // Annotate `spawn_subagent` with the current subagent skill-card catalog
    // so the main agent can discover valid `skill:` names.
    tact::tool::annotate_spawn_subagent_skill_catalog(&mut tools);
    // Opt-in sandbox, resolved once: it is constant for the session lifetime, so
    // the bash description below can state the sandbox semantics truthfully. A
    // switch that cannot be honoured degrades to unsandboxed and is announced.
    let (sandbox, sandbox_degraded) = tact::sandbox::resolve(config.tools.sandbox, &work_dir);
    if let Some(degraded) = &sandbox_degraded {
        notices.notice("sandbox", &degraded.reason);
    }
    if sandbox.is_some() {
        tools.set_tool_description("bash", tact::tool::SANDBOXED_BASH_DESCRIPTION);
    }

    let (events, ui_responder) = frontend_wiring(&frontend);
    let tool_context = ToolContext {
        // The seven domain handles are deliberately NOT set here. Each is owned by
        // the plugin installed below, and `ToolContext::rebind_from` fills it from
        // the registry during Agent assembly. Supplying them here as well is what
        // made the plugins decorative: a host could omit one and every tool would
        // keep reading this copy.
        skill_registry: None,
        // Filled by the plugin registry during Agent assembly, like the seven
        // domain handles above.
        agent_factory: None,
        subagent_start_hooks: tact::plugin::plugin_subagent_start_hooks(tact_path.workdir())?,
        subagent_stop_hooks: tact::plugin::plugin_subagent_stop_hooks(tact_path.workdir())?,
        memory_manager: None,
        work_dir,
        task_manager: None,
        goal_manager,
        active_goal_id: None,
        background_manager: None,
        teammate_manager: None,
        worktree_manager: None,
        subagent_manager: None,
        events,
        ui_responder,
        progress_reporter: tact::tool::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        // Resolved once here from the snapshot the bootstrap read, so the tool
        // never reaches for the process configuration mid-call.
        vision: tact::tool::VisionPolicy::from_settings(&config.ui.vision_image),
        // The run's own settings snapshot: a subagent spawned from this context
        // inherits this session's model and budgets rather than the process
        // global's, which another session could have replaced.
        agent_settings: config.agent.clone(),
        bash_timeout_secs: config.tools.bash_timeout_secs,
        bash_nice: config.tools.bash_nice,
        sandbox,
        sandbox_degraded,
        session_id: None,
        session_store: None,
        permission_snapshot: None,
        is_subagent: false,
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
    // The loop's service is registered during assembly, so it has to exist
    // first. Its slot stays empty until the agent below installs itself. The
    // cancel flag is the same one the tools see: a cancel through the service and
    // a cancel through the UI must stop the same run.
    let agent_service = std::sync::Arc::new(tact::agent::service::RuntimeAgentService::pending(
        tool_context.cancel_flag.clone(),
    ));
    // Shares the agent service's slot, so a turn and a compaction serialise
    // against each other.
    let chat_service = std::sync::Arc::new(
        tact::agent::chat::RuntimeChatService::from_agent_service(&agent_service),
    );
    let mut agent = Agent::new_with_host(
        client,
        tool_context,
        tools,
        mcp_router,
        permission_manager,
        AgentSystemPrompt::Dynamic,
        // The store travels through the plugin bundle, not as a second direct
        // handoff: opening it is async and had to happen before the session id
        // was resolved, but from here on the registry is the only path.
        tact::host::HostExtras {
            storage: Some(tact::host::StorageOptions {
                store: session_store.clone(),
                path: db_path.clone(),
            }),
            agent_service: Some(agent_service.clone()),
            chat_service: Some(chat_service.clone()),
            background: Some(plugin_background),
            memory: Some(plugin_memory),
            // The skill registry is still shared with the TUI, which reloads it
            // through `/skill reload`; the plugin registers the same handle.
            skills: Some(Box::new(tact::skill::SkillsPlugin::new(
                skill_registry.clone(),
            ))),
            worktree: Some(plugin_worktree),
            subagent: Some(plugin_subagent),
            task: Some(plugin_task),
            team: Some(plugin_team),
            // The view this process draws. `Interactive` means a terminal is
            // attached, so the TUI plugin is the one; everything else observes.
            view: Some(match frontend {
                Frontend::Interactive(_) => {
                    Box::new(tui::plugin::TuiPlugin::new()) as Box<dyn tact_kernel::plugin::Plugin>
                }
                Frontend::Observing(_) => Box::new(crate::view_plugin::HeadlessPlugin::new())
                    as Box<dyn tact_kernel::plugin::Plugin>,
            }),
            ..tact::host::HostExtras::default()
        },
    )
    // The plugin was installed in the line above, so a failure here means the
    // bundle is broken rather than that the caller forgot something.
    .with_installed_session(session_id)
    .context("the storage plugin installed by this bootstrap must provide the store")?
    .with_provider_kind(provider_kind);
    agent = match frontend {
        Frontend::Interactive(wiring) => agent.with_ui_channel(wiring.tx),
        // Observation only: the agent keeps its empty responder, so every
        // permission check still takes the non-interactive path.
        Frontend::Observing(tx) => agent.with_event_observer(tx),
    };
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

    /// An observing frontend gets no way to answer a prompt.
    ///
    /// This is the guard that keeps a headless run from accepting the calls
    /// `PermissionManager::ask_user` denies when nobody can answer. Both halves
    /// matter: the sink must carry no answering channel, *and* the responder
    /// handed to the tool context must be an empty one.
    ///
    /// The observer's sender is deliberately not consumed here — it is attached
    /// to the agent afterwards by `with_event_observer`, which also syncs
    /// `tool_context.events`. What must be true at *this* stage is that nothing
    /// claims the answering channel.
    #[tokio::test]
    async fn an_observing_frontend_cannot_answer_prompts() {
        let (tx, _rx) = unbounded_channel();
        let (events, responder) = frontend_wiring(&Frontend::Observing(tx));

        assert!(!events.has_ui(), "an observer is not an answering frontend");
        assert!(events.ui().is_none());
        assert!(
            responder.snapshot().is_empty(),
            "an empty responder has no pending request"
        );
    }

    /// The interactive frontend keeps both, or the TUI could not prompt.
    #[tokio::test]
    async fn an_interactive_frontend_keeps_its_channel_and_responder() {
        let (tx, mut rx) = unbounded_channel();
        let responder = UiResponder::new();
        let wiring = UiWiring {
            tx: tx.clone(),
            responder: responder.clone(),
        };
        let (events, got_responder) = frontend_wiring(&Frontend::Interactive(wiring));

        assert!(events.has_ui(), "the TUI answers prompts");
        events.send(AgentUpdate::Info("progress".into()));
        assert!(
            matches!(rx.try_recv(), Ok(AgentUpdate::Info(text)) if text == "progress"),
            "the TUI receives the event"
        );

        // The responder is the one the frontend supplied, not a fresh empty
        // one — a copy would silently strand every pending prompt.
        assert_eq!(
            got_responder.snapshot().len(),
            responder.snapshot().len(),
            "the frontend's own responder is passed through"
        );
    }

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

    /// The store reaches the agent through the plugin bundle, once.
    ///
    /// `HostOptions::storage` existed before this bootstrap used it, so the
    /// production path and the tests disagreed about how a store is handed over
    /// — and the direct `with_session` call was the second path the architecture
    /// forbids. Reading the source is the only check that sees the shape of this
    /// function without standing up an LLM client and an MCP connect: the
    /// runtime behaviour is pinned separately, by the agent test that resolves an
    /// installed store.
    #[test]
    fn the_bootstrap_hands_the_store_over_through_the_plugin_bundle() {
        let source = include_str!("session_bootstrap.rs");
        let body = source
            .split_once("pub async fn bootstrap_session(")
            .expect("the shared bootstrap is declared here")
            .1;
        let body = body
            .split_once("\n#[cfg(test)]")
            .expect("and ends where the tests begin")
            .0;
        assert!(
            body.contains("tact::host::HostExtras"),
            "the bootstrap must install the store as plugin infrastructure"
        );
        assert!(
            body.contains("with_installed_session"),
            "and resolve it from the registry rather than taking a second copy"
        );
        assert!(
            !body.contains(".with_session("),
            "the direct handoff is what made HostOptions::storage test-only"
        );
    }
}

#[cfg(test)]
mod session_open_tests {
    use super::*;
    use clap::Parser;

    /// `--resume-last` had no test coverage before `open_session` existed: no
    /// test in the workspace built a `CliArgs`, so nothing pinned which session
    /// a run continues. It is the only decision in here, and getting it wrong
    /// is silent — a fresh id looks exactly like a resumed one from the outside
    /// until the user notices their history is gone.
    #[tokio::test]
    async fn open_session_resolves_the_id_the_args_ask_for() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let tact_path = TactPath::new(tmp.path());
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
            .await
            .expect("session store");
        let registry = SessionLockRegistry::new();
        let root_dir = tact_path.workdir().display().to_string();

        // A named id is taken as given.
        let args = cli_args(&["--session", "fixed"]);
        let (id, lock) = open_session(&args, &tact_path, &store, &registry)
            .await
            .expect("open");
        assert_eq!(id, "fixed");
        lock.release().await.expect("release");

        for (label, extra) in [
            ("with nothing to resume", &[][..]),
            ("resuming with no history", &["--resume-last"][..]),
        ] {
            // Nothing to continue from — either explicitly asked or by default:
            // a fresh id, and a real row behind it.
            let args = cli_args(extra);
            let (fresh, lock) = open_session(&args, &tact_path, &store, &registry)
                .await
                .expect("open");
            assert!(
                uuid::Uuid::parse_str(&fresh).is_ok(),
                "{label}: expected a new id, got {fresh}"
            );
            let listed = store
                .list_sessions(Some(&root_dir))
                .await
                .expect("list sessions");
            assert!(
                listed.iter().any(|s| s.id == fresh),
                "{label}: open_session must leave a row behind, got {listed:?}"
            );
            lock.release().await.expect("release");

            // ...and now there is history to resume from.
            let args = cli_args(&["--resume-last"]);
            let (resumed, lock) = open_session(&args, &tact_path, &store, &registry)
                .await
                .expect("open");
            assert_eq!(resumed, fresh, "{label}: --resume-last must find it");
            lock.release().await.expect("release");
        }
    }

    /// Top-level flags precede the subcommand: they are declared on `CliArgs`
    /// and not marked `global`. Pinned in `crates/tact` too; repeated here
    /// because this is the argv the test above constructs.
    fn cli_args(extra: &[&str]) -> CliArgs {
        let mut argv = vec!["tact-ui"];
        argv.extend_from_slice(extra);
        argv.extend_from_slice(&["headless", "prompt"]);
        CliArgs::parse_from(argv)
    }
}
