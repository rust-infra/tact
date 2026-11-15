//! ACP (Agent Client Protocol) v2 frontend for Tact.
//!
//! T3 Code launches this binary as a local ACP agent and drives it over
//! newline-delimited JSON-RPC on stdin/stdout. There is deliberately no TUI
//! here: T3 renders the transcript from `session/update` notifications.
//!
//! The adapter owns one [`HostSession`] per ACP session. A per-session actor
//! pump translates [`AgentUpdate`] events into ACP updates and routes
//! `UserCommand`s back to the host. The actor is the only writer of the
//! per-session state (`SessionState`): the manager holds immutable session
//! handles and reaches state through actor commands.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

use agent_client_protocol::v2::{ConnectionTo, Responder, schema};
use agent_client_protocol::{Agent, Client, Error, Result as AcpResult, Stdio};
use clap::Parser;
use tact::config::CliArgs;
use tact::store::{ArchiveFilter, SessionListQuery, open_sqlite_session_store};
use tact::ui_responder::PendingUiRequestKind;
use tact_host::{
    HostEvent, HostSession, HostSessionConfig, SessionLockRegistry, SessionSelection, TactPath,
    UserCommand,
};
use tact_protocol::{
    AgentUpdate, StepResult, StepStatus, ThinkingChunk, ToolPresentationInfo, ToolVisualKind,
    UiRequestPurpose, UiResponse,
};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// Bounded progress channel for one host session. Tool output can be chatty;
/// 1024 matches the headless observer used by `tact-ui`.
const PROGRESS_CAPACITY: usize = 1024;
/// How long `session/close` waits for the host driver to drain.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);
/// The only session config option this adapter serves.
const MODEL_CONFIG_ID: &str = "model";
/// Property name inside the elicitation form schema.
const ELICITATION_FIELD: &str = "selections";

