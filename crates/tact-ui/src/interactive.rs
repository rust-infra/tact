use std::sync::Arc;

use tact::{config::CliArgs, consts::TactPath, store::DynSessionStore};
use tact_host::{
    HostEvent, HostFrontendChannels, HostSession, HostSessionConfig, SessionLockRegistry,
    SessionSelection,
};
use tact_protocol::{AgentErrorKind, AgentUpdate};

use crate::account;

pub async fn run_interactive(
    args: CliArgs,
    tact_path: TactPath,
    session_store: DynSessionStore,
    lock_registry: Arc<SessionLockRegistry>,
) -> anyhow::Result<()> {
    let selection = if let Some(id) = &args.session {
        SessionSelection::Resume(id.clone())
    } else if args.resume_last {
        SessionSelection::ResumeLast
    } else {
        SessionSelection::New
    };
    let account_enabled = account::is_supported();
    let host_config = HostSessionConfig {
        tact_path: tact_path.clone(),
        work_dir: tact_path.workdir().to_path_buf(),
        session: selection,
        args,
        session_store: session_store.clone(),
        lock_registry,
        progress_capacity: 256,
        account_polling: account_enabled,
    };
    let (mut host, channels) = HostSession::start_with_frontend_channels(host_config).await?;

    let run_result = run_tui_host(tact_path, session_store, &mut host, channels).await;
    let close_result = host.close().await;

    match (run_result, close_result) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(anyhow::anyhow!("failed to close host session: {error:?}")),
        (Ok(()), Ok(_)) => Ok(()),
    }
}

async fn run_tui_host(
    tact_path: TactPath,
    session_store: DynSessionStore,
    host: &mut HostSession,
    channels: HostFrontendChannels,
) -> anyhow::Result<()> {
    let session_id = host.session_id().to_string();
    let input_history = session_store.load_input_history(&session_id).await?;
    let work_dir = tact_path.workdir().to_path_buf();
    let skill_registry = host.skill_registry();

    let (plugin_tx, plugin_request_rx) = tokio::sync::mpsc::unbounded_channel();
    let (plugin_event_tx, plugin_rx) = tokio::sync::mpsc::unbounded_channel();
    let _plugin_worker = match tact::consts::PluginHome::from_environment() {
        Some(plugin_home) => {
            tact::plugin::spawn_worker(plugin_home, plugin_request_rx, plugin_event_tx)
        }
        None => tact::plugin::spawn_unavailable_worker(plugin_request_rx, plugin_event_tx),
    };

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

    let startup_error_tx = channels.agent_tx.clone();
    let pending_ui = host.ui_responder();
    let mut tui_handle = tokio::spawn(tui::run_tui(tui::TuiConfig {
        agent_rx: channels.agent_rx,
        account_rx: channels.account_rx,
        plugin_rx,
        plugin_tx,
        user_cmd_tx: channels.command_tx,
        work_dir,
        input_history_entries: input_history,
        session_id,
        session_store,
        pending_ui,
        history_save_tx,
        theme: tact::config::settings().ui.theme.clone(),
        language: tact::config::settings().ui.language.clone(),
        ui_config_path: tact::config::settings().config_path.clone(),
        hook_output: tact::config::settings().ui.hook_output,
        model_context_window: tact::config::settings().agent.model_context_window,
        model_name: tact::config::settings().agent.model.clone(),
        model_max_tokens: tact::config::settings().agent.max_tokens,
        model_thinking_budget: tact::config::settings().agent.thinking_budget,
        permission_mode: tact::config::settings().permission_mode,
        skills_description: {
            let registry = tact::skill::lock_skills(&skill_registry);
            registry.describe_available()
        },
        skills_data: {
            let registry = tact::skill::lock_skills(&skill_registry);
            registry
                .skills()
                .values()
                .map(|doc| tui::SkillEntry {
                    name: doc.manifest.name.clone(),
                    description: doc.manifest.description.clone(),
                    body: doc.body.clone(),
                    dir: doc.dir.display().to_string(),
                    disabled: registry.is_disabled(&doc.manifest.name),
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
    }));

    let mut host_events_open = true;
    loop {
        tokio::select! {
            biased;
            tui = &mut tui_handle => {
                tui??;
                return Ok(());
            }
            event = host.next_event(), if host_events_open => {
                match event {
                    Some(HostEvent::StartupFailed(error)) => {
                        let _ = startup_error_tx.send(AgentUpdate::Error(AgentErrorKind::Other(
                            format!("startup failed: {error:?}"),
                        )));
                    }
                    Some(HostEvent::Disconnected) | None => {
                        host_events_open = false;
                    }
                    Some(HostEvent::Agent(_)) | Some(HostEvent::Account(_)) | Some(HostEvent::Ready) => {}
                }
            }
        }
    }
}
