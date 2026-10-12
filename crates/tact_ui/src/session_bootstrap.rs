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
//! - [`UiWiring`] — the TUI registers the in-process `RuntimeEvent` channel and
//!   hands the agent the `UiResponder` its prompts are answered through;
//!   headless has neither.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tact::TrajectoryService;
use tact_extensions::{
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

use crate::{
    permission::{permission_mode_from_config, serving_permission},
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
    Ui(tact::EventTransport),
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
                let _ = tx.publish(tact_protocol::RuntimeEvent::Notification {
                    level: "info".into(),
                    content: message.to_string(),
                });
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

/// The interactive frontend's protocol event and interaction services.
pub struct UiWiring {
    /// The in-process prompt registry the *tool* prompts are answered through.
    /// It stays beside the broker rather than behind it: the tool and permission
    /// prompt path is not the Kernel's interaction surface, and it keeps its own
    /// responder by design (`crates/tact_extensions/src/ui_responder.rs`), so the
    /// driver answers whichever of the two knows a response's request id.
    pub responder: UiResponder,
    pub runtime_events: tact::EventTransport,
    /// The session's Kernel interaction surface.
    ///
    /// The frontend creates it and passes it in (rather than the bootstrap
    /// creating one and handing it back) for two reasons: the process that owns
    /// a UI is the thing that can answer through it, and a broker the caller
    /// holds keeps `bootstrap_session`'s signature — and every existing
    /// caller's — unchanged. A context that installs none
    /// ([`build_serving_context`] with `None`, i.e. headless) keeps the no-op
    /// service, so an interaction request there fails closed.
    pub interaction: tact::InteractionBroker,
}

fn runtime_event_transport(ui: Option<&UiWiring>) -> tact::EventTransport {
    ui.map(|wiring| wiring.runtime_events.clone())
        .unwrap_or_else(|| tact::EventTransport::new(256))
}

/// Starts the durable trajectory recorder and returns the service that writes
/// to it.
///
/// The returned handle is the *same* recorder the transport replays from, so
/// the serving context's `trajectory.*` capabilities read the facts the session
/// actually persisted. `None` means the durable recorder is unavailable (the
/// notice says so); the caller degrades rather than losing the session.
async fn start_trajectory_recorder(
    db_path: &Path,
    event_transport: &tact::EventTransport,
    redaction: tact_trajectory::TrajectoryRedactionConfig,
    notices: &Notices,
) -> Option<Arc<dyn TrajectoryService>> {
    match tact_trajectory::SqliteTrajectoryRecorder::open(db_path).await {
        Ok(recorder) => {
            let trajectory = std::sync::Arc::new(
                tact_trajectory::SqliteTrajectoryService::with_redaction(recorder, redaction),
            );
            // Live delivery and replay must read the same recorder, or a client
            // that reconnects from a sequence would see a different history
            // than the one that was persisted.
            event_transport.set_replay_source(trajectory.clone());
            let mut subscription = event_transport.subscribe();
            let writer = trajectory.clone();
            tokio::spawn(async move {
                // A lagged broadcast window must not end recording: the writer
                // skips the lost window and keeps persisting every later fact.
                while let Some(event) = subscription.recv_skipping_lag().await {
                    let _ = writer.append(None, None, event).await;
                }
            });
            Some(trajectory)
        }
        Err(error) => {
            notices.notice(
                "trajectory",
                &format!("SQLite trajectory recorder unavailable: {error}"),
            );
            None
        }
    }
}

/// Builds the session's serving [`tact::RuntimeContext`].
///
/// This is the one place the Kernel's service capabilities are registered with
/// real backing. Before it existed, `tact::services::register` had no caller, so
/// `storage.*`, `events.*`, `trajectory.*`, `permission.request` and
/// `interaction.request` were unreachable in the shipping binary even though
/// every piece of them existed.
///
/// - `router` carries the Kernel services (`tact::services::register`) and the
///   Session extension over the real session store.
/// - `events` is the same [`tact::EventTransport`] the agent publishes to, so a
///   capability-published event is delivered to the subscribers the session
///   already has (and recorded by the trajectory writer that consumes them).
/// - `trajectory` is the durable recorder when SQLite opened; without it the
///   in-process recorder keeps `trajectory.*` answering instead of claiming a
///   history that was never written.
/// - `permission` is the session's configured policy plus the host's own
///   control plane — see [`crate::permission::serving_permission`].
/// - `storage` is a second SQLite handle on the session database, so a plugin's
///   `plugins/<id>` namespace survives the process.
/// - `interaction` is the session's Kernel interaction surface when the host
///   has one. The interactive frontend supplies its broker, so
///   `interaction.request` is answered by the process that owns the UI; headless
///   passes `None` and keeps the no-op service, so the same call fails closed
///   rather than waiting on a prompt nobody can answer.
///
/// `permission.request` is answered through the *permission* service, not this
/// one, so installing a broker here does not cover it on its own: the session's
/// policy is built with the same broker as its responder (see
/// [`crate::permission::serving_permission`]). A context built with `None` gets
/// neither, which is the fail-closed default for both capabilities.
///
/// A failure at any step degrades to a notice — a serving context that is
/// missing one capability must not stop the session from starting.
pub(crate) async fn build_serving_context(
    db_path: &Path,
    session_store: &DynSessionStore,
    runtime_events: &tact::EventTransport,
    trajectory: Arc<dyn TrajectoryService>,
    permission: Arc<dyn tact::PermissionService>,
    interaction: Option<Arc<dyn tact::InteractionService>>,
    notices: &Notices,
) -> tact::RuntimeContext {
    let storage: Arc<dyn tact::StorageService> =
        match tact::SqliteStorageService::open(db_path).await {
            Ok(storage) => Arc::new(storage),
            Err(error) => {
                notices.notice("storage", &format!("SQLite storage unavailable: {error}"));
                Arc::new(tact::StorageServiceImpl::default())
            }
        };

    let mut services = tact::RuntimeServices::new(
        Arc::new(runtime_events.clone()),
        trajectory,
        permission,
        storage,
    );
    if let Some(interaction) = interaction {
        services = services.with_interaction(interaction);
    }
    let context = tact::RuntimeContext::with_services(tact::CapabilityRouter::new(), services);
    if let Err(error) = tact::services::register(context.router()) {
        notices.notice(
            "services",
            &format!("Kernel service capabilities unavailable: {error}"),
        );
    }
    if let Err(error) =
        tact_extensions::extensions::session::SessionExtension::new(session_store.clone())
            .register(&context)
    {
        notices.notice(
            "session",
            &format!("session capabilities unavailable: {error}"),
        );
    }
    context
}

/// Registers the host's control-plane extensions on the session's serving
/// context: Chat's `chat.submit` (the conversational turn), the Agent's
/// `runs.*` (the run that turn executes through), and Workflow's
/// `workflow.run` (a sequence of such turns).
///
/// Both need the *shared* Agent, which the bootstrap does not have: it builds
/// the Agent and hands it to the frontend, and only the frontend wraps it in the
/// `Arc<Mutex<...>>` a run needs. So registration happens in the frontends —
/// through this one helper, so the two hosts cannot register a different set of
/// host capabilities. Chat registers first only because `chat.submit` invokes
/// `runs.start`: the router resolves capabilities at invocation time, so the
/// order is not a dependency, but reading them in the order a turn uses them
/// keeps the wiring legible.
pub(crate) fn register_host_extensions(
    serving: &tact::RuntimeContext,
    agent: Arc<tokio::sync::Mutex<Agent>>,
    cancel_flag: Arc<std::sync::atomic::AtomicBool>,
) -> Result<(), tact::KernelError> {
    tact_extensions::extensions::chat::ChatExtension::new(Arc::clone(&agent), serving.clone())
        .register(serving)?;
    tact_extensions::extensions::agent::AgentExtension::from_shared(agent, cancel_flag)
        .register(serving)?;
    // Workflow needs no Agent handle: it choreographs turns by invoking
    // `chat.submit` on the same router, so it is registered over a clone of the
    // serving context and is live the moment Chat is.
    tact_extensions::extensions::workflow::WorkflowExtension::new(Arc::new(
        tact_extensions::extensions::workflow::ChatTurnWorkflow::new(serving.clone()),
    ))
    .register(serving)
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
    // The session's Kernel interaction surface, when this host has a UI. The
    // same broker answers both Kernel prompt capabilities — `interaction.request`
    // through the interaction service itself, `permission.request` through the
    // policy's responder, because that capability asks the *permission* service.
    // `None` (headless) leaves the no-op interaction service and the
    // no-responder policy, which is the fail-closed default for both.
    let interaction = ui.as_ref().map(|wiring| wiring.interaction.clone());
    // The serving context's policy is built from the same settings, so a
    // capability reaching the router is decided exactly as the session's tools
    // would be. `try_new_with_settings` takes the settings by value, hence the
    // clone.
    let serving_policy = serving_permission(mode, settings.clone(), interaction.clone())?;
    let permission_manager = PermissionManager::try_new_with_settings(mode, settings)?;
    notices.permission_mode(mode);

    let db_path = tact_path.session_db_path();
    // Runtime events are the single source for replayable execution history.
    // Start the recorder before the agent can emit its first event so both UI
    // and headless runs persist the complete stream, including startup hooks.
    // The recorder redacts payloads with the session's effective policy, so a
    // secret a tool printed never lands in the trajectory table verbatim.
    let redaction = permission_manager.security_config().redaction.clone();
    let runtime_events = runtime_event_transport(ui.as_ref());
    let recorder = start_trajectory_recorder(&db_path, &runtime_events, redaction, &notices).await;
    // The session's one serving router: the Kernel's service capabilities and
    // the Session extension, over the real event transport, trajectory recorder,
    // permission policy and session database. Built here (not by a frontend) so
    // both frontends serve the same capabilities, and attached to the Agent
    // below so it lives exactly as long as the session does.
    let trajectory: Arc<dyn TrajectoryService> = match recorder {
        Some(recorder) => recorder,
        // No durable recorder: keep the trajectory-backed capabilities
        // answering from the Runtime's in-process recorder instead of failing
        // every `trajectory.read` because the database could not be opened.
        None => Arc::new(tact_trajectory::KernelTrajectoryRecorder::default()),
    };
    let serving = build_serving_context(
        &db_path,
        &session_store,
        &runtime_events,
        trajectory,
        serving_policy,
        interaction.map(|broker| Arc::new(broker) as Arc<dyn tact::InteractionService>),
        &notices,
    )
    .await;
    let task_manager = SharedTaskManager::new(TaskManager::new(&db_path).await?);
    let background_manager = SharedBackgroundManager::new(BackgroundManager::new(&db_path).await?);
    let teammate_manager = SharedTeammateManager::new(TeammateManager::new(&db_path).await?);
    let worktree_manager =
        SharedWorktreeManager::new(WorktreeManager::new(&db_path, work_dir.clone()).await?);
    let subagent_manager = SharedSubagentManager::new(SubagentManager::new(&db_path).await?);
    // Memory is scoped to the git **repository** (`~/.tact/projects/<slug>/memory`),
    // so a project fact stays inside the project that produced it. Worktrees of
    // one repository share a directory because the slug comes from the common
    // git dir, not the working directory.
    let memory_dir = tact_extensions::memory::memory_root(&work_dir);
    // The retired global `~/.tact/memory` is migrated in, once, when the
    // scoped directory is still empty. Never fatal: a failure here costs the
    // import, not the session.
    if let Some(legacy) = TactPath::home_memory_dir()
        && legacy != memory_dir
    {
        match tact_extensions::memory::migrate_legacy_memory(&legacy, &memory_dir) {
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
    let memory_manager = Arc::new(std::sync::Mutex::new(memory_manager(memory_dir)?));
    let (mcp_router, mcp_report) = load_mcp_router_with_report().await?;
    // MCP problems are collected, never fatal (one broken server must not stop
    // startup), so they have to be surfaced here — otherwise a typo'd command
    // or an ignored override would be silent. A clean load emits nothing.
    for line in mcp_report.notice_lines() {
        notices.notice("mcp", &line);
    }

    let mut tools = toolset_with_memory(tact_extensions::config::settings().agent.memory_enabled);
    // Annotate `spawn_subagent` with the current subagent skill-card catalog
    // so the main agent can discover valid `skill:` names.
    tact_extensions::tool::annotate_spawn_subagent_skill_catalog(&mut tools);
    // Opt-in sandbox, resolved once: it is constant for the session lifetime, so
    // the bash description below can state the sandbox semantics truthfully. A
    // switch that cannot be honoured degrades to unsandboxed and is announced.
    let (sandbox, sandbox_degraded) = tact_extensions::sandbox::resolve(
        tact_extensions::config::settings().tools.sandbox,
        &work_dir,
    );
    if let Some(degraded) = &sandbox_degraded {
        notices.notice("sandbox", &degraded.reason);
    }
    if sandbox.is_some() {
        tools.set_tool_description("bash", tact_extensions::tool::SANDBOXED_BASH_DESCRIPTION);
    }

    let ui_responder = match &ui {
        Some(wiring) => wiring.responder.clone(),
        // An empty responder is the honest value for "nobody can answer": a
        // tool that asks fails cleanly instead of hanging on a prompt that has
        // no one behind it.
        None => UiResponder::new(),
    };
    let tool_context = ToolContext {
        skill_registry: skill_registry.clone(),
        subagent_start_hooks: tact_extensions::plugin::plugin_subagent_start_hooks(
            tact_path.workdir(),
        )?,
        subagent_stop_hooks: tact_extensions::plugin::plugin_subagent_stop_hooks(
            tact_path.workdir(),
        )?,
        memory_manager,
        work_dir,
        task_manager,
        background_manager,
        teammate_manager,
        worktree_manager,
        subagent_manager,
        interactive: ui.is_some(),
        ui_tx: None,
        view_updates: tact_extensions::tool::ViewUpdateEmitter::default(),
        ui_responder,
        progress_reporter: tact_extensions::tool::ToolProgressReporter::default(),
        cancel_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        bash_timeout_secs: tact_extensions::config::settings().tools.bash_timeout_secs,
        bash_nice: tact_extensions::config::settings().tools.bash_nice,
        sandbox,
        sandbox_degraded,
        parent_step_id: None,
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
    // The official extension manifests register in the session's Kernel
    // registry, which lives on the serving context built above: extension
    // lifecycle is session state, so it is not carried by the Agent (an Agent
    // is one participant in the session). Registering is still a real startup
    // step — it refuses a manifest whose declared dependencies are not
    // registered yet — but no host reports extension health today, so nothing
    // reads the state back yet.
    tact_extensions::extensions::register_official_manifests(serving.plugins(), &agent)?;
    // Installed plugins that declare a runtime half are started here, next to
    // the plugin MCP load: their capabilities register on the serving router so
    // a hosted capability is reachable through the same permissioned path as
    // every Kernel service, and their manifests register in the same session
    // registry the official ones do. A failure is a notice, never fatal — one
    // broken package must not take the session down — so nothing here uses `?`.
    let mut hosted_plugins = match tact_extensions::consts::PluginHome::from_environment() {
        Some(home) => {
            let (specs, discovery_failures) =
                tact_extensions::plugin::discover_runtime_plugins(&home);
            for failure in &discovery_failures {
                notices.notice("plugin", failure);
            }
            tact_extensions::plugin::HostedPlugins::start(specs).await
        }
        None => tact_extensions::plugin::HostedPlugins::empty(),
    };
    hosted_plugins.register(&serving, serving.plugins()).await;
    for line in hosted_plugins.notice_lines() {
        notices.notice("plugin", line);
    }
    agent = agent.with_hosted_plugins(hosted_plugins);
    agent = agent.with_runtime_event_transport(runtime_events);
    // The serving context is held by the session's Agent: it is the only
    // reference to the Kernel service router and to the session's plugin
    // registry, so dropping it here would take `storage.*`, `events.*`,
    // `trajectory.*` and the Session capabilities with
    // it.
    agent = agent.with_serving_context(serving);
    // RTK filter is opt-in — `with_post_tool` no-ops unless the
    // `tools.rtk_filter` setting is enabled.
    agent = agent.with_post_tool(tact_extensions::hook::rtk_filter::create_rtk_post_tool_hook());

    // Command hooks (SessionStart / UserPromptSubmit / PreToolUse /
    // PostToolUse / …) from installed plugins, `~/.tact/hooks.json` and
    // `.tact/hooks.json`. A hook whose definition has not been reviewed is not
    // registered, and the report names it here — a repository that ships hooks
    // must not be able to run them silently.
    let (hooked, hook_report) =
        tact_extensions::plugin::apply_plugin_hooks_with_report(agent, tact_path.workdir())?;
    agent = hooked;
    for line in hook_report.notice_lines() {
        notices.notice("hooks", &line);
    }
    Ok(agent)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::Duration;

    use serde_json::json;
    use tact::{SqliteStorageService, StorageService};
    use tact_extensions::extensions::agent::AgentExecutor;

    /// A run executor stands in for the Agent so a registration test does not
    /// have to build one; the capability only has to *be* registered here.
    struct StubExecutor;

    #[async_trait::async_trait]
    impl AgentExecutor for StubExecutor {
        async fn run(
            &self,
            _context: tact::InvocationContext,
            _message: tact_llm::Message,
        ) -> Result<tact_protocol::RunId, tact::KernelError> {
            Ok(tact_protocol::RunId::from("stub-run"))
        }
    }

    /// The serving context is live with real backing, not merely declared.
    ///
    /// The whole point of routing the §4 capabilities through one Router is
    /// that the implementations behind it are the session's real ones, so this
    /// drives each kind of slot the way a plugin would: `storage.set` then
    /// `storage.get` for the caller's own `plugins/<id>` namespace (and a second
    /// SQLite handle sees the row, which an in-memory store would not),
    /// `events.publish` observed on the session's own transport, `sessions.read`
    /// answered by the real session store, and `trajectory.read` answered by the
    /// recorder that persists the session. The Router must also describe every
    /// §4 capability name: a capability that is not declared cannot be
    /// discovered, let alone invoked.
    #[tokio::test]
    async fn the_serving_context_serves_the_kernel_capabilities_over_real_backing() {
        let directory = tempfile::tempdir().expect("temp directory");
        let db_path = directory.path().join("session.db");
        let session_id = "serving-context-session";
        let store = tact_extensions::store::open_sqlite_session_store(&db_path)
            .await
            .expect("session store");
        store
            .ensure_session_row(session_id, &directory.path().display().to_string(), "")
            .await
            .expect("session row");

        let transport = tact::EventTransport::new(64);
        let mut subscription = transport.subscribe();
        let recorder = start_trajectory_recorder(
            &db_path,
            &transport,
            tact_trajectory::TrajectoryRedactionConfig::default(),
            &Notices::Stderr,
        )
        .await
        .expect("the durable recorder opens on a fresh database");
        let settings = PermissionSettings::load_from(&directory.path().join("settings.json"), None);
        let serving = build_serving_context(
            &db_path,
            &store,
            &transport,
            recorder,
            crate::permission::serving_permission(PermissionMode::Auto, settings, None)
                .expect("serving policy"),
            None,
            &Notices::Stderr,
        )
        .await;

        // The headless host registers the Agent extension on this same router,
        // which is what puts `runs.*` beside the Kernel's own services.
        tact_extensions::extensions::agent::AgentExtension::new(Arc::new(StubExecutor))
            .register(&serving)
            .expect("the Agent extension registers on the serving context");

        // Every §4 capability is declared — the Kernel's own eight, the
        // Session extension's two, and the host's two.
        for name in [
            "runs.start",
            "runs.cancel",
            "sessions.read",
            "sessions.write",
            "events.subscribe",
            "events.publish",
            "trajectory.read",
            "trajectory.append_plugin_event",
            "storage.get",
            "storage.set",
            "permission.request",
            "interaction.request",
        ] {
            assert!(
                serving.router().describe(name).is_some(),
                "{name} is not registered on the serving router"
            );
        }

        let plugin = tact_protocol::PluginId::from("demo.plugin");
        let namespace = format!("plugins/{plugin}");
        let invocation = || {
            serving.invocation(
                tact_protocol::RequestId::from(uuid::Uuid::new_v4().to_string()),
                plugin.clone(),
                "serving-context-test",
            )
        };
        let session_invocation =
            invocation().with_session_id(tact_protocol::SessionId::from(session_id));

        serving
            .router()
            .invoke(
                "storage.set",
                invocation(),
                json!({"namespace": namespace, "key": "greeting", "value": "hello"}),
            )
            .await
            .expect("storage.set is served");
        assert_eq!(
            serving
                .router()
                .invoke(
                    "storage.get",
                    invocation(),
                    json!({"namespace": namespace, "key": "greeting"}),
                )
                .await
                .expect("storage.get is served"),
            json!("hello")
        );
        // The value is in the database, not in a process-local map: a second
        // handle on the same file reads it back.
        assert_eq!(
            SqliteStorageService::open(&db_path)
                .await
                .expect("second handle")
                .get(&namespace, "greeting")
                .await
                .expect("read through the second handle"),
            Some(json!("hello"))
        );
        // The plugin's own namespace is its own; the Runtime's are not.
        let denied = serving
            .router()
            .invoke(
                "storage.get",
                invocation(),
                json!({"namespace": "runtime", "key": "greeting"}),
            )
            .await
            .expect_err("a plugin cannot read the Runtime's namespace");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );

        assert_eq!(
            serving
                .router()
                .invoke(
                    "sessions.read",
                    session_invocation,
                    json!({"session_id": session_id}),
                )
                .await
                .expect("sessions.read is served")["messages"],
            json!([])
        );

        let run_id = tact_protocol::RunId::from("serving-context-run");
        serving
            .router()
            .invoke(
                "events.publish",
                invocation(),
                serde_json::to_value(tact_protocol::RuntimeEvent::RunStarted {
                    run_id: run_id.clone(),
                })
                .unwrap(),
            )
            .await
            .expect("events.publish is served");
        let observed = tokio::time::timeout(Duration::from_secs(1), subscription.recv())
            .await
            .expect("the published event was not delivered to the session's transport")
            .expect("the session's transport is live");
        assert!(
            matches!(&observed, tact_protocol::RuntimeEvent::RunStarted { run_id: id } if id == &run_id),
            "{observed:?}"
        );

        // `trajectory.read` is answered by the durable recorder the session's
        // own writer persists into (a no-op slot would report "not available").
        let facts = tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                let facts = serving
                    .router()
                    .invoke(
                        "trajectory.read",
                        invocation(),
                        json!({"run_id": run_id.as_str()}),
                    )
                    .await
                    .expect("trajectory.read is served");
                if facts.as_array().is_some_and(|facts| !facts.is_empty()) {
                    break facts;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the trajectory recorder did not answer");
        assert_eq!(facts[0]["run_id"], json!(run_id.as_str()));
    }

    /// A production-shaped serving context: the bootstrap's own construction,
    /// with the broker the interactive host installs and a policy for `mode`
    /// that lets a plugin capability reach its handler.
    ///
    /// `Auto` mode is what makes most of these tests about *interaction* rather
    /// than about permission: `permission.request` is checked before it runs,
    /// and in `default` mode that check fails closed before the handler could
    /// ask anyone (which a test of its own pins).
    async fn serving_context_with_interaction(
        directory: &Path,
        mode: PermissionMode,
        interaction: Option<tact::InteractionBroker>,
    ) -> tact::RuntimeContext {
        let db_path = directory.join("session.db");
        let store = tact_extensions::store::open_sqlite_session_store(&db_path)
            .await
            .expect("session store");
        let transport = tact::EventTransport::new(16);
        let settings = PermissionSettings::load_from(&directory.join("settings.json"), None);
        let policy = crate::permission::serving_permission(mode, settings, interaction.clone())
            .expect("serving policy");
        build_serving_context(
            &db_path,
            &store,
            &transport,
            Arc::new(tact_trajectory::KernelTrajectoryRecorder::default()),
            policy,
            interaction.map(|broker| Arc::new(broker) as Arc<dyn tact::InteractionService>),
            &Notices::Stderr,
        )
        .await
    }

    fn plugin_invocation(
        serving: &tact::RuntimeContext,
        request_id: &str,
    ) -> tact::InvocationContext {
        serving.invocation(
            tact_protocol::RequestId::from(request_id.to_string()),
            tact_protocol::PluginId::from("demo.plugin"),
            "interaction-test",
        )
    }

    /// A plugin's `interaction.request`, on the context the bootstrap builds, is
    /// answered by a client of the session's broker — the production path for a
    /// hosted capability asking the user something.
    #[tokio::test]
    async fn the_serving_context_answers_an_interaction_request_through_the_broker() {
        let directory = tempfile::tempdir().expect("temp directory");
        let broker = tact::InteractionBroker::new(8);
        let mut client = broker.subscribe();
        let serving = serving_context_with_interaction(
            directory.path(),
            PermissionMode::Auto,
            Some(broker.clone()),
        )
        .await;

        let invoked = tokio::spawn({
            let serving = serving.clone();
            async move {
                serving
                    .router()
                    .invoke(
                        "interaction.request",
                        plugin_invocation(&serving, "interaction-caller"),
                        json!({
                            "type": "confirm",
                            "request_id": "interaction-1",
                            "prompt": "Continue?",
                        }),
                    )
                    .await
            }
        });

        // The client is the View: it is handed the request over the broker's own
        // stream, and the invoke returns exactly the response it sent back.
        let delivered = tokio::time::timeout(Duration::from_secs(1), client.recv())
            .await
            .expect("the request was not published to the broker's client")
            .expect("the client is subscribed");
        assert!(
            matches!(&delivered, tact_protocol::InteractionRequest::Confirm { request_id, .. }
                if request_id.as_str() == "interaction-1"),
            "{delivered:?}"
        );
        assert!(
            broker.respond(tact_protocol::InteractionResponse::Approved {
                request_id: tact_protocol::RequestId::from("interaction-1"),
            })
        );

        let response = tokio::time::timeout(Duration::from_secs(1), invoked)
            .await
            .expect("interaction.request did not return")
            .expect("invoke task")
            .expect("the invoked request is answered");
        let response: tact_protocol::InteractionResponse =
            serde_json::from_value(response).expect("a serialized response");
        assert!(matches!(
            response,
            tact_protocol::InteractionResponse::Approved { .. }
        ));
    }

    /// With nobody to ask, the request fails closed instead of hanging.
    ///
    /// Two shapes, one answer. A headless context installs no broker at all, so
    /// the no-op service refuses with `PermissionDenied` — the documented
    /// default. An interactive context *with* a broker but no client subscribed
    /// refuses in the broker: the request cannot even be published, so the call
    /// errors rather than waiting for a response nobody can send. Both are
    /// asserted under a timeout because the defect this pins is a hang, and a
    /// hang has no assertion to fail.
    #[tokio::test]
    async fn an_unanswerable_interaction_request_fails_closed_instead_of_hanging() {
        let headless_dir = tempfile::tempdir().expect("temp directory");
        let headless =
            serving_context_with_interaction(headless_dir.path(), PermissionMode::Auto, None).await;
        let denied = tokio::time::timeout(
            Duration::from_secs(1),
            headless.router().invoke(
                "interaction.request",
                plugin_invocation(&headless, "headless-caller"),
                json!({
                    "type": "confirm",
                    "request_id": "headless-1",
                    "prompt": "Continue?",
                }),
            ),
        )
        .await
        .expect("a headless interaction request must not hang")
        .expect_err("a context with no interaction service refuses");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );

        let interactive_dir = tempfile::tempdir().expect("temp directory");
        let broker = tact::InteractionBroker::new(8);
        let interactive = serving_context_with_interaction(
            interactive_dir.path(),
            PermissionMode::Auto,
            Some(broker),
        )
        .await;
        let unanswered = tokio::time::timeout(
            Duration::from_secs(1),
            interactive.router().invoke(
                "interaction.request",
                plugin_invocation(&interactive, "unheard-caller"),
                json!({
                    "type": "confirm",
                    "request_id": "unheard-1",
                    "prompt": "Anyone?",
                }),
            ),
        )
        .await
        .expect("an interaction request with no client must not hang")
        .expect_err("a broker with no subscriber refuses the request");
        // The broker says why: nothing is subscribed to hear it. The category is
        // not the point — that the call *returns* is.
        assert_eq!(
            unanswered.category(),
            tact_protocol::ErrorCategory::PluginUnavailable
        );
    }

    /// `permission.request` is answerable for the same reason: it asks the
    /// *permission* service, so the bootstrapped policy is built with the
    /// session's broker as its responder. Without that join a broker alone
    /// would leave this capability failing closed while `interaction.request`
    /// worked, which is exactly the asymmetry this test would catch.
    #[tokio::test]
    async fn the_serving_context_answers_a_permission_request_through_the_broker() {
        let directory = tempfile::tempdir().expect("temp directory");
        let broker = tact::InteractionBroker::new(8);
        let mut client = broker.subscribe();
        let serving = serving_context_with_interaction(
            directory.path(),
            PermissionMode::Auto,
            Some(broker.clone()),
        )
        .await;

        let invoked = tokio::spawn({
            let serving = serving.clone();
            async move {
                serving
                    .router()
                    .invoke(
                        "permission.request",
                        plugin_invocation(&serving, "permission-caller"),
                        json!({
                            "type": "permission",
                            "request_id": "permission-1",
                            "capability": "filesystem.write",
                            "reason": "Write generated output",
                        }),
                    )
                    .await
            }
        });

        let delivered = tokio::time::timeout(Duration::from_secs(1), client.recv())
            .await
            .expect("the permission prompt was not published to the broker's client")
            .expect("the client is subscribed");
        assert!(
            matches!(&delivered, tact_protocol::InteractionRequest::Permission { request_id, .. }
                if request_id.as_str() == "permission-1"),
            "{delivered:?}"
        );
        assert!(
            broker.respond(tact_protocol::InteractionResponse::Approved {
                request_id: tact_protocol::RequestId::from("permission-1"),
            })
        );

        let response = tokio::time::timeout(Duration::from_secs(1), invoked)
            .await
            .expect("permission.request did not return")
            .expect("invoke task")
            .expect("the invoked request is answered");
        let response: tact_protocol::InteractionResponse =
            serde_json::from_value(response).expect("a serialized response");
        assert!(matches!(
            response,
            tact_protocol::InteractionResponse::Approved { .. }
        ));
    }

    /// The one way this capability is *still* fail-closed in an interactive
    /// session, pinned so it is a decision rather than a surprise: the
    /// capability's own permission check runs **before** its handler, and in
    /// `default` (ask) mode that check denies a plugin-initiated capability —
    /// the responder answers the prompt the handler raises, it does not turn
    /// "ask" into "show a dialog". So a broker answers `permission.request`
    /// where the check allows the call (Auto, or an explicit allow rule) and the
    /// prompt is refused otherwise, without ever reaching the broker.
    #[tokio::test]
    async fn permission_request_is_refused_by_its_own_check_before_the_broker_is_asked() {
        let directory = tempfile::tempdir().expect("temp directory");
        let broker = tact::InteractionBroker::new(8);
        let serving = serving_context_with_interaction(
            directory.path(),
            PermissionMode::Default,
            Some(broker.clone()),
        )
        .await;

        let denied = tokio::time::timeout(
            Duration::from_secs(1),
            serving.router().invoke(
                "permission.request",
                plugin_invocation(&serving, "ask-mode-caller"),
                json!({
                    "type": "permission",
                    "request_id": "ask-mode-1",
                    "capability": "filesystem.write",
                    "reason": "Write generated output",
                }),
            ),
        )
        .await
        .expect("a denied permission.request must not hang")
        .expect_err("ask mode refuses a plugin-initiated capability");
        assert_eq!(
            denied.category(),
            tact_protocol::ErrorCategory::PermissionDenied
        );
        assert_eq!(
            broker.pending_count(),
            0,
            "the check refuses the call before its handler can raise a prompt"
        );
    }

    /// The UI path carries the finding and drops the tag.
    ///
    /// The tag exists so a bare stderr line says which startup step produced
    /// it; a TUI row has its own place on screen and reads on its own.
    #[tokio::test]
    async fn ui_notices_arrive_as_an_info_row_without_the_tag() {
        let transport = tact::EventTransport::new(4);
        let mut subscription = transport.subscribe();
        Notices::Ui(transport).notice("mcp", "server demo did not answer");

        let update = subscription.try_recv().expect("a notice");
        assert!(
            matches!(&update, tact_protocol::RuntimeEvent::Notification { level, content }
                if level == "info" && content == "server demo did not answer"),
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
        let transport = tact::EventTransport::new(4);
        let mut subscription = transport.subscribe();
        Notices::Ui(transport).permission_mode(PermissionMode::Plan);
        assert!(
            subscription.try_recv().is_err(),
            "the UI path must not repeat what its status bar already shows"
        );
    }

    #[tokio::test]
    async fn headless_bootstrap_keeps_a_protocol_event_transport() {
        let transport = runtime_event_transport(None);
        let mut subscription = transport.subscribe();
        transport
            .publish(tact_protocol::RuntimeEvent::Notification {
                level: "info".into(),
                content: "headless event".into(),
            })
            .unwrap();

        let event =
            tokio::time::timeout(std::time::Duration::from_millis(200), subscription.recv())
                .await
                .expect("headless event was not delivered")
                .unwrap();
        assert!(matches!(
            event,
            tact_protocol::RuntimeEvent::Notification { content, .. }
                if content == "headless event"
        ));
    }

    #[tokio::test]
    async fn headless_runtime_events_are_persisted_to_the_trajectory() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("session.db");
        let transport = runtime_event_transport(None);
        start_trajectory_recorder(
            &db_path,
            &transport,
            tact_trajectory::TrajectoryRedactionConfig::default(),
            &Notices::Stderr,
        )
        .await;
        let run_id = tact_protocol::RunId::from("headless-run");
        transport
            .publish(tact_protocol::RuntimeEvent::RunStarted {
                run_id: run_id.clone(),
            })
            .unwrap();

        let recorder = tact_trajectory::SqliteTrajectoryRecorder::open(&db_path)
            .await
            .unwrap();
        let events = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let events = recorder
                    .query(&tact_protocol::TrajectoryId::from(run_id.as_str()), 0)
                    .await
                    .unwrap();
                if !events.is_empty() {
                    break events;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("trajectory recorder did not persist the event");

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].run_id, Some(run_id.clone()));

        // A client that reconnects resumes through the transport, so the same
        // recorder the subscriber writes to has to answer replay.
        let replayed = transport
            .replay_from(&tact_protocol::TrajectoryId::from(run_id.as_str()), 0)
            .await
            .expect("the trajectory recorder is installed as the replay source");
        assert_eq!(replayed.len(), 1);
        assert_eq!(replayed[0].run_id, Some(run_id));
    }

    /// A secret a tool printed must not reach the durable trajectory verbatim.
    ///
    /// The trajectory table is a persistence sink like the transcript, so the
    /// session's redaction policy applies to event payloads before they are
    /// written. Pinning it here is what stops a later refactor from dropping
    /// the redaction on the way to SQLite.
    #[tokio::test]
    async fn persisted_trajectory_payloads_are_redacted() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("session.db");
        let transport = runtime_event_transport(None);
        start_trajectory_recorder(
            &db_path,
            &transport,
            tact_trajectory::TrajectoryRedactionConfig::default(),
            &Notices::Stderr,
        )
        .await;
        let run_id = tact_protocol::RunId::from("secret-run");
        transport
            .publish(tact_protocol::RuntimeEvent::Text {
                run_id: Some(run_id.clone()),
                role: "tool".into(),
                content: "api sk-abcdefghijklmnopqrstuvwx done".into(),
            })
            .unwrap();

        let recorder = tact_trajectory::SqliteTrajectoryRecorder::open(&db_path)
            .await
            .unwrap();
        let events = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let events = recorder
                    .query(&tact_protocol::TrajectoryId::from(run_id.as_str()), 0)
                    .await
                    .unwrap();
                if !events.is_empty() {
                    break events;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("trajectory recorder did not persist the event");

        let stored = serde_json::to_string(&events[0].payload).unwrap();
        assert!(
            !stored.contains("sk-abcdefghijklmnopqrstuvwx"),
            "secret reached the trajectory table: {stored}"
        );
        assert!(stored.contains("[redacted:api-key]"), "{stored}");
    }

    /// A lagged subscriber must not stop the durable writer for the session.
    ///
    /// The recorder is the only durable writer, and it consumes a bounded
    /// broadcast channel. Before this fix, one `RecvError::Lagged` ended its
    /// loop — every later fact was silently lost. Here the writer falls behind
    /// on purpose and every event published after it catches up is still
    /// persisted.
    #[tokio::test]
    async fn trajectory_writer_survives_a_lagged_subscription() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("session.db");
        // Capacity 1 keeps the test small while still forcing a lag.
        let transport = tact::EventTransport::new(1);
        start_trajectory_recorder(
            &db_path,
            &transport,
            tact_trajectory::TrajectoryRedactionConfig::default(),
            &Notices::Stderr,
        )
        .await;
        let run_id = tact_protocol::RunId::from("lagged-run");

        // Publish more than the capacity without yielding, so the subscriber
        // is behind when it next polls and the bus reports a lag.
        for index in 0..64 {
            transport
                .publish(tact_protocol::RuntimeEvent::Text {
                    run_id: Some(run_id.clone()),
                    role: "assistant".into(),
                    content: format!("{run_id}-{index}"),
                })
                .unwrap();
        }
        // The last event is published after a yield so it falls outside any
        // skipped window the writer may have consumed.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        transport
            .publish(tact_protocol::RuntimeEvent::Text {
                run_id: Some(run_id.clone()),
                role: "assistant".into(),
                content: format!("{run_id}-sentinel"),
            })
            .unwrap();

        let recorder = tact_trajectory::SqliteTrajectoryRecorder::open(&db_path)
            .await
            .unwrap();
        let saw_sentinel = tokio::time::timeout(std::time::Duration::from_secs(1), async {
            loop {
                let events = recorder
                    .query(&tact_protocol::TrajectoryId::from(run_id.as_str()), 0)
                    .await
                    .unwrap();
                if events.iter().any(|event| {
                    serde_json::to_string(&event.payload)
                        .is_ok_and(|text| text.contains("lagged-run-sentinel"))
                }) {
                    break true;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("a lagged subscriber must keep the trajectory writer alive");
        assert!(saw_sentinel);
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
        let store = tact_extensions::store::open_sqlite_session_store(&tact_path.session_db_path())
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
