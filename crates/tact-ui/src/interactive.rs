use std::sync::Arc;

use tact::{Agent, config::CliArgs, consts::TactPath, store::DynSessionStore};
use tact_protocol::{AccountUpdate, AgentErrorKind, AgentUpdate};

use crate::{
    account,
    driver::run_command_loop_with_account,
    session_bootstrap::{Notices, UiWiring, bootstrap_session, open_session},
    session_lock::{SessionLockGuard, SessionLockRegistry},
};

pub async fn run_interactive(
    args: CliArgs,
    tact_path: TactPath,
    session_store: DynSessionStore,
    lock_registry: Arc<SessionLockRegistry>,
) -> anyhow::Result<()> {
    let (session_id, session_lock) =
        open_session(&args, &tact_path, &session_store, lock_registry.as_ref()).await?;

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
    let runtime_events = tact::kernel::EventTransport::new(256);
    // One broker is shared by the agent (registration/waiter) and the TUI
    // (snapshot/reconcile). This remains an in-process transport: no protocol
    // enum or channel type is embedded in `tact_protocol`.
    let ui_responder = tact::ui_responder::UiResponder::new();
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
    let language = tact::config::settings().ui.language.clone();
    let model_context_window = tact::config::settings().agent.model_context_window;
    let model_name = tact::config::settings().agent.model.clone();
    let model_max_tokens = tact::config::settings().agent.max_tokens;
    let model_thinking_budget = tact::config::settings().agent.thinking_budget;
    let account_enabled = account::is_supported();
    let tui_ui_responder = ui_responder.clone();
    let tui_runtime_events = runtime_events.clone();
    let mut tui_handle = tokio::spawn(Box::pin(async move {
        let account_rx = if account_enabled {
            Some(account_rx)
        } else {
            None
        };
        tui::run_tui(tui::TuiConfig {
            agent_rx,
            runtime_events: tui_runtime_events,
            account_rx,
            plugin_rx,
            plugin_tx,
            user_cmd_tx,
            work_dir: tui_work_dir,
            input_history_entries: input_history,
            session_id,
            session_store,
            pending_ui: tui_ui_responder,
            history_save_tx,
            theme,
            language,
            ui_config_path: tact::config::settings().config_path.clone(),
            hook_output: tact::config::settings().ui.hook_output,
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
    //
    // Raced against the TUI, because this is the slowest thing that happens
    // after the UI appears: connecting MCP servers is a handshake per server,
    // and a remote one can spend seconds on OAuth discovery before it fails.
    // Awaiting it unconditionally made `q` look broken — the TUI printed its
    // bye line and restored the terminal immediately, and then the process sat
    // there until the last server finished connecting, with nothing on screen
    // to say why. A user who quits during startup has nothing left to drive, so
    // the build is dropped instead of waited out. That is safe: every MCP
    // transport kills its child on drop, and the session simply ends with no
    // agent, which is also why the session-id / stats lines below are skipped
    // on this path — there is no agent to summarise.
    let mut build = Box::pin(build_agent_for_interactive(
        tact_path,
        agent_tx.clone(),
        agent_skill_registry,
        agent_session_id,
        agent_session_store,
        work_dir,
        ui_responder,
        runtime_events,
    ));
    let driver = tokio::select! {
        // `biased`, so a build that is *already* finished is always used: the
        // default random choice would occasionally drop a just-built agent
        // because the TUI happened to become ready in the same poll, and the
        // stats below would go missing for no reason the user could see.
        biased;
        built = &mut build => match built {
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
        },
        tui = &mut tui_handle => {
            // Quit while the agent was still being built. `run_tui` has already
            // restored the terminal and printed its goodbye, so the only thing
            // left to do is to stop waiting for a build nobody will use.
            tui??;
            return Ok(());
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

/// Run the deferred, fallible startup steps needed to construct the main agent.
///
/// Invoked after the TUI is already visible; any error is surfaced in the TUI
/// rather than propagated through the raw-mode boundary (see the caller). The
/// steps themselves are shared with headless — see
/// [`crate::session_bootstrap`]; this only says where their notices go and
/// which channel the agent may talk back on.
async fn build_agent_for_interactive(
    tact_path: TactPath,
    agent_tx: tokio::sync::mpsc::UnboundedSender<AgentUpdate>,
    skill_registry: tact::skill::SharedSkillRegistry,
    session_id: String,
    session_store: DynSessionStore,
    work_dir: std::path::PathBuf,
    ui_responder: tact::ui_responder::UiResponder,
    runtime_events: tact::kernel::EventTransport,
) -> anyhow::Result<Agent> {
    // One clone for the notices: the wiring below takes the original channel.
    let notices = Notices::Ui(agent_tx.clone());
    bootstrap_session(
        &tact_path,
        work_dir,
        skill_registry,
        session_id,
        session_store,
        Some(UiWiring {
            tx: agent_tx,
            responder: ui_responder,
            runtime_events,
        }),
        notices,
    )
    .await
}