/// Run the ACP agent until stdin closes.
pub async fn run() -> anyhow::Result<()> {
    init_logging();

    // T3 passes its hidden `commandArgs` through `AcpAgentConfig`, so argv is
    // the only way `--config` reaches this frontend. A parse failure is
    // reported through the session handlers like any other config error.
    let (args, parse_error) = match CliArgs::try_parse() {
        Ok(args) => (args, None),
        Err(error) => (CliArgs::default(), Some(error.to_string())),
    };
    let config_error = parse_error.or_else(|| {
        tact::config::init_from_args(&args)
            .err()
            .map(|error| format!("{error:#}"))
    });

    let manager = SessionManager::new(config_error);

    let result = Agent
        .v2()
        .name("tact-acp")
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::InitializeRequest,
                            responder: Responder<schema::InitializeResponse>,
                            _connection: ConnectionTo<Client>| {
                    let form = request
                        .capabilities
                        .elicitation
                        .as_ref()
                        .is_some_and(|elicitation| elicitation.supports_form());
                    manager.set_elicitation_form(form);
                    responder.respond(
                        schema::InitializeResponse::new(
                            request.protocol_version,
                            schema::Implementation::new("tact-acp", env!("CARGO_PKG_VERSION")),
                        )
                        .capabilities(
                            schema::AgentCapabilities::new()
                                .session(schema::SessionCapabilities::new()),
                        ),
                    )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::NewSessionRequest,
                            responder: Responder<schema::NewSessionResponse>,
                            connection: ConnectionTo<Client>| {
                    // T3 always attaches its stdio bridge even when the agent
                    // advertises no `session.mcp`, so rejecting this request
                    // would make every thread fail to start. Tact has no
                    // per-session MCP injection point; accepting and ignoring
                    // the server is the deliberate compatibility exception.
                    if !request.mcp_servers.is_empty() {
                        debug!(
                            count = request.mcp_servers.len(),
                            "ignoring client MCP servers; Tact uses its own MCP configuration"
                        );
                    }
                    let cwd = request.cwd.0.clone();
                    match manager.new_session(cwd, connection).await {
                        Ok(response) => responder.respond(response),
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::ListSessionsRequest,
                            responder: Responder<schema::ListSessionsResponse>,
                            _connection: ConnectionTo<Client>| {
                    if request.cursor.is_some() {
                        return responder.respond_with_error(invalid_params(
                            "session/list pagination is not supported",
                        ));
                    }
                    let cwd = request.cwd.as_ref().map(|path| path.0.clone());
                    match manager.list_sessions(cwd).await {
                        Ok(sessions) => {
                            responder.respond(schema::ListSessionsResponse::new(sessions))
                        }
                        Err(error) => responder.respond_with_error(error),
                    }
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::ResumeSessionRequest,
                            responder: Responder<schema::ResumeSessionResponse>,
                            connection: ConnectionTo<Client>| {
                    let cwd = request.cwd.0.clone();
                    let session_id = request.session_id.clone();
                    let replay_from = request.replay_from.clone();
                    // A live session's options are an actor read. Spawning the
                    // whole resume keeps that round trip off the dispatch loop,
                    // where it would delay the next inbound request.
                    let resume_connection = connection.clone();
                    let resume_manager = manager.clone();
                    if connection
                        .spawn(async move {
                            match resume_manager
                                .resume_session(cwd, session_id, replay_from, resume_connection)
                                .await
                            {
                                Ok(response) => drop(responder.respond(response)),
                                Err(error) => drop(responder.respond_with_error(error)),
                            }
                            Ok(())
                        })
                        .is_err()
                    {
                        debug!("failed to spawn session/resume task");
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::SetSessionConfigOptionRequest,
                            responder: Responder<schema::SetSessionConfigOptionResponse>,
                            connection: ConnectionTo<Client>| {
                    let reply = match manager.set_config_option(
                        &request.session_id,
                        &request.config_id,
                        &request.value,
                    ) {
                        Ok(reply) => reply,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    // The actor answers without awaiting the host, but the
                    // reply still crosses a task boundary. Spawn so a busy
                    // dispatch loop cannot serialize every session behind it.
                    if connection
                        .spawn(async move {
                            match reply.await {
                                Ok(Ok(options)) => {
                                    drop(responder.respond(
                                        schema::SetSessionConfigOptionResponse::new(options),
                                    ))
                                }
                                Ok(Err(error)) => {
                                    drop(responder.respond_with_error(set_config_error(error)));
                                }
                                Err(_) => drop(
                                    responder.respond_with_error(internal("session actor stopped")),
                                ),
                            }
                            Ok(())
                        })
                        .is_err()
                    {
                        debug!("failed to spawn session/set_config_option task");
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::CloseSessionRequest,
                            responder: Responder<schema::CloseSessionResponse>,
                            _connection: ConnectionTo<Client>| {
                    manager.close_session(&request.session_id).await;
                    responder.respond(schema::CloseSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::PromptRequest,
                            responder: Responder<schema::PromptResponse>,
                            connection: ConnectionTo<Client>| {
                    let text = match prompt_text(&request.prompt) {
                        Ok(text) => text,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    let message_id = manager.next_message_id("user");
                    let session_id = request.session_id.clone();
                    if let Err(error) = manager.submit_prompt(&session_id, text) {
                        return responder.respond_with_error(error);
                    }
                    // A v2 prompt response means "inserted", not "finished".
                    responder.respond(schema::PromptResponse::new(message_id.clone()))?;
                    let _ = send_update(
                        &connection,
                        &session_id,
                        schema::SessionUpdate::UserMessage(
                            schema::UserMessage::new(message_id).content(request.prompt.clone()),
                        ),
                    );
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            {
                let manager = manager.clone();
                async move |notification: schema::CancelSessionNotification,
                            _connection: ConnectionTo<Client>| {
                    manager.cancel(&notification.session_id);
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await;

    manager.shutdown_all().await;
    result.map_err(|error| anyhow::anyhow!("ACP connection failed: {error}"))
}

fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .with_target(false)
        .try_init();
}

#[derive(Clone)]
struct SessionManager {
    inner: Arc<ManagerInner>,
}

struct ManagerInner {
    sessions: Mutex<HashMap<schema::SessionId, SessionHandle>>,
    lock_registry: Arc<SessionLockRegistry>,
    next_message: Arc<AtomicU64>,
    config_error: Option<String>,
    /// Connection-scoped, captured once in `initialize`. ACP has one
    /// connection per process, so this is not per session.
    elicitation_form: AtomicBool,
}

/// Address for one live session. Immutable after creation, so the manager can
/// clone it freely without owning state.
#[derive(Clone)]
struct SessionHandle {
    id: schema::SessionId,
    /// The cwd the session was opened with. Read back on live resume to reject
    /// a mismatched directory.
    cwd: PathBuf,
    commands: mpsc::UnboundedSender<SessionCommand>,
}

#[derive(Clone, Copy, Debug)]
struct ElicitationSupport {
    form: bool,
}

impl ManagerInner {
    fn elicitation(&self) -> ElicitationSupport {
        ElicitationSupport {
            form: self.elicitation_form.load(Ordering::Acquire),
        }
    }
}

impl SessionManager {
    fn new(config_error: Option<String>) -> Self {
        let lock_registry = SessionLockRegistry::new();
        lock_registry.spawn_exit_listener();
        Self {
            inner: Arc::new(ManagerInner {
                sessions: Mutex::new(HashMap::new()),
                lock_registry,
                next_message: Arc::new(AtomicU64::new(1)),
                config_error,
                elicitation_form: AtomicBool::new(false),
            }),
        }
    }

    fn set_elicitation_form(&self, form: bool) {
        self.inner.elicitation_form.store(form, Ordering::Release);
    }

    fn elicitation(&self) -> ElicitationSupport {
        self.inner.elicitation()
    }

    fn ensure_config(&self) -> Result<(), Error> {
        match &self.inner.config_error {
            Some(error) => Err(internal(format!(
                "Tact configuration could not be loaded: {error}"
            ))),
            None => Ok(()),
        }
    }

    fn next_message_id(&self, kind: &str) -> schema::MessageId {
        let value = self.inner.next_message.fetch_add(1, Ordering::Relaxed);
        schema::MessageId::new(format!("{kind}-{value}"))
    }

    /// Immutable lookup. Returns a full handle so callers never reconstruct
    /// one field by field.
    fn handle(&self, session_id: &schema::SessionId) -> Result<SessionHandle, Error> {
        self.live_handle(session_id)
            .ok_or_else(|| session_not_found(session_id))
    }

    fn live_handle(&self, session_id: &schema::SessionId) -> Option<SessionHandle> {
        self.inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .get(session_id)
            .cloned()
    }

    /// Read the initial model state from global config. `HostSession::start`
    /// returns before bootstrap finishes, so setup responses must advertise
    /// from config rather than from the built agent.
    fn initial_state(&self) -> Result<SessionState, Error> {
        let settings = tact::config::try_settings()
            .ok_or_else(|| internal("Tact configuration is not installed"))?;
        Ok(SessionState {
            model: ModelSelection::from_settings(&settings),
            phase: TurnPhase::Idle,
        })
    }

    async fn new_session(
        &self,
        cwd: PathBuf,
        connection: ConnectionTo<Client>,
    ) -> Result<schema::NewSessionResponse, Error> {
        self.ensure_config()?;
        let state = self.initial_state()?;
        let options = state.model.config_options();
        let session_id = self
            .start_host(
                cwd,
                connection,
                SessionSelection::New,
                state,
                self.elicitation(),
            )
            .await?;
        // A new ACP session starts idle. Sending an idle here would be a
        // second, spurious turn boundary for the client.
        Ok(schema::NewSessionResponse::new(session_id).config_options(options))
    }

    async fn resume_session(
        &self,
        cwd: PathBuf,
        session_id: schema::SessionId,
        replay_from: Option<schema::ReplayFrom>,
        connection: ConnectionTo<Client>,
    ) -> Result<schema::ResumeSessionResponse, Error> {
        self.ensure_config()?;
        let replay = match replay_from {
            None => false,
            Some(schema::ReplayFrom::Start(_)) => true,
            Some(schema::ReplayFrom::Other(other)) => {
                return Err(invalid_params(format!(
                    "unsupported replay cursor `{}`",
                    other.type_
                )));
            }
            Some(_) => {
                return Err(invalid_params("unsupported replay cursor"));
            }
        };

        if let Some(handle) = self.live_handle(&session_id) {
            if handle.cwd != cwd {
                return Err(invalid_params(
                    "session cwd does not match the live session",
                ));
            }
            let options = self.config_options_of(&handle).await?;
            if replay {
                send_update(
                    &connection,
                    &handle.id,
                    schema::SessionUpdate::ReplayComplete(schema::ReplayComplete::new()),
                )?;
            }
            send_update(&connection, &handle.id, idle(None))?;
            return Ok(schema::ResumeSessionResponse::new().config_options(options));
        }

        let state = self.initial_state()?;
        let options = state.model.config_options();
        let actual = self
            .start_host(
                cwd,
                connection.clone(),
                SessionSelection::Resume(session_id.to_string()),
                state,
                self.elicitation(),
            )
            .await?;
        if actual != session_id {
            return Err(internal(format!(
                "resumed session id changed from `{session_id}` to `{actual}`"
            )));
        }
        if replay {
            send_update(
                &connection,
                &session_id,
                schema::SessionUpdate::ReplayComplete(schema::ReplayComplete::new()),
            )?;
        }
        // Resume keeps its idle: T3 races it against a two second replay gap
        // and uses the idle to learn that the session settled.
        send_update(&connection, &session_id, idle(None))?;
        Ok(schema::ResumeSessionResponse::new().config_options(options))
    }

    /// The live-session options read. Only called from the spawned resume
    /// task, never from the dispatch loop.
    async fn config_options_of(
        &self,
        handle: &SessionHandle,
    ) -> Result<Vec<schema::SessionConfigOption>, Error> {
        let (reply, reply_rx) = oneshot::channel();
        handle
            .commands
            .send(SessionCommand::ConfigOptions { reply })
            .map_err(|_| internal("session driver is closed"))?;
        reply_rx
            .await
            .map_err(|_| internal("session actor stopped"))
    }

    async fn start_host(
        &self,
        cwd: PathBuf,
        connection: ConnectionTo<Client>,
        selection: SessionSelection,
        state: SessionState,
        elicitation: ElicitationSupport,
    ) -> Result<schema::SessionId, Error> {
        let tact_path = TactPath::new(cwd.clone());
        let store = open_sqlite_session_store(&tact_path.session_db_path())
            .await
            .map_err(|error| internal(format!("failed to open session store: {error:#}")))?;
        let config = HostSessionConfig {
            tact_path,
            work_dir: cwd.clone(),
            session: selection,
            // The host ignores `args`; configuration is read from the process
            // globals installed by `init_from_args`.
            args: CliArgs::default(),
            session_store: store,
            lock_registry: self.inner.lock_registry.clone(),
            progress_capacity: PROGRESS_CAPACITY,
            account_polling: false,
        };
        let host = HostSession::start(config)
            .await
            .map_err(|error| internal(format!("failed to start session: {error}")))?;
        let session_id = schema::SessionId::new(host.session_id().to_string());
        let (commands, command_rx) = mpsc::unbounded_channel();
        let handle = SessionHandle {
            id: session_id.clone(),
            cwd,
            commands: commands.clone(),
        };
        self.inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .insert(session_id.clone(), handle);
        spawn_actor(
            host,
            session_id.clone(),
            connection,
            commands,
            command_rx,
            self.inner.next_message.clone(),
            state,
            elicitation,
        );
        info!(%session_id, "ACP session started");
        Ok(session_id)
    }

    fn submit_prompt(&self, session_id: &schema::SessionId, text: String) -> Result<(), Error> {
        self.handle(session_id)?
            .commands
            .send(SessionCommand::User(UserCommand::SubmitTask(text)))
            .map_err(|_| internal("session driver is closed"))
    }

    fn set_config_option(
        &self,
        session_id: &schema::SessionId,
        config_id: &schema::SessionConfigId,
        value: &schema::SessionConfigOptionValue,
    ) -> Result<oneshot::Receiver<Result<Vec<schema::SessionConfigOption>, SetConfigFailure>>, Error>
    {
        if config_id.0.as_ref() != MODEL_CONFIG_ID {
            return Err(invalid_params(format!(
                "unknown config option `{config_id}`"
            )));
        }
        let model = match value {
            schema::SessionConfigOptionValue::Id { value } => value.0.to_string(),
            _ => {
                return Err(invalid_params(
                    "model config option requires a string id value",
                ));
            }
        };
        let handle = self.handle(session_id)?;
        let (reply, reply_rx) = oneshot::channel();
        handle
            .commands
            .send(SessionCommand::SetConfig { model, reply })
            .map_err(|_| internal("session driver is closed"))?;
        Ok(reply_rx)
    }

    fn cancel(&self, session_id: &schema::SessionId) {
        if let Ok(handle) = self.handle(session_id) {
            let _ = handle.commands.send(SessionCommand::Cancel);
        }
    }

    async fn close_session(&self, session_id: &schema::SessionId) {
        let handle = self
            .inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .remove(session_id);
        let Some(handle) = handle else {
            return;
        };
        let (reply_tx, reply_rx) = oneshot::channel();
        if handle
            .commands
            .send(SessionCommand::Close(reply_tx))
            .is_ok()
        {
            match tokio::time::timeout(CLOSE_TIMEOUT, reply_rx).await {
                Ok(Ok(Ok(()))) => {}
                Ok(Ok(Err(error))) => warn!(%session_id, %error, "session close failed"),
                Ok(Err(_)) => warn!(%session_id, "session close reply dropped"),
                Err(_) => warn!(%session_id, "session close timed out"),
            }
        }
    }

    async fn list_sessions(&self, cwd: Option<PathBuf>) -> Result<Vec<schema::SessionInfo>, Error> {
        self.ensure_config()?;
        let cwd = match cwd {
            Some(cwd) => cwd,
            None => std::env::current_dir()
                .map_err(|error| internal(format!("failed to resolve cwd: {error}")))?,
        };
        let tact_path = TactPath::new(cwd.clone());
        let store = open_sqlite_session_store(&tact_path.session_db_path())
            .await
            .map_err(|error| internal(format!("failed to open session store: {error:#}")))?;
        let summaries = store
            .list_sessions_filtered(SessionListQuery {
                root_dir: Some(tact_path.workdir().display().to_string()),
                archive: ArchiveFilter::Active,
                search: None,
            })
            .await
            .map_err(|error| internal(format!("failed to list sessions: {error:#}")))?;
        Ok(summaries
            .into_iter()
            .map(|summary| {
                schema::SessionInfo::new(summary.id, cwd.clone())
                    .title(summary.title)
                    .updated_at(summary.updated_at.to_rfc3339())
            })
            .collect())
    }

    async fn shutdown_all(&self) {
        let handles: Vec<SessionHandle> = self
            .inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .drain()
            .map(|(_, handle)| handle)
            .collect();
        for handle in handles {
            let (reply_tx, reply_rx) = oneshot::channel();
            if handle
                .commands
                .send(SessionCommand::Close(reply_tx))
                .is_ok()
            {
                let _ = tokio::time::timeout(CLOSE_TIMEOUT, reply_rx).await;
            }
        }
    }
}

/// The model state a session can render. One source of truth, with an explicit
/// in-flight overlay instead of a second boolean.
#[derive(Clone, Debug)]
struct ModelSelection {
    /// The last model the runtime confirmed via `AgentUpdate::ModelInfo`.
    applied: String,
    /// A value accepted from `session/set_config_option` that the runtime has
    /// not confirmed yet. Set while the host applies it, or while a turn keeps
    /// it queued.
    requested: Option<String>,
    /// The requested value handed to the host. Distinguishes "queued, not sent"
    /// from "sent, awaiting confirmation" so an unrelated error cannot cancel
    /// a change that has not been attempted.
    sent: Option<String>,
    /// Candidate list from global config. Never empty; `applied` and every
    /// requested value are members by construction.
    candidates: Vec<String>,
}

impl ModelSelection {
    /// Boundary parse: global config in, a valid selection out.
    fn from_settings(settings: &tact::config::ResolvedConfig) -> Self {
        let applied = if settings.agent.model.trim().is_empty() {
            settings.llm.model.clone()
        } else {
            settings.agent.model.clone()
        };
        let mut candidates = settings.llm.models.clone();
        // ACP requires the current value to be one of the option values.
        if !candidates.iter().any(|candidate| candidate == &applied) {
            candidates.insert(0, applied.clone());
        }
        Self {
            applied,
            requested: None,
            sent: None,
            candidates,
        }
    }

    /// What the client should show. The pending value wins so a mid-turn
    /// switch is visible in the `set_config_option` response.
    fn shown(&self) -> &str {
        self.requested.as_deref().unwrap_or(&self.applied)
    }

    fn accepts(&self, model: &str) -> bool {
        self.candidates.iter().any(|candidate| candidate == model)
    }

    /// The only place that builds ACP config options.
    fn config_options(&self) -> Vec<schema::SessionConfigOption> {
        let shown = self.shown();
        let mut values = Vec::with_capacity(self.candidates.len() + 1);
        // Defensive: a future writer must not be able to produce an option
        // whose current value is absent from its values.
        if !self.candidates.iter().any(|candidate| candidate == shown) {
            values.push(schema::SessionConfigSelectOption::new(
                shown.to_string(),
                shown.to_string(),
            ));
        }
        values.extend(
            self.candidates
                .iter()
                .map(|model| schema::SessionConfigSelectOption::new(model.clone(), model.clone())),
        );
        vec![
            schema::SessionConfigOption::select(
                MODEL_CONFIG_ID,
                "Model",
                shown.to_string(),
                values,
            )
            .category(schema::SessionConfigOptionCategory::Model),
        ]
    }

    fn begin_request(&mut self, model: String) -> bool {
        let changed = model != self.shown();
        self.requested = Some(model);
        changed
    }

    /// `AgentUpdate::ModelInfo` is the runtime truth. Returns true when the
    /// client needs a `config_option_update`: either the advertised value
    /// moved, or a change that was queued/sent was confirmed. The confirmation
    /// case emits after the idle of the turn that queued it, which is the
    /// notification T3 uses to settle a mid-turn switch.
    fn commit(&mut self, reported: String) -> bool {
        let advertised = self.shown().to_string();
        let applied_before = self.applied.clone();
        self.applied = reported.clone();
        let confirmed = self.requested.as_deref() == Some(reported.as_str());
        if confirmed {
            self.requested = None;
            self.sent = None;
        }
        advertised != self.shown() || (confirmed && applied_before != self.applied)
    }
}

#[derive(Debug)]
struct SessionState {
    model: ModelSelection,
    phase: TurnPhase,
}

/// The wire-facing foreground state. Replaces a bare bool so "running because
/// a prompt was accepted" and "running because a host run started" cannot
/// disagree.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TurnPhase {
    Idle,
    Running,
}

impl TurnPhase {
    fn is_running(self) -> bool {
        matches!(self, TurnPhase::Running)
    }
}

enum SessionCommand {
    /// Forward a host command (`SubmitTask`, `UiResponse`, `Cancel`, ...).
    User(UserCommand),
    /// Validate and queue a model change. The actor answers from its own state
    /// and never awaits the host on this path.
    SetConfig {
        model: String,
        reply: oneshot::Sender<Result<Vec<schema::SessionConfigOption>, SetConfigFailure>>,
    },
    /// Read the current option list. Pure state read, used by live resume.
    ConfigOptions {
        reply: oneshot::Sender<Vec<schema::SessionConfigOption>>,
    },
    /// Answer pending UI requests, withdraw the ACP dialogs, forward Cancel.
    Cancel,
    /// The spawned ACP request task finished, so its cancellation handle can
    /// be dropped.
    RequestSettled {
        request_id: u64,
    },
    Close(oneshot::Sender<Result<(), String>>),
}

/// Actor-side refusal, mapped to an ACP error at the handler boundary. Keeping
/// ACP error construction out of the actor keeps protocol policy in one place.
enum SetConfigFailure {
    UnknownModel(String),
}

fn set_config_error(error: SetConfigFailure) -> Error {
    match error {
        SetConfigFailure::UnknownModel(model) => invalid_params(format!(
            "model `{model}` is not in this session's candidate list"
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_actor(
    host: HostSession,
    session_id: schema::SessionId,
    connection: ConnectionTo<Client>,
    commands: mpsc::UnboundedSender<SessionCommand>,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    next_message: Arc<AtomicU64>,
    state: SessionState,
    elicitation: ElicitationSupport,
) {
    tokio::spawn(async move {
        let mut actor = SessionActor {
            host,
            session_id,
            connection,
            commands,
            next_message,
            state,
            elicitation,
            open_requests: HashMap::new(),
            published_requests: HashSet::new(),
            agent_message: None,
            thought: None,
            open_tools: HashSet::new(),
            turn_saw_text: false,
        };
        actor.reconcile_pending_requests();
        loop {
            tokio::select! {
                event = actor.host.next_event() => {
                    match event {
                        Some(event) => {
                            actor.handle_event(event);
                            actor.reconcile_pending_requests();
                        }
                        None => {
                            actor.finish_turn(
                                schema::StopReason::Error(schema::ErrorStopReason::new()),
                                None,
                            );
                            break;
                        }
                    }
                }
                command = command_rx.recv() => {
                    match command {
                        Some(SessionCommand::User(command)) => actor.forward_user(command),
                        Some(SessionCommand::SetConfig { model, reply }) => {
                            actor.set_config(model, reply);
                        }
                        Some(SessionCommand::ConfigOptions { reply }) => {
                            let _ = reply.send(actor.state.model.config_options());
                        }
                        Some(SessionCommand::Cancel) => actor.cancel_session(),
                        Some(SessionCommand::RequestSettled { request_id }) => {
                            actor.request_settled(request_id);
                        }
                        Some(SessionCommand::Close(reply)) => {
                            let result = actor
                                .host
                                .close()
                                .await
                                .map(|_| ())
                                .map_err(|error| error.to_string());
                            let _ = reply.send(result);
                            break;
                        }
                        None => {
                            let _ = actor.host.close().await;
                            break;
                        }
                    }
                }
            }
        }
    });
}

struct OpenMessage {
    id: schema::MessageId,
    text: String,
}

/// One Tact UI request, normalized across the `AgentUpdate` hint and the
/// `UiResponder` snapshot used for reconciliation.
struct UiRequest {
    request_id: u64,
    prompt: String,
    options: Vec<String>,
    purpose: UiRequestPurpose,
    kind: UiRequestKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum UiRequestKind {
    Single,
    Multi,
}

impl UiRequest {
    fn from_pending(pending: tact::ui_responder::PendingUiRequest) -> Self {
        let kind = match pending.kind {
            PendingUiRequestKind::Select => UiRequestKind::Single,
            PendingUiRequestKind::MultiSelect => UiRequestKind::Multi,
        };
        Self {
            request_id: pending.request_id,
            prompt: pending.prompt,
            options: pending.options,
            purpose: pending.purpose,
            kind,
        }
    }
}

struct SessionActor {
    host: HostSession,
    session_id: schema::SessionId,
    connection: ConnectionTo<Client>,
    commands: mpsc::UnboundedSender<SessionCommand>,
    next_message: Arc<AtomicU64>,
    state: SessionState,
    elicitation: ElicitationSupport,
    /// Cancellation handles for ACP requests the adapter published, keyed by
    /// Tact's request id. This is a withdrawal index, not a second pending
    /// request registry: `UiResponder` remains authoritative for answering.
    open_requests: HashMap<u64, agent_client_protocol::RequestCancellationHandle>,
    /// Request ids already published to the client, so a hint delivered after
    /// reconciliation does not open a second dialog.
    published_requests: HashSet<u64>,
    agent_message: Option<OpenMessage>,
    thought: Option<OpenMessage>,
    open_tools: HashSet<String>,
    turn_saw_text: bool,
}

impl SessionActor {
    fn handle_event(&mut self, event: HostEvent) {
        match event {
            HostEvent::Agent(update) => self.handle_update(*update),
            HostEvent::Disconnected => {
                warn!(%self.session_id, "host disconnected");
                self.finish_turn(
                    schema::StopReason::Error(schema::ErrorStopReason::new()),
                    None,
                );
            }
            HostEvent::StartupFailed(failure) => {
                warn!(%self.session_id, ?failure, "host startup failed");
                self.notice(
                    schema::NoticeSeverity::Error,
                    "Tact failed to start",
                    Some(format!("{failure:?}")),
                );
                self.finish_turn(
                    schema::StopReason::Error(schema::ErrorStopReason::new()),
                    None,
                );
            }
            HostEvent::Ready => debug!(%self.session_id, "host ready"),
            HostEvent::Account(_) => {}
        }
    }

    fn handle_update(&mut self, update: AgentUpdate) {
        match update {
            AgentUpdate::StreamChunk(text) => {
                self.turn_saw_text = true;
                let id = self.ensure_agent_message();
                if let Some(message) = self.agent_message.as_mut() {
                    message.text.push_str(&text);
                }
                self.send(schema::SessionUpdate::AgentMessageChunk(
                    schema::ContentChunk::new(text.into(), id),
                ));
            }
            AgentUpdate::MessageStarted { message_id } => {
                debug!(
                    session_id = %self.session_id,
                    provider_message_id = %message_id,
                    "model message started"
                );
                self.finalize_agent_message();
                self.agent_message = Some(OpenMessage {
                    id: self.next_id("agent"),
                    text: String::new(),
                });
            }
            AgentUpdate::MessageFinished { .. } => self.finalize_agent_message(),
            AgentUpdate::ThinkingChunk(ThinkingChunk::Started) => {
                self.finalize_thought();
                self.thought = Some(OpenMessage {
                    id: self.next_id("thought"),
                    text: String::new(),
                });
            }
            AgentUpdate::ThinkingChunk(ThinkingChunk::Delta(text)) => {
                let id = self.ensure_thought();
                if let Some(thought) = self.thought.as_mut() {
                    thought.text.push_str(&text);
                }
                self.send(schema::SessionUpdate::AgentThoughtChunk(
                    schema::ContentChunk::new(text.into(), id),
                ));
            }
            AgentUpdate::ThinkingChunk(ThinkingChunk::Finished) => self.finalize_thought(),
            AgentUpdate::StepQueued {
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                presentation,
                ..
            } => {
                self.open_tools.insert(tool_id.clone());
                self.send(schema::SessionUpdate::ToolCallUpdate(
                    self.tool_call_update(
                        &tool_id,
                        &tool_name,
                        &arg_summary,
                        &arg_full,
                        &presentation,
                        schema::ToolCallStatus::Pending,
                        None,
                    ),
                ));
            }
            AgentUpdate::StepStarted {
                tool_id,
                tool_name,
                arg_summary,
                arg_full,
                presentation,
                ..
            } => {
                self.open_tools.insert(tool_id.clone());
                self.send(schema::SessionUpdate::ToolCallUpdate(
                    self.tool_call_update(
                        &tool_id,
                        &tool_name,
                        &arg_summary,
                        &arg_full,
                        &presentation,
                        schema::ToolCallStatus::InProgress,
                        None,
                    ),
                ));
            }
            AgentUpdate::StepFinished {
                tool_id, result, ..
            } => {
                self.open_tools.remove(&tool_id);
                self.send_finished_step(&tool_id, &result);
            }
            AgentUpdate::StepFailed { tool_id, error, .. } => {
                self.open_tools.remove(&tool_id);
                self.send(schema::SessionUpdate::ToolCallUpdate(
                    schema::ToolCallUpdate::new(tool_id)
                        .status(schema::ToolCallStatus::Failed)
                        .content(vec![text_tool_content(error)]),
                ));
            }
            AgentUpdate::ToolProgress {
                tool_id, chunks, ..
            } => {
                let text: String = chunks
                    .iter()
                    .map(|chunk| chunk.text.as_str())
                    .collect::<Vec<_>>()
                    .join("");
                if !text.is_empty() {
                    self.send(schema::SessionUpdate::ToolCallContentChunk(
                        schema::ToolCallContentChunk::new(tool_id, text_tool_content(text)),
                    ));
                }
            }
            AgentUpdate::BackgroundTaskFinished {
                tool_id,
                success,
                message,
                output,
                ..
            } => {
                self.open_tools.remove(&tool_id);
                let text = if output.trim().is_empty() {
                    message
                } else {
                    output
                };
                self.send(schema::SessionUpdate::ToolCallUpdate(
                    schema::ToolCallUpdate::new(tool_id)
                        .status(if success {
                            schema::ToolCallStatus::Completed
                        } else {
                            schema::ToolCallStatus::Failed
                        })
                        .content(vec![text_tool_content(text)]),
                ));
            }
            AgentUpdate::SubagentFinished {
                tool_id,
                success,
                summary,
                ..
            } => {
                self.open_tools.remove(&tool_id);
                self.send(schema::SessionUpdate::ToolCallUpdate(
                    schema::ToolCallUpdate::new(tool_id)
                        .status(if success {
                            schema::ToolCallStatus::Completed
                        } else {
                            schema::ToolCallStatus::Failed
                        })
                        .content(vec![text_tool_content(summary)]),
                ));
            }
            AgentUpdate::TaskComplete(summary) => {
                self.finish_turn(schema::StopReason::EndTurn, Some(summary));
            }
            AgentUpdate::TaskCancelled => {
                self.finish_turn(schema::StopReason::Cancelled, None);
            }
            AgentUpdate::TaskIdle => {
                self.finish_turn(schema::StopReason::EndTurn, None);
            }
            AgentUpdate::Error(kind) => {
                self.notice(
                    schema::NoticeSeverity::Error,
                    "Tact turn failed",
                    Some(kind.to_string()),
                );
                if self.state.model.sent.is_some() {
                    // The runtime rejected the model handed to it. Withdraw the
                    // optimistic value so the client sees what is active.
                    self.state.model.requested = None;
                    self.state.model.sent = None;
                    self.send_options();
                }
                self.finish_turn(
                    schema::StopReason::Error(schema::ErrorStopReason::new()),
                    None,
                );
            }
            AgentUpdate::ModelInfo(params) => {
                if self.state.model.commit(params.model) {
                    self.send_options();
                }
            }
            AgentUpdate::RunStarted { .. } => {
                if !self.state.phase.is_running() {
                    self.state.phase = TurnPhase::Running;
                    self.send(running());
                }
            }
            AgentUpdate::RunFailed { .. } => {
                self.notice(schema::NoticeSeverity::Error, "Tact run failed", None);
            }
            AgentUpdate::RequestSelect {
                request_id,
                prompt,
                options,
                purpose,
                ..
            } => self.handle_ui_request(UiRequest {
                request_id,
                prompt,
                options,
                purpose,
                kind: UiRequestKind::Single,
            }),
            AgentUpdate::RequestMultiSelect {
                request_id,
                prompt,
                options,
                purpose,
                ..
            } => self.handle_ui_request(UiRequest {
                request_id,
                prompt,
                options,
                purpose,
                kind: UiRequestKind::Multi,
            }),
            AgentUpdate::Info(text) | AgentUpdate::MdInfo(text) => {
                self.notice(schema::NoticeSeverity::Info, "Tact", Some(text));
            }
            AgentUpdate::UiResponseRejected { request_id } => {
                debug!(%self.session_id, request_id, "UI response arrived after the request was answered");
            }
            AgentUpdate::TokenUsage(_)
            | AgentUpdate::TurnStats { .. }
            | AgentUpdate::RunFinished { .. }
            | AgentUpdate::StepAdded(_) => {}
            other => debug!(%self.session_id, ?other, "unmapped Tact update"),
        }
    }

    /// Reconcile against the authoritative pending-request snapshot. Covers a
    /// lost `AgentUpdate` hint without making the hint a second registry.
    fn reconcile_pending_requests(&mut self) {
        let pending = self.host.pending_requests();
        let live: HashSet<u64> = pending.iter().map(|request| request.request_id).collect();
        self.published_requests.retain(|id| live.contains(id));
        for request in pending {
            if self.published_requests.insert(request.request_id) {
                self.publish_ui_request(UiRequest::from_pending(request));
            }
        }
    }

    fn handle_ui_request(&mut self, request: UiRequest) {
        if !self.published_requests.insert(request.request_id) {
            return;
        }
        self.publish_ui_request(request);
    }

    /// One routing point for permission, question, and unsupported requests.
    fn publish_ui_request(&mut self, request: UiRequest) {
        let permission = matches!(
            request.purpose,
            UiRequestPurpose::Permission | UiRequestPurpose::PlanApproval
        );
        if permission && request.kind == UiRequestKind::Single {
            if request.options.is_empty() {
                self.respond_ui(declined(request.request_id, false));
                return;
            }
            if self.state.phase.is_running() {
                self.send(requires_action());
            }
            self.spawn_permission(request);
            return;
        }

        let question = matches!(
            request.purpose,
            UiRequestPurpose::Question | UiRequestPurpose::Other
        );
        if question && self.elicitation.form && !request.options.is_empty() {
            if self.state.phase.is_running() {
                self.send(requires_action());
            }
            self.spawn_elicitation(request);
            return;
        }

        let multi = request.kind == UiRequestKind::Multi;
        if question && !request.options.is_empty() {
            self.notice(
                schema::NoticeSeverity::Info,
                "Tact asked a question",
                Some(
                    "The client did not advertise form elicitation; the question was declined."
                        .into(),
                ),
            );
        } else if !question {
            self.notice(
                schema::NoticeSeverity::Info,
                "Tact asked for input",
                Some("This request cannot be forwarded to the ACP client; it was declined.".into()),
            );
        }
        self.respond_ui(declined(request.request_id, multi));
    }

    fn spawn_permission(&mut self, request: UiRequest) {
        let permission_options = request
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| {
                schema::PermissionOption::new(
                    index.to_string(),
                    label.clone(),
                    permission_kind(label),
                )
            })
            .collect::<Vec<_>>();
        let title = first_line(&request.prompt, 120);
        let description = request.prompt;
        let session_id = self.session_id.clone();
        let connection = self.connection.clone();
        let commands = self.commands.clone();
        let request_id = request.request_id;
        let acp_request =
            schema::RequestPermissionRequest::new(session_id.clone(), title, permission_options)
                .description(Some(description));
        let sent = connection.send_request(acp_request);
        self.open_requests
            .insert(request_id, sent.cancellation_handle());
        if connection
            .spawn(async move {
                let choice = match sent.block_task().await {
                    Ok(response) => match response.outcome {
                        schema::RequestPermissionOutcome::Selected(selected) => {
                            selected.option_id.to_string().parse::<usize>().ok()
                        }
                        _ => None,
                    },
                    Err(error) => {
                        debug!(%session_id, %error, "permission request failed");
                        None
                    }
                };
                let _ = commands.send(SessionCommand::User(UserCommand::UiResponse(
                    UiResponse::Select { request_id, choice },
                )));
                let _ = commands.send(SessionCommand::RequestSettled { request_id });
                Ok(())
            })
            .is_err()
        {
            self.open_requests.remove(&request_id);
            debug!(%self.session_id, "failed to spawn permission request");
            self.respond_ui(UiResponse::Select {
                request_id,
                choice: None,
            });
        }
    }

    fn spawn_elicitation(&mut self, request: UiRequest) {
        let multi = request.kind == UiRequestKind::Multi;
        let option_count = request.options.len();
        let schema = elicitation_schema(&request.options, multi);
        let session_id = self.session_id.clone();
        let connection = self.connection.clone();
        let commands = self.commands.clone();
        let request_id = request.request_id;
        let acp_request = schema::CreateElicitationRequest::new(
            schema::ElicitationMode::Form(schema::ElicitationFormMode::new(
                schema::ElicitationSessionScope::new(session_id.clone()),
                schema,
            )),
            request.prompt,
        );
        let sent = connection.send_request(acp_request);
        self.open_requests
            .insert(request_id, sent.cancellation_handle());
        if connection
            .spawn(async move {
                let answer = match sent.block_task().await {
                    Ok(response) => match response.action {
                        schema::ElicitationAction::Accept(accept) => {
                            map_accept(accept.content, multi, option_count)
                        }
                        _ => None,
                    },
                    Err(error) => {
                        debug!(%session_id, %error, "elicitation request failed");
                        None
                    }
                };
                let response = match answer {
                    Some(indices) if multi => UiResponse::MultiSelect {
                        request_id,
                        choices: Some(indices),
                    },
                    Some(mut indices) => UiResponse::Select {
                        request_id,
                        choice: indices.pop(),
                    },
                    None => declined(request_id, multi),
                };
                let _ = commands.send(SessionCommand::User(UserCommand::UiResponse(response)));
                let _ = commands.send(SessionCommand::RequestSettled { request_id });
                Ok(())
            })
            .is_err()
        {
            self.open_requests.remove(&request_id);
            self.notice(
                schema::NoticeSeverity::Error,
                "Tact asked a question",
                Some("The ACP connection could not carry the elicitation; the question was declined.".into()),
            );
            self.respond_ui(declined(request_id, multi));
        }
    }

    fn request_settled(&mut self, request_id: u64) {
        self.open_requests.remove(&request_id);
        // The UI response and this command are ordered, but the host removes
        // the pending entry asynchronously. Keep the id published until the
        // snapshot agrees, or reconciliation would reopen the dialog.
        if !self
            .host
            .pending_requests()
            .iter()
            .any(|request| request.request_id == request_id)
        {
            self.published_requests.remove(&request_id);
        }
    }

    fn set_config(
        &mut self,
        model: String,
        reply: oneshot::Sender<Result<Vec<schema::SessionConfigOption>, SetConfigFailure>>,
    ) {
        if !self.state.model.accepts(&model) {
            let _ = reply.send(Err(SetConfigFailure::UnknownModel(model)));
            return;
        }
        let changed = self.state.model.begin_request(model);
        if changed && !self.state.phase.is_running() {
            self.apply_model_now();
        }
        let _ = reply.send(Ok(self.state.model.config_options()));
    }

    /// Hand a queued model change to the host. Safe only at an idle boundary:
    /// the driver parks config commands behind the active turn, which would
    /// also park the `UiResponse` and `Cancel` commands a blocked turn needs.
    fn apply_model_now(&mut self) {
        let Some(model) = self.state.model.requested.clone() else {
            return;
        };
        if model == self.state.model.applied {
            self.state.model.requested = None;
            self.state.model.sent = None;
            return;
        }
        self.state.model.sent = Some(model.clone());
        if let Err(error) = self.host.send(UserCommand::SetModel(model)) {
            warn!(%self.session_id, %error, "failed to send model change");
            self.state.model.requested = None;
            self.state.model.sent = None;
            self.send_options();
        }
    }

    fn cancel_session(&mut self) {
        // Answer the waiting Tact requests before sending Cancel. A turn
        // parked on a permission or question cannot reach its cancellation
        // checkpoint until those waiters are released, and the driver's
        // command queue may still be parked behind the turn.
        let responder = self.host.ui_responder();
        for pending in self.host.pending_requests() {
            let response = match pending.kind {
                PendingUiRequestKind::Select => UiResponse::Select {
                    request_id: pending.request_id,
                    choice: None,
                },
                PendingUiRequestKind::MultiSelect => UiResponse::MultiSelect {
                    request_id: pending.request_id,
                    choices: None,
                },
            };
            if !responder.respond(response) {
                debug!(%self.session_id, request_id = pending.request_id, "pending request was already answered");
            }
        }
        for (_, cancellation) in self.open_requests.drain() {
            let _ = cancellation.cancel();
        }
        self.published_requests.clear();
        if let Err(error) = self.host.send(UserCommand::Cancel) {
            warn!(%self.session_id, %error, "failed to send cancel");
            if self.state.phase.is_running() {
                self.finish_turn(
                    schema::StopReason::Error(schema::ErrorStopReason::new()),
                    None,
                );
            }
        }
    }

    fn forward_user(&mut self, command: UserCommand) {
        let prompt = matches!(&command, UserCommand::SubmitTask(_));
        let ui_response = matches!(&command, UserCommand::UiResponse(_));
        if prompt && !self.state.phase.is_running() {
            self.state.phase = TurnPhase::Running;
            self.send(running());
        } else if ui_response && self.state.phase.is_running() {
            // The actor sent requires_action before publishing; progress
            // resumed with this answer. Never emit running after idle.
            self.send(running());
        }
        if let Err(error) = self.host.send(command) {
            warn!(%self.session_id, %error, "failed to send command to host");
            // A refused prompt must still settle: the client waits for idle.
            if prompt {
                self.finish_turn(
                    schema::StopReason::Error(schema::ErrorStopReason::new()),
                    None,
                );
            }
        }
    }

    fn respond_ui(&self, response: UiResponse) {
        let _ = self
            .commands
            .send(SessionCommand::User(UserCommand::UiResponse(response)));
    }

    fn send_options(&self) {
        self.send(schema::SessionUpdate::ConfigOptionUpdate(
            schema::ConfigOptionUpdate::new(self.state.model.config_options()),
        ));
    }

    fn send_finished_step(&mut self, tool_id: &str, result: &StepResult) {
        let status = match result.status {
            StepStatus::Success => schema::ToolCallStatus::Completed,
            StepStatus::Failed => schema::ToolCallStatus::Failed,
            StepStatus::Cancelled => schema::ToolCallStatus::Cancelled,
        };
        let text = result
            .detail
            .as_deref()
            .filter(|detail| !detail.trim().is_empty())
            .unwrap_or(&result.message)
            .to_string();
        let content = if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![text_tool_content(text)]
        };
        self.send(schema::SessionUpdate::ToolCallUpdate(
            schema::ToolCallUpdate::new(tool_id.to_string())
                .status(status)
                .content(content),
        ));
    }

    #[allow(clippy::too_many_arguments)]
    fn tool_call_update(
        &self,
        tool_id: &str,
        tool_name: &str,
        arg_summary: &str,
        arg_full: &str,
        presentation: &ToolPresentationInfo,
        status: schema::ToolCallStatus,
        content: Option<String>,
    ) -> schema::ToolCallUpdate {
        let title = if arg_summary.trim().is_empty() {
            if presentation.display_name.trim().is_empty() {
                tool_name.to_string()
            } else {
                presentation.display_name.clone()
            }
        } else {
            arg_summary.to_string()
        };
        let mut update = schema::ToolCallUpdate::new(tool_id.to_string())
            .name(tool_name.to_string())
            .title(title)
            .kind(tool_kind(tool_name, presentation))
            .status(status);
        if let Some(raw) = parse_json(arg_full) {
            update = update.raw_input(raw);
        }
        if let Some(content) = content {
            update = update.content(vec![text_tool_content(content)]);
        }
        update
    }

    fn ensure_agent_message(&mut self) -> schema::MessageId {
        if let Some(message) = &self.agent_message {
            return message.id.clone();
        }
        let id = self.next_id("agent");
        self.agent_message = Some(OpenMessage {
            id: id.clone(),
            text: String::new(),
        });
        id
    }

    fn ensure_thought(&mut self) -> schema::MessageId {
        if let Some(thought) = &self.thought {
            return thought.id.clone();
        }
        let id = self.next_id("thought");
        self.thought = Some(OpenMessage {
            id: id.clone(),
            text: String::new(),
        });
        id
    }

    fn finalize_agent_message(&mut self) {
        let Some(message) = self.agent_message.take() else {
            return;
        };
        if message.text.trim().is_empty() {
            return;
        }
        self.send(schema::SessionUpdate::AgentMessage(
            schema::AgentMessage::new(message.id).content(vec![message.text.into()]),
        ));
    }

    fn finalize_thought(&mut self) {
        let Some(thought) = self.thought.take() else {
            return;
        };
        if thought.text.trim().is_empty() {
            return;
        }
        self.send(schema::SessionUpdate::AgentThought(
            schema::AgentThought::new(thought.id).content(vec![thought.text.into()]),
        ));
    }

    /// The only emitter of idle. Gated on `Running`, so a late completion or a
    /// cancel on an idle session cannot produce a second turn boundary.
    fn finish_turn(&mut self, stop_reason: schema::StopReason, fallback_text: Option<String>) {
        self.finalize_agent_message();
        self.finalize_thought();
        if !self.turn_saw_text
            && let Some(text) = fallback_text.filter(|text| !text.trim().is_empty())
        {
            let id = self.next_id("agent");
            self.send(schema::SessionUpdate::AgentMessage(
                schema::AgentMessage::new(id).content(vec![text.into()]),
            ));
        }
        let open: Vec<String> = self.open_tools.drain().collect();
        for tool_id in open {
            self.send(schema::SessionUpdate::ToolCallUpdate(
                schema::ToolCallUpdate::new(tool_id).status(schema::ToolCallStatus::Cancelled),
            ));
        }
        self.turn_saw_text = false;
        if self.state.phase.is_running() {
            self.send(idle(Some(stop_reason)));
            self.state.phase = TurnPhase::Idle;
            self.apply_model_now();
        }
    }

    fn next_id(&self, kind: &str) -> schema::MessageId {
        let value = self.next_message.fetch_add(1, Ordering::Relaxed);
        schema::MessageId::new(format!("{kind}-{value}"))
    }

    fn send(&self, update: schema::SessionUpdate) {
        if let Err(error) = send_update(&self.connection, &self.session_id, update) {
            debug!(%self.session_id, %error, "failed to send ACP update");
        }
    }

    fn notice(
        &self,
        severity: schema::NoticeSeverity,
        title: impl Into<String>,
        description: Option<String>,
    ) {
        let mut notice = schema::Notice::new(severity, title);
        if let Some(description) = description {
            notice = notice.description(Some(description));
        }
        self.send(schema::SessionUpdate::Notice(notice));
    }
}

/// The single mapper for "the user gave no usable answer". `multi` selects the
/// variant the waiting Tact request accepts.
fn declined(request_id: u64, multi: bool) -> UiResponse {
    if multi {
        UiResponse::MultiSelect {
            request_id,
            choices: None,
        }
    } else {
        UiResponse::Select {
            request_id,
            choice: None,
        }
    }
}

/// Build the form schema. Values are option indices so repeated labels stay
/// distinguishable and client text can never be mistaken for a position.
fn elicitation_schema(options: &[String], multi: bool) -> schema::ElicitationSchema {
    let items = options
        .iter()
        .enumerate()
        .map(|(index, label)| schema::EnumOption::new(index.to_string(), label.clone()))
        .collect::<Vec<_>>();
    let property = if multi {
        schema::ElicitationPropertySchema::from(schema::MultiSelectPropertySchema::titled(items))
    } else {
        schema::ElicitationPropertySchema::from(schema::StringPropertySchema::new().one_of(items))
    };
    schema::ElicitationSchema::new().property(ELICITATION_FIELD, property, true)
}

/// Map accepted content back to Tact indices. Anything malformed or out of
/// range is a declined answer rather than a bogus index.
fn map_accept(
    content: Option<BTreeMap<String, schema::ElicitationContentValue>>,
    multi: bool,
    option_count: usize,
) -> Option<Vec<usize>> {
    let mut content = content?;
    let raw = match content.remove(ELICITATION_FIELD)? {
        schema::ElicitationContentValue::String(id) => vec![id],
        schema::ElicitationContentValue::StringArray(ids) => ids,
        _ => return None,
    };
    let parsed = raw
        .iter()
        .map(|id| id.parse::<usize>().ok())
        .collect::<Option<Vec<_>>>()?;
    if parsed.iter().any(|index| *index >= option_count) {
        return None;
    }
    if !multi && parsed.len() != 1 {
        return None;
    }
    Some(parsed)
}

fn send_update(
    connection: &ConnectionTo<Client>,
    session_id: &schema::SessionId,
    update: schema::SessionUpdate,
) -> AcpResult<()> {
    connection.send_notification(schema::UpdateSessionNotification::new(
        session_id.clone(),
        update,
    ))
}

fn running() -> schema::SessionUpdate {
    schema::SessionUpdate::StateUpdate(schema::StateUpdate::Running(
        schema::RunningStateUpdate::new(),
    ))
}

fn requires_action() -> schema::SessionUpdate {
    schema::SessionUpdate::StateUpdate(schema::StateUpdate::RequiresAction(
        schema::RequiresActionStateUpdate::new(),
    ))
}

fn idle(stop_reason: Option<schema::StopReason>) -> schema::SessionUpdate {
    schema::SessionUpdate::StateUpdate(schema::StateUpdate::Idle(
        schema::IdleStateUpdate::new().stop_reason(stop_reason),
    ))
}

fn text_tool_content(text: impl Into<String>) -> schema::ToolCallContent {
    schema::ToolCallContent::Content(Box::new(schema::Content::new(text.into())))
}

fn prompt_text(blocks: &[schema::ContentBlock]) -> Result<String, Error> {
    if blocks.is_empty() {
        return Err(invalid_params("prompt must not be empty"));
    }
    let mut parts = Vec::with_capacity(blocks.len());
    for block in blocks {
        match block {
            schema::ContentBlock::Text(text) => parts.push(text.text.clone()),
            schema::ContentBlock::ResourceLink(link) => parts.push(format!(
                "[{}]({})",
                link.title.as_deref().unwrap_or(&link.name),
                link.uri
            )),
            _ => {
                return Err(invalid_params(
                    "this agent only supports text and resource_link prompt content",
                ));
            }
        }
    }
    Ok(parts.join("\n\n"))
}

fn parse_json(value: &str) -> Option<serde_json::Value> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed).ok()
}

fn tool_kind(tool_name: &str, presentation: &ToolPresentationInfo) -> schema::ToolKind {
    match presentation.visual_kind {
        ToolVisualKind::FileRead => schema::ToolKind::Read,
        ToolVisualKind::FileWrite | ToolVisualKind::FileEdit => schema::ToolKind::Edit,
        ToolVisualKind::Command => schema::ToolKind::Execute,
        ToolVisualKind::Task => schema::ToolKind::Think,
        ToolVisualKind::Subagent | ToolVisualKind::Sleep | ToolVisualKind::Generic => {
            match tool_name {
                "read_file" | "read_image" => schema::ToolKind::Read,
                "write_file" | "edit_file" | "apply_patch" => schema::ToolKind::Edit,
                "bash" | "run_command" | "background_run" => schema::ToolKind::Execute,
                "grep" | "glob" | "search" => schema::ToolKind::Search,
                "web_search" | "web_fetch" => schema::ToolKind::Fetch,
                _ => schema::ToolKind::Other,
            }
        }
    }
}

fn permission_kind(label: &str) -> schema::PermissionOptionKind {
    let lower = label.to_ascii_lowercase();
    if lower.contains("always") && (lower.contains("allow") || !lower.contains("deny")) {
        schema::PermissionOptionKind::AllowAlways
    } else if lower.contains("always") && lower.contains("deny") {
        schema::PermissionOptionKind::RejectAlways
    } else if lower.contains("allow") || lower.contains("approve") || lower.contains("yes") {
        schema::PermissionOptionKind::AllowOnce
    } else if lower.contains("deny") || lower.contains("reject") || lower.contains("no") {
        schema::PermissionOptionKind::RejectOnce
    } else {
        schema::PermissionOptionKind::Other("_tact".to_string())
    }
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().next().unwrap_or(text).trim();
    let mut result: String = line.chars().take(max_chars).collect();
    if result.is_empty() {
        result = "Tact needs approval".to_string();
    }
    result
}

fn invalid_params(message: impl ToString) -> Error {
    Error::invalid_params().data(message.to_string())
}

fn internal(message: impl ToString) -> Error {
    Error::internal_error().data(message.to_string())
}

fn session_not_found(session_id: &schema::SessionId) -> Error {
    schema::Error::session_not_found(session_id.clone()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> ModelSelection {
        ModelSelection {
            applied: "fake-a".to_string(),
            requested: None,
            sent: None,
            candidates: vec!["fake-a".to_string(), "fake-b".to_string()],
        }
    }

    #[test]
    fn model_commit_confirms_a_queued_change() {
        let mut model = selection();
        assert!(model.begin_request("fake-b".to_string()));
        assert_eq!(model.shown(), "fake-b");
        assert!(model.commit("fake-b".to_string()));
        assert_eq!(model.applied, "fake-b");
        assert_eq!(model.requested, None);
        assert_eq!(model.sent, None);
        assert!(!model.commit("fake-b".to_string()));
    }

    #[test]
    fn model_commit_keeps_a_queued_change_on_unrelated_info() {
        let mut model = selection();
        model.begin_request("fake-b".to_string());
        assert!(!model.commit("fake-a".to_string()));
        assert_eq!(model.requested.as_deref(), Some("fake-b"));
        assert_eq!(model.applied, "fake-a");
    }

    #[test]
    fn model_commit_reports_an_external_change() {
        let mut model = selection();
        assert!(model.commit("fake-b".to_string()));
        assert_eq!(model.applied, "fake-b");
    }

    #[test]
    fn model_options_always_contain_the_current_value() {
        let mut model = selection();
        model.begin_request("fake-b".to_string());
        let options = model.config_options();
        assert_eq!(options.len(), 1);
        assert!(matches!(
            options[0].category,
            Some(schema::SessionConfigOptionCategory::Model)
        ));
        let schema::SessionConfigKind::Select(select) = &options[0].kind else {
            panic!("model option must be a select");
        };
        assert_eq!(select.current_value.0.as_ref(), "fake-b");
        let schema::SessionConfigSelectOptions::Ungrouped(values) = &select.options else {
            panic!("model options must be ungrouped");
        };
        assert!(
            values
                .iter()
                .any(|value| value.value.0.as_ref() == "fake-b")
        );
        assert!(
            values
                .iter()
                .any(|value| value.value.0.as_ref() == "fake-a")
        );
    }

    #[test]
    fn map_accept_reads_indices_and_rejects_bad_ones() {
        let mut content = BTreeMap::new();
        content.insert(
            ELICITATION_FIELD.to_string(),
            schema::ElicitationContentValue::StringArray(vec!["0".into(), "2".into()]),
        );
        assert_eq!(map_accept(Some(content), true, 3), Some(vec![0, 2]));

        let mut content = BTreeMap::new();
        content.insert(
            ELICITATION_FIELD.to_string(),
            schema::ElicitationContentValue::String("1".into()),
        );
        assert_eq!(map_accept(Some(content.clone()), false, 3), Some(vec![1]));
        assert_eq!(map_accept(Some(content), false, 1), None);

        let mut content = BTreeMap::new();
        content.insert(
            ELICITATION_FIELD.to_string(),
            schema::ElicitationContentValue::String("7".into()),
        );
        assert_eq!(map_accept(Some(content), false, 3), None);

        let mut content = BTreeMap::new();
        content.insert(
            ELICITATION_FIELD.to_string(),
            schema::ElicitationContentValue::Boolean(true),
        );
        assert_eq!(map_accept(Some(content), true, 3), None);
    }

    #[test]
    fn elicitation_schema_uses_index_values() {
        let schema = elicitation_schema(&["yes".into(), "yes".into()], false);
        let schema::ElicitationPropertySchema::String(property) =
            &schema.properties[ELICITATION_FIELD]
        else {
            panic!("single select must be a string property");
        };
        let values = property.one_of.as_ref().expect("oneOf populated");
        assert_eq!(values[0].value, "0");
        assert_eq!(values[1].value, "1");
        assert_eq!(values[0].title, "yes");
        assert_eq!(
            schema.required.as_deref(),
            Some(&[ELICITATION_FIELD.to_string()][..])
        );

        let schema = elicitation_schema(&["a".into(), "b".into()], true);
        assert!(matches!(
            schema.properties[ELICITATION_FIELD],
            schema::ElicitationPropertySchema::Array(_)
        ));
    }

    #[test]
    fn prompt_text_accepts_text_and_resource_links() {
        let blocks = vec![
            schema::ContentBlock::Text(schema::TextContent::new("hello")),
            schema::ContentBlock::ResourceLink(schema::ResourceLink::new("file", "file:///tmp/a")),
        ];
        assert_eq!(
            prompt_text(&blocks).expect("baseline blocks should be accepted"),
            "hello\n\n[file](file:///tmp/a)"
        );
    }

    #[test]
    fn prompt_text_rejects_media_blocks() {
        let blocks = vec![schema::ContentBlock::Image(schema::ImageContent::new(
            "AAAA",
            "image/png",
        ))];
        let error = prompt_text(&blocks).expect_err("image blocks are not advertised");
        assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn permission_kind_maps_tact_labels() {
        assert_eq!(
            permission_kind("Allow once"),
            schema::PermissionOptionKind::AllowOnce
        );
        assert_eq!(
            permission_kind("Always allow this tool"),
            schema::PermissionOptionKind::AllowAlways
        );
        assert_eq!(
            permission_kind("Always allow this pattern"),
            schema::PermissionOptionKind::AllowAlways
        );
        assert_eq!(
            permission_kind("Deny"),
            schema::PermissionOptionKind::RejectOnce
        );
    }

    #[test]
    fn tool_kind_prefers_presentation_over_tool_name() {
        let mut presentation = ToolPresentationInfo::generic("Write");
        presentation.visual_kind = ToolVisualKind::FileWrite;
        assert_eq!(tool_kind("bash", &presentation), schema::ToolKind::Edit);

        let presentation = ToolPresentationInfo::generic("Command");
        assert_eq!(tool_kind("bash", &presentation), schema::ToolKind::Execute);
    }

    #[test]
    fn first_line_truncates_and_falls_back() {
        assert_eq!(first_line("first\nsecond", 80), "first");
        assert_eq!(first_line("   ", 80), "Tact needs approval");
        assert_eq!(first_line("abcdef", 3), "abc");
    }

    #[test]
    fn parse_json_ignores_empty_and_non_json() {
        assert!(parse_json("  ").is_none());
        assert!(parse_json("not json").is_none());
        assert_eq!(parse_json(r#"{"a":1}"#).unwrap()["a"], 1);
    }
}
