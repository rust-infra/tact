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
    path::{Path, PathBuf},
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
    AgentUpdate, PermissionMode, StepResult, StepStatus, TaskSnapshot, TaskStatusSnapshot,
    ThinkingChunk, ToolPresentationInfo, ToolVisualKind, UiRequestPurpose, UiResponse,
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
                            // v2 treats `session/list`, `session/resume` and
                            // `session/close` as baseline: a non-null
                            // `capabilities.session` advertises them, so only
                            // the optional ones are named here. T3 derives
                            // `canList`/`canResume`/`canResumeAfterRestart`
                            // from exactly that reading (`normalizeInitializeResponse`),
                            // which is what turns on resume after a restart.
                            schema::AgentCapabilities::new().session(
                                schema::SessionCapabilities::new()
                                    .mcp(
                                        schema::McpCapabilities::new()
                                            .stdio(schema::McpStdioCapabilities::new())
                                            .http(schema::McpHttpCapabilities::new()),
                                    )
                                    .delete(schema::SessionDeleteCapabilities::new())
                                    .prompt(
                                        schema::PromptCapabilities::new()
                                            .image(schema::PromptImageCapabilities::new()),
                                    ),
                            ),
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
                    let mcp_servers = match session_mcp_servers(&request.mcp_servers) {
                        Ok(servers) => servers,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    let cwd = request.cwd.0.clone();
                    match manager.new_session(cwd, connection, mcp_servers).await {
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
                    let mcp_servers = match session_mcp_servers(&request.mcp_servers) {
                        Ok(servers) => servers,
                        Err(error) => return responder.respond_with_error(error),
                    };
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
                                .resume_session(
                                    cwd,
                                    session_id,
                                    replay_from,
                                    resume_connection,
                                    mcp_servers,
                                )
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
                async move |request: schema::DeleteSessionRequest,
                            responder: Responder<schema::DeleteSessionResponse>,
                            _connection: ConnectionTo<Client>| {
                    match manager.delete_session(&request.session_id).await {
                        Ok(()) => responder.respond(schema::DeleteSessionResponse::new()),
                        Err(error) => responder.respond_with_error(error),
                    }
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
                    let message_id = manager.next_message_id("user");
                    let session_id = request.session_id.clone();
                    let cwd = match manager.handle(&session_id) {
                        Ok(handle) => handle.cwd,
                        Err(error) => return responder.respond_with_error(error),
                    };
                    let text = match prompt_text(&request.prompt, &cwd) {
                        Ok(text) => text,
                        Err(error) => return responder.respond_with_error(error),
                    };
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

    /// Read the initial option state from global config. `HostSession::start`
    /// returns before bootstrap finishes, so setup responses must advertise
    /// from config rather than from the built agent.
    fn initial_state(&self) -> Result<SessionState, Error> {
        let settings = tact::config::try_settings()
            .ok_or_else(|| internal("Tact configuration is not installed"))?;
        Ok(SessionState::from_settings(&settings))
    }

    async fn new_session(
        &self,
        cwd: PathBuf,
        connection: ConnectionTo<Client>,
        mcp_servers: Vec<(String, tact::mcp::McpProjectConfig)>,
    ) -> Result<schema::NewSessionResponse, Error> {
        self.ensure_config()?;
        let state = self.initial_state()?;
        let options = state.config_options();
        let skill_cwd = cwd.clone();
        let session_id = self
            .start_host(
                cwd,
                connection.clone(),
                SessionSelection::New,
                state,
                self.elicitation(),
                mcp_servers,
            )
            .await?;
        let commands = available_commands(&available_skills(&skill_cwd));
        // A client that shows a `/` menu learns about commands from the
        // notification, not the setup response: T3 reads the response only when
        // probing a provider and listens for this update on a real session. The
        // response still carries the list, because that is the protocol's
        // stated place for it.
        send_update(
            &connection,
            &session_id,
            schema::SessionUpdate::AvailableCommandsUpdate(schema::AvailableCommandsUpdate::new(
                commands.clone(),
            )),
        )?;
        // A new ACP session starts idle. Sending an idle here would be a
        // second, spurious turn boundary for the client.
        Ok(schema::NewSessionResponse::new(session_id)
            .config_options(options)
            .available_commands(commands))
    }

    async fn resume_session(
        &self,
        cwd: PathBuf,
        session_id: schema::SessionId,
        replay_from: Option<schema::ReplayFrom>,
        connection: ConnectionTo<Client>,
        mcp_servers: Vec<(String, tact::mcp::McpProjectConfig)>,
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
            let commands = available_commands(&available_skills(&handle.cwd));
            send_update(
                &connection,
                &handle.id,
                schema::SessionUpdate::AvailableCommandsUpdate(
                    schema::AvailableCommandsUpdate::new(commands.clone()),
                ),
            )?;
            send_update(&connection, &handle.id, idle(None))?;
            return Ok(schema::ResumeSessionResponse::new()
                .config_options(options)
                .available_commands(commands));
        }

        let state = self.initial_state()?;
        let options = state.config_options();
        let skill_cwd = cwd.clone();
        let actual = self
            .start_host(
                cwd,
                connection.clone(),
                SessionSelection::Resume(session_id.to_string()),
                state,
                self.elicitation(),
                mcp_servers,
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
        let commands = available_commands(&available_skills(&skill_cwd));
        send_update(
            &connection,
            &session_id,
            schema::SessionUpdate::AvailableCommandsUpdate(schema::AvailableCommandsUpdate::new(
                commands.clone(),
            )),
        )?;
        send_update(&connection, &session_id, idle(None))?;
        Ok(schema::ResumeSessionResponse::new()
            .config_options(options)
            .available_commands(commands))
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
        mcp_servers: Vec<(String, tact::mcp::McpProjectConfig)>,
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
            extra_mcp_servers: mcp_servers,
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

    /// Hand a prompt to the actor. A slash command is a session action, not a
    /// model turn, so the actor decides which it is; the manager only routes.
    fn submit_prompt(&self, session_id: &schema::SessionId, text: String) -> Result<(), Error> {
        self.handle(session_id)?
            .commands
            .send(SessionCommand::Prompt(text))
            .map_err(|_| internal("session driver is closed"))
    }

    fn set_config_option(
        &self,
        session_id: &schema::SessionId,
        config_id: &schema::SessionConfigId,
        value: &schema::SessionConfigOptionValue,
    ) -> Result<oneshot::Receiver<Result<Vec<schema::SessionConfigOption>, SetConfigFailure>>, Error>
    {
        let id = config_id.0.as_ref();
        if !matches!(
            id,
            MODEL_CONFIG_ID | PERMISSION_CONFIG_ID | THOUGHT_LEVEL_CONFIG_ID
        ) {
            return Err(invalid_params(format!(
                "unknown config option `{config_id}`"
            )));
        }
        let schema::SessionConfigOptionValue::Id { value } = value else {
            return Err(invalid_params(format!(
                "config option `{config_id}` requires a string id value"
            )));
        };
        let handle = self.handle(session_id)?;
        let (reply, reply_rx) = oneshot::channel();
        handle
            .commands
            .send(SessionCommand::SetConfig {
                config_id: id.to_string(),
                value: value.0.to_string(),
                reply,
            })
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

    /// Delete one stored session.
    ///
    /// A live session is closed first, and its cwd is read before that: the
    /// workspace is what locates the store, and closing removes the handle that
    /// carries it. Deleting the row under a running host would otherwise leave
    /// its driver writing into a session the client can no longer see.
    async fn delete_session(&self, session_id: &schema::SessionId) -> Result<(), Error> {
        self.ensure_config()?;
        let cwd = match self.live_handle(session_id) {
            Some(handle) => {
                self.close_session(session_id).await;
                handle.cwd
            }
            None => std::env::current_dir()
                .map_err(|error| internal(format!("failed to resolve cwd: {error}")))?,
        };
        let tact_path = TactPath::new(cwd);
        let store = open_sqlite_session_store(&tact_path.session_db_path())
            .await
            .map_err(|error| internal(format!("failed to open session store: {error:#}")))?;
        store
            .delete_session(session_id.0.as_ref())
            .await
            .map_err(|error| internal(format!("failed to delete session: {error:#}")))
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

/// One select-valued session config option with an explicit in-flight overlay.
///
/// `applied` is what the runtime last reported, `requested` is what the client
/// last asked for, and `sent` marks the value already handed to the host. Three
/// states instead of a second boolean: "queued behind a turn", "sent, awaiting
/// the runtime's report", and "settled" are each representable once, so a
/// confirmation and an unrelated report cannot be confused.
#[derive(Clone, Debug)]
struct SelectOption {
    id: &'static str,
    name: &'static str,
    description: Option<&'static str>,
    category: Option<schema::SessionConfigOptionCategory>,
    applied: String,
    requested: Option<String>,
    sent: Option<String>,
    candidates: Vec<String>,
}

impl SelectOption {
    fn shown(&self) -> &str {
        self.requested.as_deref().unwrap_or(&self.applied)
    }

    fn accepts(&self, value: &str) -> bool {
        self.candidates.iter().any(|candidate| candidate == value)
    }

    fn to_acp(&self) -> schema::SessionConfigOption {
        let shown = self.shown();
        let mut values = Vec::with_capacity(self.candidates.len() + 1);
        // Defensive: ACP requires the current value to be one of the values, so
        // a writer cannot produce an option the client would reject.
        if !self.candidates.iter().any(|candidate| candidate == shown) {
            values.push(schema::SessionConfigSelectOption::new(
                shown.to_string(),
                shown.to_string(),
            ));
        }
        values.extend(
            self.candidates
                .iter()
                .map(|value| schema::SessionConfigSelectOption::new(value.clone(), value.clone())),
        );
        let option =
            schema::SessionConfigOption::select(self.id, self.name, shown.to_string(), values);
        match (self.description, &self.category) {
            (None, None) => option,
            (None, Some(category)) => option.category(category.clone()),
            (Some(description), None) => option.description(Some(description.to_string())),
            (Some(description), Some(category)) => option
                .description(Some(description.to_string()))
                .category(category.clone()),
        }
    }

    /// Returns whether the advertised value moved.
    fn begin_request(&mut self, value: String) -> bool {
        let changed = value != self.shown();
        self.requested = Some(value);
        changed
    }

    /// The runtime's report. Returns true when the client needs a
    /// `config_option_update`: the advertised value moved, or a queued change
    /// was confirmed. Only a matching report clears the overlay, because the
    /// runtime reports the same field for other reasons mid-turn.
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

/// The permission-mode option. Tact's `PermissionMode` labels are its values,
/// and the ids are the same three strings the config file uses.
const PERMISSION_CONFIG_ID: &str = "permission_mode";
/// The reasoning-effort option, shown by clients as a reasoning control.
const THOUGHT_LEVEL_CONFIG_ID: &str = "thought_level";
/// The single plan id this adapter uses, so each task-list update replaces the
/// previous checklist rather than accumulating plans.
const TACT_PLAN_ID: &str = "tasks";
/// Workspace-relative directory prompt images are written to.
const ACP_IMAGE_DIR: &str = ".tact-acp-images";
/// How long a non-turn slash command may take to produce its answer.
///
/// These are answered from shared snapshots, so the wait is one channel hop;
/// the bound exists only so a command that emits nothing cannot leave the
/// client's prompt open forever.
const COMMAND_SETTLE_TIMEOUT: Duration = Duration::from_millis(750);
/// Thinking-budget tiers offered when the model has no profile of its own.
const DEFAULT_THINKING_BUDGETS: [u32; 5] = [0, 8_000, 32_000, 64_000, 128_000];

/// How the active model expresses "think harder".
///
/// An effort model takes a named tier; a budget model takes a token count.
/// Which one applies is the provider's rule (`tact_llm::model_uses_effort`),
/// the same one the TUI's `/model` second step uses, so the two frontends
/// cannot offer different tier sets for one model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThoughtKind {
    Effort,
    Budget,
}

/// Every config option this adapter serves, in the order the client sees them.
#[derive(Debug)]
struct SessionState {
    model: SelectOption,
    permission: SelectOption,
    thought: SelectOption,
    thought_kind: ThoughtKind,
    phase: TurnPhase,
}

impl SessionState {
    fn from_settings(settings: &tact::config::ResolvedConfig) -> Self {
        let applied = if settings.agent.model.trim().is_empty() {
            settings.llm.model.clone()
        } else {
            settings.agent.model.clone()
        };
        let mut models = settings.llm.models.clone();
        if !models.iter().any(|candidate| candidate == &applied) {
            models.insert(0, applied.clone());
        }
        let (thought, thought_kind) = thought_option(settings, &applied);
        Self {
            model: SelectOption {
                id: MODEL_CONFIG_ID,
                name: "Model",
                description: None,
                category: Some(schema::SessionConfigOptionCategory::Model),
                applied,
                requested: None,
                sent: None,
                candidates: models,
            },
            permission: SelectOption {
                id: PERMISSION_CONFIG_ID,
                name: "Permission mode",
                description: Some("What Tact may do without asking"),
                // T3 treats a `mode`-category select as its plan-mode toggle,
                // and only a mode option makes that toggle settable on v2.
                category: Some(schema::SessionConfigOptionCategory::Mode),
                applied: settings.permission_mode.as_str().to_string(),
                requested: None,
                sent: None,
                candidates: PermissionMode::ALL
                    .iter()
                    .map(|mode| mode.as_str().to_string())
                    .collect(),
            },
            thought,
            thought_kind,
            phase: TurnPhase::Idle,
        }
    }

    fn config_options(&self) -> Vec<schema::SessionConfigOption> {
        vec![
            self.model.to_acp(),
            self.permission.to_acp(),
            self.thought.to_acp(),
        ]
    }

    fn option_mut(&mut self, config_id: &str) -> Option<&mut SelectOption> {
        match config_id {
            MODEL_CONFIG_ID => Some(&mut self.model),
            PERMISSION_CONFIG_ID => Some(&mut self.permission),
            THOUGHT_LEVEL_CONFIG_ID => Some(&mut self.thought),
            _ => None,
        }
    }

    /// Rebuild the thought-level option for the model that was just applied.
    ///
    /// A model switch can change the tier set (an effort model became a budget
    /// model, or the profile changed), so the option cannot be built once at
    /// setup. A pending request survives only if the new model still offers it.
    fn rebuild_thought(&mut self, model: &str) {
        let Some(settings) = tact::config::try_settings() else {
            return;
        };
        let (mut thought, kind) = thought_option(&settings, model);
        thought.applied = current_thought_value(&thought.candidates, kind, None);
        if let Some(requested) = self.thought.requested.clone()
            && thought.candidates.iter().any(|value| value == &requested)
        {
            thought.requested = Some(requested);
        }
        self.thought = thought;
        self.thought_kind = kind;
    }

    /// Settle the thought option from the runtime's report.
    ///
    /// `ModelInfo` carries both possible shapes; only the one this model uses
    /// is meaningful, and an absent value means the model runs with thinking
    /// off.
    fn commit_thought(&mut self, budget: Option<u32>, effort: Option<&str>) -> bool {
        let reported = match self.thought_kind {
            ThoughtKind::Effort => effort.unwrap_or("none").to_string(),
            ThoughtKind::Budget => budget.unwrap_or(0).to_string(),
        };
        self.thought.commit(reported)
    }
}

/// Build the thought-level option for `model`.
fn thought_option(
    settings: &tact::config::ResolvedConfig,
    model: &str,
) -> (SelectOption, ThoughtKind) {
    let profile = settings.llm.model_profiles.get(model);
    let provider = settings.llm.provider_info();
    if tact_llm::model_uses_effort(model, &provider) {
        let candidates = profile
            .filter(|profile| !profile.reasoning_efforts.is_empty())
            .map(|profile| profile.reasoning_efforts.clone())
            .unwrap_or_else(|| tact_llm::default_effort_tiers(&provider))
            .into_iter()
            .map(|effort| effort.as_str().to_string())
            .collect::<Vec<_>>();
        let applied = settings
            .agent
            .reasoning_effort
            .map(|effort| effort.as_str().to_string())
            .unwrap_or_else(|| "none".to_string());
        (
            SelectOption {
                id: THOUGHT_LEVEL_CONFIG_ID,
                name: "Reasoning effort",
                description: Some("How much the model thinks before answering"),
                category: Some(schema::SessionConfigOptionCategory::ThoughtLevel),
                applied,
                requested: None,
                sent: None,
                candidates,
            },
            ThoughtKind::Effort,
        )
    } else {
        let candidates = profile
            .filter(|profile| !profile.thinking_budgets.is_empty())
            .map(|profile| {
                profile
                    .thinking_budgets
                    .iter()
                    .map(|budget| budget.to_string())
                    .collect()
            })
            .unwrap_or_else(|| {
                DEFAULT_THINKING_BUDGETS
                    .iter()
                    .map(|budget| budget.to_string())
                    .collect()
            });
        (
            SelectOption {
                id: THOUGHT_LEVEL_CONFIG_ID,
                name: "Thinking budget",
                description: Some("Tokens the model may spend thinking"),
                category: Some(schema::SessionConfigOptionCategory::ThoughtLevel),
                applied: settings.agent.thinking_budget.to_string(),
                requested: None,
                sent: None,
                candidates,
            },
            ThoughtKind::Budget,
        )
    }
}

/// A current value that is guaranteed to be one of `candidates`.
fn current_thought_value(
    candidates: &[String],
    kind: ThoughtKind,
    reported: Option<&str>,
) -> String {
    let fallback = match kind {
        ThoughtKind::Effort => "none",
        ThoughtKind::Budget => "0",
    };
    let value = reported.unwrap_or(fallback);
    if candidates.iter().any(|candidate| candidate == value) {
        value.to_string()
    } else {
        candidates
            .first()
            .cloned()
            .unwrap_or_else(|| value.to_string())
    }
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
    /// A prompt or slash command from `session/prompt`. The actor decides
    /// whether it becomes a model turn or a session action.
    Prompt(String),
    /// Forward a host command (`SubmitTask`, `UiResponse`, `Cancel`, ...).
    User(UserCommand),
    /// Validate and queue a config change. The actor answers from its own
    /// state and never awaits the host on this path.
    SetConfig {
        config_id: String,
        value: String,
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
    /// A command turn's answer window closed; emit its idle if still owed.
    SettleCommand,
    Close(oneshot::Sender<Result<(), String>>),
}

/// Actor-side refusal, mapped to an ACP error at the handler boundary. Keeping
/// ACP error construction out of the actor keeps protocol policy in one place.
enum SetConfigFailure {
    UnknownValue { config_id: String, value: String },
}

fn set_config_error(error: SetConfigFailure) -> Error {
    match error {
        SetConfigFailure::UnknownValue { config_id, value } => invalid_params(format!(
            "`{value}` is not a value of config option `{config_id}`"
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
            context_window: context_window(),
            last_plan: Vec::new(),
            command_turn: false,
        };
        actor.reconcile_pending_requests();
        loop {
            tokio::select! {
                event = actor.host.next_event() => {
                    match event {
                        Some(event) => {
                            let answers_command = matches!(
                                &event,
                                HostEvent::Agent(update)
                                    if matches!(
                                        update.as_ref(),
                                        AgentUpdate::Info(_)
                                            | AgentUpdate::MdInfo(_)
                                            | AgentUpdate::PopupMarkdown { .. }
                                            | AgentUpdate::Error(_)
                                    )
                            );
                            actor.handle_event(event);
                            actor.reconcile_pending_requests();
                            if answers_command {
                                actor.settle_command_turn();
                            }
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
                        Some(SessionCommand::Prompt(text)) => actor.handle_prompt(text),
                        Some(SessionCommand::SetConfig {
                            config_id,
                            value,
                            reply,
                        }) => actor.set_config(config_id, value, reply),
                        Some(SessionCommand::ConfigOptions { reply }) => {
                            let _ = reply.send(actor.state.config_options());
                        }
                        Some(SessionCommand::Cancel) => actor.cancel_session(),
                        Some(SessionCommand::RequestSettled { request_id }) => {
                            actor.request_settled(request_id);
                        }
                        Some(SessionCommand::SettleCommand) => actor.settle_command_turn(),
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
    /// The context window to report with usage. The window is configuration,
    /// not an event, so it is read once at session start.
    context_window: u64,
    /// The last plan published, so an unchanged task list is not re-sent.
    last_plan: Vec<(String, schema::PlanEntryStatus)>,
    /// Whether a non-turn slash command still owes its idle.
    command_turn: bool,
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
                let model_changed = self.state.model.commit(params.model.clone());
                if model_changed {
                    self.state.rebuild_thought(&params.model);
                }
                let effort = params
                    .reasoning_effort
                    .map(|effort| effort.as_str().to_string());
                let thought_changed = self
                    .state
                    .commit_thought(params.thinking_budget, effort.as_deref());
                if model_changed || thought_changed {
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
            // Commands the host answers as a read-out rather than a turn:
            // `/stats`, `/background`, `/mcp list`, and a Goal's own report.
            // ACP has no modal, so the body becomes the notice description.
            AgentUpdate::PopupMarkdown { title, source } => {
                self.notice(schema::NoticeSeverity::Info, title, Some(source));
            }
            AgentUpdate::UiResponseRejected { request_id } => {
                debug!(%self.session_id, request_id, "UI response arrived after the request was answered");
            }
            AgentUpdate::TokenUsage(usage) => {
                // `total` is what Tact counts as context occupancy: every token
                // the model just saw plus what it wrote. That is the number the
                // TUI's own context meter shows.
                self.send(schema::SessionUpdate::UsageUpdate(
                    schema::UsageUpdate::new(usage.total as u64, self.context_window),
                ));
            }
            AgentUpdate::TasksChanged { tasks, .. } => self.publish_plan(&tasks),
            AgentUpdate::TurnStats { .. }
            | AgentUpdate::RunFinished { .. }
            | AgentUpdate::StepAdded(_) => {}
            other => debug!(%self.session_id, ?other, "unmapped Tact update"),
        }
    }

    /// Publish Tact's persistent task list as an ACP plan.
    ///
    /// The task list is the only user-facing checklist Tact has, so it is what
    /// a client's todo panel should show. One stable plan id means each update
    /// replaces the previous list, which is the semantics ACP gives it.
    fn publish_plan(&mut self, tasks: &[TaskSnapshot]) {
        let entries = tasks
            .iter()
            .map(|task| {
                (
                    task.subject.clone(),
                    match task.status {
                        TaskStatusSnapshot::Pending => schema::PlanEntryStatus::Pending,
                        TaskStatusSnapshot::InProgress => schema::PlanEntryStatus::InProgress,
                        TaskStatusSnapshot::Completed => schema::PlanEntryStatus::Completed,
                    },
                )
            })
            .collect::<Vec<_>>();
        if entries == self.last_plan {
            return;
        }
        self.last_plan = entries.clone();
        let entries = entries
            .into_iter()
            .map(|(content, status)| {
                schema::PlanEntry::new(content, schema::PlanEntryPriority::Medium, status)
            })
            .collect();
        self.send(schema::SessionUpdate::PlanUpdate(schema::PlanUpdate::new(
            schema::PlanUpdateContent::items(TACT_PLAN_ID, entries),
        )));
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
        config_id: String,
        value: String,
        reply: oneshot::Sender<Result<Vec<schema::SessionConfigOption>, SetConfigFailure>>,
    ) {
        let Some(option) = self.state.option_mut(&config_id) else {
            let _ = reply.send(Err(SetConfigFailure::UnknownValue { config_id, value }));
            return;
        };
        if !option.accepts(&value) {
            let _ = reply.send(Err(SetConfigFailure::UnknownValue { config_id, value }));
            return;
        }
        let changed = option.begin_request(value);
        if changed && !self.state.phase.is_running() {
            self.apply_config_now();
        }
        let _ = reply.send(Ok(self.state.config_options()));
    }

    /// Hand every queued config change to the host. Safe only at an idle
    /// boundary: the driver parks config commands behind the active turn,
    /// which would also park the `UiResponse` and `Cancel` commands a blocked
    /// turn needs.
    fn apply_config_now(&mut self) {
        // The model must be applied first: a permission change is
        // session-scoped and cheap, but the driver still processes commands in
        // order, so the model switch this turn was waiting on should not sit
        // behind it.
        if let Some(model) = self.state.model.requested.clone() {
            if model == self.state.model.applied {
                self.state.model.requested = None;
                self.state.model.sent = None;
            } else {
                self.state.model.sent = Some(model.clone());
                if let Err(error) = self.host.send(UserCommand::SetModel(model)) {
                    warn!(%self.session_id, %error, "failed to send model change");
                    self.state.model.requested = None;
                    self.state.model.sent = None;
                    self.send_options();
                }
            }
        }
        if let Some(mode) = self.state.permission.requested.clone() {
            if mode == self.state.permission.applied {
                self.state.permission.requested = None;
                self.state.permission.sent = None;
            } else {
                match mode.parse::<PermissionMode>() {
                    Ok(mode) => {
                        self.state.permission.sent = Some(mode.as_str().to_string());
                        if let Err(error) = self.host.send(UserCommand::SetPermissionMode(mode)) {
                            warn!(%self.session_id, %error, "failed to send permission mode");
                            self.state.permission.requested = None;
                            self.state.permission.sent = None;
                            self.send_options();
                        } else {
                            // The mode is accepted by the host immediately; no
                            // runtime report follows it, so settle here rather
                            // than leaving the overlay waiting forever.
                            self.state.permission.applied = mode.as_str().to_string();
                            self.state.permission.requested = None;
                            self.state.permission.sent = None;
                            self.send_options();
                        }
                    }
                    Err(error) => {
                        warn!(%self.session_id, %error, "invalid permission mode");
                        self.state.permission.requested = None;
                        self.state.permission.sent = None;
                        self.send_options();
                    }
                }
            }
        }
        if let Some(value) = self.state.thought.requested.clone() {
            if value == self.state.thought.applied {
                self.state.thought.requested = None;
                self.state.thought.sent = None;
            } else {
                self.state.thought.sent = Some(value.clone());
                // Both commands clear the other's stale state in the agent
                // (`SetThinkingBudget` clears effort, and vice versa), so the
                // reported `ModelInfo` will settle the overlay.
                let command = match self.state.thought_kind {
                    ThoughtKind::Effort => value
                        .parse::<tact_llm::OpenAiReasoningEffort>()
                        .ok()
                        .map(|effort| UserCommand::SetReasoningEffort(Some(effort.into()))),
                    ThoughtKind::Budget => value
                        .parse::<usize>()
                        .ok()
                        .map(UserCommand::SetThinkingBudget),
                };
                let sent = match command {
                    Some(command) => self.host.send(command).map_err(|error| error.to_string()),
                    None => Err(format!("`{value}` is not a valid thought level")),
                };
                if let Err(error) = sent {
                    warn!(%self.session_id, %error, "failed to send thought level");
                    self.state.thought.requested = None;
                    self.state.thought.sent = None;
                    self.send_options();
                }
            }
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

    /// Turn one `session/prompt` into either a model turn or a session action.
    ///
    /// A client cannot know which `/…` names the adapter handles, so an
    /// unrecognized one is forwarded as ordinary text: losing a typo silently
    /// would be worse than letting the model see it.
    fn handle_prompt(&mut self, text: String) {
        let command = match wrapped_user_request(&text) {
            // T3 only skips its instruction wrapper for prompts starting with
            // `/`, so a `$skill` as a thread's first turn arrives wrapped. Only
            // a command the adapter answers itself is lifted out of the
            // wrapper; everything else reaches the model with the instructions
            // intact.
            Some(body) => match slash_command(body) {
                SlashCommand::Forward(_) => SlashCommand::Forward(text.clone()),
                command => command,
            },
            None => slash_command(&text),
        };
        let forwarded = self.handle_prompt_command(command);
        if !forwarded {
            // Everything the adapter answers itself — an unknown skill, a
            // rejected `/model` value — still owes the client an idle: a prompt
            // response only means "inserted", so a command with no idle leaves
            // the client showing a turn that never ends.
            self.settle_local_prompt();
        }
    }

    /// Run one parsed prompt. Split from [`Self::handle_prompt`] so a
    /// command-plus-request prompt can run its command through the same arms.
    ///
    /// Returns whether the prompt was handed to the host. A host turn and a
    /// host command both settle themselves; only a local answer needs the
    /// caller to close the turn.
    fn handle_prompt_command(&mut self, command: SlashCommand) -> bool {
        match command {
            SlashCommand::Forward(text) => {
                self.forward_user(UserCommand::SubmitTask(text));
                true
            }
            SlashCommand::Model(value) => {
                self.apply_slash_config(MODEL_CONFIG_ID, value);
                false
            }
            SlashCommand::Permission(value) => {
                self.apply_slash_config(PERMISSION_CONFIG_ID, value);
                false
            }
            SlashCommand::Thought(value) => {
                self.apply_slash_config(THOUGHT_LEVEL_CONFIG_ID, value);
                false
            }
            SlashCommand::Skill { name, args } => self.invoke_skill(&name, &args),
            SlashCommand::Then(command, rest) => {
                // The command half may answer locally; the request half is a
                // real turn, so it owns the boundary even when the first half
                // sent a notice. A host command half would otherwise claim the
                // first answer-carrying update (`/stats`'s read-out) as its
                // own and settle the turn the request is still running.
                self.handle_prompt_command(*command);
                self.command_turn = false;
                self.forward_user(UserCommand::SubmitTask(rest));
                true
            }
            SlashCommand::Host(command) => {
                self.forward_slash_command(command);
                true
            }
        }
    }

    /// Close a prompt the adapter answered itself.
    ///
    /// A live turn's own idle settles the prompt, so this only fires when no
    /// turn is running: sending an idle mid-turn would end the turn the model
    /// is still working on.
    fn settle_local_prompt(&mut self) {
        if self.state.phase.is_running() {
            return;
        }
        self.send(idle(Some(schema::StopReason::EndTurn)));
    }

    /// Run a skill by wrapping its body the way the TUI does.
    ///
    /// The client inserted `$name`; a skill is a prompt template, so the turn
    /// the model sees must be the body, not the name. A name this session has
    /// no skill for is reported rather than silently submitted as text.
    ///
    /// Returns whether the skill was dispatched as a turn.
    fn invoke_skill(&mut self, name: &str, args: &str) -> bool {
        let registry = self.host.skill_registry();
        let registry = tact::skill::lock_skills(&registry);
        let Some(skill) = registry.skills().get(name) else {
            let available = registry.skills().keys().cloned().collect::<Vec<_>>();
            let mut available = available;
            available.sort();
            self.notice(
                schema::NoticeSeverity::Warning,
                format!("Unknown skill `{name}`"),
                Some(format!("Available: {}", available.join(", "))),
            );
            return false;
        };
        let task = tact::skill::format_skill_agent_task(skill, args);
        drop(registry);
        self.forward_user(UserCommand::SubmitTask(task));
        true
    }

    /// Apply a `/model`, `/permission`, or `/thought` argument against this
    /// session's own options, so the entity validating is the one that renders.
    fn apply_slash_config(&mut self, config_id: &'static str, value: String) {
        let running = self.state.phase.is_running();
        let Some(option) = self.state.option_mut(config_id) else {
            return;
        };
        if !option.accepts(&value) {
            let accepted = option.candidates.join(", ");
            self.notice(
                schema::NoticeSeverity::Warning,
                format!("Unknown value for /{config_id}"),
                Some(format!("`{value}` is not one of: {accepted}")),
            );
            return;
        }
        let changed = option.begin_request(value);
        if changed && !running {
            self.apply_config_now();
        }
        self.send_options();
    }

    /// Forward a command the host answers as updates rather than a turn.
    ///
    /// The host never emits `TaskComplete` for these, but an ACP prompt still
    /// has to settle: a client waits for an idle after every prompt, so this
    /// owns the turn boundary itself. The answer arrives first as a notice,
    /// then the idle. A queued command errors immediately and still settles.
    fn forward_slash_command(&mut self, command: UserCommand) {
        if !self.state.phase.is_running() {
            self.state.phase = TurnPhase::Running;
            self.send(running());
        }
        if let Err(error) = self.host.send(command) {
            warn!(%self.session_id, %error, "failed to send slash command");
            self.notice(
                schema::NoticeSeverity::Error,
                "Command failed",
                Some(error.to_string()),
            );
            self.finish_turn(
                schema::StopReason::Error(schema::ErrorStopReason::new()),
                None,
            );
            return;
        }
        // The host answers through the same event channel this actor reads, so
        // the idle cannot follow here: the answer is still queued. The turn
        // settles when an answer-carrying update arrives, not merely when any
        // event does — session bookkeeping events (a background-task snapshot)
        // are delivered at arbitrary times and would otherwise close the turn
        // before the answer reached the client. A command that produces no
        // answer at all settles on the fallback below.
        self.command_turn = true;
        let commands = self.commands.clone();
        tokio::spawn(async move {
            tokio::time::sleep(COMMAND_SETTLE_TIMEOUT).await;
            let _ = commands.send(SessionCommand::SettleCommand);
        });
    }

    /// Close a command turn. Idempotent: `finish_turn` is gated on `Running`,
    /// so an event that already settled is not settled twice.
    fn settle_command_turn(&mut self) {
        if self.command_turn {
            self.command_turn = false;
            self.finish_turn(schema::StopReason::EndTurn, None);
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
            schema::ConfigOptionUpdate::new(self.state.config_options()),
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
            self.apply_config_now();
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

/// Flatten prompt content into one task text, writing images into the
/// workspace as files.
///
/// Tact takes a text task and de-inlines image references into paths the model
/// reads with `read_image`, so an ACP image block becomes a saved file plus a
/// markdown reference to it: no base64 crosses the protocol boundary twice,
/// and a prompt image behaves exactly like one the user attached in the TUI.
fn prompt_text(blocks: &[schema::ContentBlock], work_dir: &Path) -> Result<String, Error> {
    if blocks.is_empty() {
        return Err(invalid_params("prompt must not be empty"));
    }
    let mut parts = Vec::with_capacity(blocks.len());
    for (index, block) in blocks.iter().enumerate() {
        match block {
            schema::ContentBlock::Text(text) => parts.push(text.text.clone()),
            schema::ContentBlock::ResourceLink(link) => parts.push(format!(
                "[{}]({})",
                link.title.as_deref().unwrap_or(&link.name),
                link.uri
            )),
            schema::ContentBlock::Image(image) => {
                parts.push(write_prompt_image(image, index, work_dir)?);
            }
            schema::ContentBlock::Audio(_) => {
                return Err(invalid_params(
                    "this agent does not accept audio prompt content",
                ));
            }
            _ => {
                return Err(invalid_params(
                    "this agent only supports text, resource_link, and image prompt content",
                ));
            }
        }
    }
    Ok(parts.join("\n\n"))
}

/// Write one prompt image under the workspace and return its markdown
/// reference.
///
/// The file must live inside the workspace or `read_image` refuses it, so it
/// goes in a per-session `acp-images` directory and the reference is relative.
fn write_prompt_image(
    image: &schema::ImageContent,
    index: usize,
    work_dir: &Path,
) -> Result<String, Error> {
    let extension = match image.mime_type.0.as_ref() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        other => {
            return Err(invalid_params(format!(
                "unsupported image media type `{other}`"
            )));
        }
    };
    let bytes = base64_decode(&image.data)
        .map_err(|error| invalid_params(format!("image data is not valid base64: {error}")))?;
    let dir = work_dir.join(ACP_IMAGE_DIR);
    std::fs::create_dir_all(&dir)
        .map_err(|error| internal(format!("failed to create image directory: {error}")))?;
    let name = format!("prompt-{index}.{extension}");
    std::fs::write(dir.join(&name), bytes)
        .map_err(|error| internal(format!("failed to write prompt image: {error}")))?;
    Ok(format!("![prompt image]({ACP_IMAGE_DIR}/{name})"))
}

/// Minimal standard-base64 decoder, so the adapter needs no new dependency.
fn base64_decode(data: &str) -> std::result::Result<Vec<u8>, String> {
    fn value(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let mut buffer = 0u32;
    let mut bits = 0u8;
    for byte in data.bytes() {
        if byte == b'=' || byte.is_ascii_whitespace() {
            continue;
        }
        let Some(value) = value(byte) else {
            return Err(format!("invalid character `{}`", byte as char));
        };
        buffer = (buffer << 6) | u32::from(value);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
            buffer &= (1 << bits) - 1;
        }
    }
    Ok(out)
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

/// The configured context window, or 0 when unknown.
///
/// Tact reports usage as "tokens in context" and leaves the window to config,
/// so a client's context meter needs both; an unknown window is reported as 0
/// rather than guessed, which makes the client show the used count alone.
fn context_window() -> u64 {
    tact::config::try_settings()
        .map(|settings| settings.agent.model_context_window as u64)
        .unwrap_or(0)
}

/// The commands this adapter advertises to an ACP client.
///
/// T3 sends a chosen command back as verbatim `/name args` prompt text, so this
/// list and [`slash_command`] must agree: a command named here with no handler
/// would reach the model as literal text.
fn available_commands(skills: &[(String, String)]) -> Vec<schema::AvailableCommand> {
    let commands = [
        ("compact", "Compact the conversation context", ""),
        ("stats", "Show session token and turn statistics", ""),
        ("mcp", "Inspect the connected MCP servers", "[list|prompts]"),
        (
            "background",
            "List background tasks, or show one by id",
            "[task-id]",
        ),
        // These two resolve to the same state the config options hold; the
        // slash forms exist because a client that hides config options (or a
        // keyboard user) still needs a way to switch them.
        (
            "model",
            "Switch the model for the rest of the session",
            "<model>",
        ),
        ("permission", "Set the permission mode", "default|plan|auto"),
        (
            "thought",
            "Set the reasoning effort or thinking budget",
            "<level>",
        ),
    ];
    let mut available = commands
        .into_iter()
        .map(|(name, description, hint)| {
            let mut command = schema::Command::new(name, description);
            if !hint.is_empty() {
                command = command.input(schema::AvailableCommandInput::Text(
                    schema::TextCommandInput::new(hint),
                ));
            }
            schema::AvailableCommand::Command(command)
        })
        .collect::<Vec<_>>();
    // A `$name` entry is how a client tells a skill apart from a command; the
    // client inserts it verbatim as `$name args`, which `slash_command` then
    // resolves back to the skill body. A skill whose name begins with `$`
    // would be ambiguous, so it is left out of the menu (it is still reachable
    // as `/skill <name>` text the model can act on).
    available.extend(
        skills
            .iter()
            .filter(|(name, _)| !name.starts_with('$'))
            .map(|(name, description)| {
                schema::AvailableCommand::Command(
                    schema::Command::new(format!("${name}"), description.clone()).input(
                        schema::AvailableCommandInput::Text(schema::TextCommandInput::new(
                            "[args]",
                        )),
                    ),
                )
            }),
    );
    available
}

/// The skills a `session/new` or `session/resume` should advertise.
///
/// Read once per session from the same registry the agent uses. A failure is
/// not fatal: a session with no menu entries is worse than one with a menu,
/// but far better than one that cannot start.
fn available_skills(cwd: &std::path::Path) -> Vec<(String, String)> {
    match tact::skill::get_skill_registry(cwd) {
        Ok(registry) => {
            let mut skills = registry
                .skills()
                .iter()
                .filter(|(name, _)| !registry.is_disabled(name))
                .map(|(name, doc)| (name.clone(), doc.manifest.description.clone()))
                .collect::<Vec<_>>();
            skills.sort_by(|a, b| a.0.cmp(&b.0));
            skills
        }
        Err(error) => {
            debug!(%error, "failed to read skills for advertisement");
            Vec::new()
        }
    }
}

/// A slash command the adapter handles itself.
enum SlashCommand {
    Model(String),
    Permission(String),
    Thought(String),
    /// `$name args`: the client's spelling for an advertised skill.
    Skill {
        name: String,
        args: String,
    },
    /// A command followed by a request on later lines: run both.
    Then(Box<SlashCommand>, String),
    /// A command the host answers as an update (`/compact`, `/stats`, ...).
    Host(UserCommand),
    /// Text that names no known command: it goes to the model verbatim, which
    /// is how a client is expected to treat an unadvertised `/…`.
    Forward(String),
}

/// The request body of T3's instruction wrapper, when the prompt has one.
///
/// T3 prepends `<t3_code_instructions>…<user_request>…</user_request>` on the
/// first turn of a thread and again whenever the interaction mode changes. It
/// skips the wrapper only when the prompt starts with `/`, so `$skill` and a
/// `/command` that arrives with mode context both come wrapped. The wrapper is
/// T3's, not ACP's: a client that sends the body bare must work exactly the
/// same, which is what makes this a probe rather than a protocol rule.
fn wrapped_user_request(text: &str) -> Option<&str> {
    let text = text.trim_start();
    if !text.starts_with("<t3_code_instructions>") {
        return None;
    }
    let body = text.split_once("<user_request>")?.1;
    let body = body.split_once("</user_request>")?.0;
    Some(body.trim())
}

/// Parse one `/name args` prompt. Case-insensitive, matching the client's own
/// menu behavior.
fn slash_command(text: &str) -> SlashCommand {
    let trimmed = text.trim();
    // A `$name` prompt is the client's way of invoking an advertised skill;
    // it is not a slash command, but it needs the same "action, not a turn"
    // routing, so one parser owns both spellings.
    if let Some(rest) = trimmed.strip_prefix('$') {
        let (name, args) = match rest.split_once(char::is_whitespace) {
            Some((name, args)) => (name, args.trim()),
            None => (rest, ""),
        };
        return SlashCommand::Skill {
            name: name.to_string(),
            args: args.to_string(),
        };
    }
    let Some(rest) = trimmed.strip_prefix('/') else {
        return SlashCommand::Forward(text.to_string());
    };
    // A command is one line. A prompt that continues past it is a command
    // followed by a request (`/permission auto` then a question), and
    // swallowing the rest as arguments would both lose the request and make
    // the value invalid. Only `$skill` takes free-form text.
    let (line, rest_of_prompt) = match rest.split_once('\n') {
        Some((line, remainder)) => (line.trim(), remainder.trim()),
        None => (rest.trim(), ""),
    };
    let (name, args) = match line.split_once(char::is_whitespace) {
        Some((name, args)) => (name, args.trim()),
        None => (line, ""),
    };
    let command = match name.to_ascii_lowercase().as_str() {
        "compact" => SlashCommand::Host(UserCommand::Compact),
        "stats" => SlashCommand::Host(UserCommand::QueryStats),
        "mcp" => match args.split_whitespace().next() {
            Some("prompts") => SlashCommand::Host(UserCommand::McpPrompts {
                server: args.split_whitespace().nth(1).map(str::to_string),
            }),
            _ => SlashCommand::Host(UserCommand::McpList),
        },
        "background" => SlashCommand::Host(UserCommand::QueryBackground(
            args.split_whitespace().next().map(str::to_string),
        )),
        "model" if !args.is_empty() => SlashCommand::Model(args.to_string()),
        "permission" if !args.is_empty() => SlashCommand::Permission(args.to_string()),
        "thought" if !args.is_empty() => SlashCommand::Thought(args.to_string()),
        _ => SlashCommand::Forward(text.to_string()),
    };
    // A recognized command with a request after it runs the command and then
    // submits the request as an ordinary turn, so neither half is dropped.
    if rest_of_prompt.is_empty() || matches!(command, SlashCommand::Forward(_)) {
        command
    } else {
        SlashCommand::Then(Box::new(command), rest_of_prompt.to_string())
    }
}

/// Translate the client's MCP servers into Tact's own project-config shape.
///
/// Tact connects these for this session only; they are never written to a
/// config file. A transport this adapter cannot build is a `-32602` rather
/// than a silent drop, because the client asked for tools the turn would then
/// be missing.
fn session_mcp_servers(
    servers: &[schema::McpServer],
) -> Result<Vec<(String, tact::mcp::McpProjectConfig)>, Error> {
    use tact::mcp::McpProjectConfig;

    servers
        .iter()
        .map(|server| match server {
            schema::McpServer::Stdio(stdio) => Ok((
                stdio.name.clone(),
                McpProjectConfig {
                    command: Some(stdio.command.0.display().to_string()),
                    args: stdio.args.clone(),
                    env: stdio
                        .env
                        .iter()
                        .map(|variable| (variable.name.clone(), variable.value.clone()))
                        .collect(),
                    ..McpProjectConfig::default()
                },
            )),
            schema::McpServer::Http(http) => Ok((
                http.name.clone(),
                McpProjectConfig {
                    url: Some(http.url.clone()),
                    headers: http
                        .headers
                        .iter()
                        .map(|header| (header.name.clone(), header.value.clone()))
                        .collect(),
                    ..McpProjectConfig::default()
                },
            )),
            other => {
                debug!(?other, "unsupported client MCP transport");
                Err(invalid_params(
                    "this agent supports stdio and http MCP servers only",
                ))
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection() -> SelectOption {
        SelectOption {
            id: MODEL_CONFIG_ID,
            name: "Model",
            description: None,
            category: Some(schema::SessionConfigOptionCategory::Model),
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
        let options = [model.to_acp()];
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
        let dir = tempfile::tempdir().expect("temp dir");
        let blocks = vec![
            schema::ContentBlock::Text(schema::TextContent::new("hello")),
            schema::ContentBlock::ResourceLink(schema::ResourceLink::new("file", "file:///tmp/a")),
        ];
        assert_eq!(
            prompt_text(&blocks, dir.path()).expect("baseline blocks should be accepted"),
            "hello\n\n[file](file:///tmp/a)"
        );
    }

    #[test]
    fn prompt_images_are_saved_inside_the_workspace() {
        let dir = tempfile::tempdir().expect("temp dir");
        let blocks = vec![schema::ContentBlock::Image(schema::ImageContent::new(
            "aGVsbG8=",
            "image/png",
        ))];
        let text = prompt_text(&blocks, dir.path()).expect("image blocks are accepted");
        assert_eq!(text, "![prompt image](.tact-acp-images/prompt-0.png)");
        let saved = std::fs::read(dir.path().join(ACP_IMAGE_DIR).join("prompt-0.png"))
            .expect("the image is written into the workspace");
        assert_eq!(saved, b"hello");
    }

    #[test]
    fn prompt_text_rejects_audio() {
        let dir = tempfile::tempdir().expect("temp dir");
        let blocks = vec![schema::ContentBlock::Audio(schema::AudioContent::new(
            "AAAA",
            "audio/wav",
        ))];
        let error = prompt_text(&blocks, dir.path()).expect_err("audio is not advertised");
        assert_eq!(error.code, agent_client_protocol::ErrorCode::INVALID_PARAMS);
    }

    #[test]
    fn base64_decode_reads_all_padding_cases() {
        assert_eq!(base64_decode("aGVsbG8=").expect("decode"), b"hello");
        assert_eq!(base64_decode("aGk=").expect("decode"), b"hi");
        assert_eq!(base64_decode("").expect("decode"), b"");
        assert!(base64_decode("!!!!").is_err());
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

    #[test]
    fn wrapped_user_request_reads_only_t3_wrappers() {
        let wrapped = "<t3_code_instructions>\n## mode\n</t3_code_instructions>\n\n<user_request>\n$greet hi\n</user_request>";
        assert_eq!(wrapped_user_request(wrapped), Some("$greet hi"));
        assert_eq!(wrapped_user_request("plain prompt"), None);
        // A half wrapper must not eat the prompt: only a complete one counts.
        assert_eq!(
            wrapped_user_request("<t3_code_instructions>\n<user_request>x"),
            None
        );
        assert_eq!(
            wrapped_user_request("<t3_code_instructions>\nx</t3_code_instructions>"),
            None
        );
    }

    #[test]
    fn a_wrapped_skill_is_still_a_skill() {
        let wrapped = "<t3_code_instructions>\n## mode\n</t3_code_instructions>\n\n<user_request>\n$greet the reader\n</user_request>";
        let body = wrapped_user_request(wrapped).expect("wrapper body");
        match slash_command(body) {
            SlashCommand::Skill { name, args } => {
                assert_eq!(name, "greet");
                assert_eq!(args, "the reader");
            }
            SlashCommand::Forward(text) => panic!("expected a skill, got text {text:?}"),
            _ => panic!("expected a skill, got a different command"),
        }
        // Ordinary text stays text, and the caller then forwards the whole
        // wrapper so the model still sees the instructions.
        assert!(matches!(
            slash_command(wrapped),
            SlashCommand::Forward(text) if text == wrapped
        ));
    }
}
