//! ACP (Agent Client Protocol) v2 frontend for Tact.
//!
//! T3 Code launches this binary as a local ACP agent and drives it over
//! newline-delimited JSON-RPC on stdin/stdout. There is deliberately no TUI
//! here: T3 renders the transcript from `session/update` notifications.
//!
//! The adapter owns one [`HostSession`] per ACP session. A per-session actor
//! pump translates [`AgentUpdate`] events into ACP updates and routes
//! `UserCommand`s back to the host.

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use agent_client_protocol::v2::{ConnectionTo, Responder, schema};
use agent_client_protocol::{Agent, Client, Error, Result as AcpResult, Stdio};
use tact::config::CliArgs;
use tact::store::{ArchiveFilter, SessionListQuery, open_sqlite_session_store};
use tact_host::{
    HostEvent, HostSession, HostSessionConfig, SessionLockRegistry, SessionSelection, TactPath,
    UserCommand,
};
use tact_protocol::{
    AgentUpdate, StepResult, StepStatus, ThinkingChunk, ToolPresentationInfo, ToolVisualKind,
    UiResponse,
};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, info, warn};

/// Bounded progress channel for one host session. Tool output can be chatty;
/// 1024 matches the headless observer used by `tact-ui`.
const PROGRESS_CAPACITY: usize = 1024;
/// How long `session/close` waits for the host driver to drain.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);

/// Run the ACP agent until stdin closes.
pub async fn run() -> anyhow::Result<()> {
    init_logging();

    let args = CliArgs::default();
    let config_error = tact::config::init_from_args(&args)
        .err()
        .map(|error| format!("{error:#}"));

    let manager = SessionManager::new(config_error);

    let result = Agent
        .v2()
        .name("tact-acp")
        .on_receive_request(
            async |request: schema::InitializeRequest,
                   responder: Responder<schema::InitializeResponse>,
                   _connection: ConnectionTo<Client>| {
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
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let manager = manager.clone();
                async move |request: schema::NewSessionRequest,
                            responder: Responder<schema::NewSessionResponse>,
                            connection: ConnectionTo<Client>| {
                    if !request.mcp_servers.is_empty() {
                        debug!(
                            count = request.mcp_servers.len(),
                            "ignoring client MCP servers; Tact uses its own MCP configuration"
                        );
                    }
                    let cwd = request.cwd.0.clone();
                    match manager.new_session(cwd, connection.clone()).await {
                        Ok(session_id) => {
                            responder
                                .respond(schema::NewSessionResponse::new(session_id.clone()))?;
                            let _ = send_update(&connection, &session_id, idle(None));
                            Ok(())
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
                    match manager
                        .resume_session(
                            cwd,
                            request.session_id.clone(),
                            request.replay_from.is_some(),
                            connection.clone(),
                        )
                        .await
                    {
                        Ok(()) => responder.respond(schema::ResumeSessionResponse::new()),
                        Err(error) => responder.respond_with_error(error),
                    }
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
}

struct SessionHandle {
    commands: mpsc::UnboundedSender<SessionCommand>,
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
            }),
        }
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

    fn handle(&self, session_id: &schema::SessionId) -> Result<SessionHandle, Error> {
        self.inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .get(session_id)
            .map(|handle| SessionHandle {
                commands: handle.commands.clone(),
            })
            .ok_or_else(|| session_not_found(session_id))
    }

    async fn new_session(
        &self,
        cwd: PathBuf,
        connection: ConnectionTo<Client>,
    ) -> Result<schema::SessionId, Error> {
        self.ensure_config()?;
        self.start_host(cwd, connection, SessionSelection::New)
            .await
    }

    async fn resume_session(
        &self,
        cwd: PathBuf,
        session_id: schema::SessionId,
        replay: bool,
        connection: ConnectionTo<Client>,
    ) -> Result<(), Error> {
        self.ensure_config()?;
        if self
            .inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .contains_key(&session_id)
        {
            if replay {
                send_update(
                    &connection,
                    &session_id,
                    schema::SessionUpdate::ReplayComplete(schema::ReplayComplete::new()),
                )?;
            }
            send_update(&connection, &session_id, idle(None))?;
            return Ok(());
        }

        let actual = self
            .start_host(
                cwd,
                connection.clone(),
                SessionSelection::Resume(session_id.to_string()),
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
        send_update(&connection, &session_id, idle(None))?;
        Ok(())
    }

    async fn start_host(
        &self,
        cwd: PathBuf,
        connection: ConnectionTo<Client>,
        selection: SessionSelection,
    ) -> Result<schema::SessionId, Error> {
        let tact_path = TactPath::new(cwd.clone());
        let store = open_sqlite_session_store(&tact_path.session_db_path())
            .await
            .map_err(|error| internal(format!("failed to open session store: {error:#}")))?;
        let config = HostSessionConfig {
            tact_path,
            work_dir: cwd,
            session: selection,
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
        self.inner
            .sessions
            .lock()
            .expect("session map poisoned")
            .insert(
                session_id.clone(),
                SessionHandle {
                    commands: commands.clone(),
                },
            );
        spawn_actor(
            host,
            session_id.clone(),
            connection,
            commands,
            command_rx,
            self.inner.next_message.clone(),
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

    fn cancel(&self, session_id: &schema::SessionId) {
        if let Ok(handle) = self.handle(session_id) {
            let _ = handle
                .commands
                .send(SessionCommand::User(UserCommand::Cancel));
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

enum SessionCommand {
    User(UserCommand),
    Close(oneshot::Sender<Result<(), String>>),
}

fn spawn_actor(
    host: HostSession,
    session_id: schema::SessionId,
    connection: ConnectionTo<Client>,
    commands: mpsc::UnboundedSender<SessionCommand>,
    mut command_rx: mpsc::UnboundedReceiver<SessionCommand>,
    next_message: Arc<AtomicU64>,
) {
    tokio::spawn(async move {
        let mut actor = SessionActor {
            host,
            session_id,
            connection,
            commands,
            next_message,
            foreground: false,
            agent_message: None,
            thought: None,
            open_tools: HashSet::new(),
            turn_saw_text: false,
        };
        loop {
            tokio::select! {
                event = actor.host.next_event() => {
                    match event {
                        Some(event) => actor.handle_event(event),
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
                        Some(SessionCommand::User(command)) => {
                            if matches!(&command, UserCommand::SubmitTask(_)) && !actor.foreground {
                                actor.foreground = true;
                                actor.send(running());
                            }
                            if let Err(error) = actor.host.send(command) {
                                warn!(session_id = %actor.session_id, %error, "failed to send command to host");
                            }
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

struct SessionActor {
    host: HostSession,
    session_id: schema::SessionId,
    connection: ConnectionTo<Client>,
    commands: mpsc::UnboundedSender<SessionCommand>,
    next_message: Arc<AtomicU64>,
    foreground: bool,
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
                self.finish_turn(
                    schema::StopReason::Error(schema::ErrorStopReason::new()),
                    None,
                );
            }
            AgentUpdate::RunStarted { .. } => {
                if !self.foreground {
                    self.foreground = true;
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
            } => self.handle_select(request_id, prompt, options, purpose),
            AgentUpdate::RequestMultiSelect {
                request_id, prompt, ..
            } => {
                self.notice(
                    schema::NoticeSeverity::Info,
                    "Tact asked a multiple-choice question",
                    Some(format!(
                        "{prompt}\n\nT3's ACP bridge cannot answer multi-select questions yet; the question was declined."
                    )),
                );
                self.respond_ui(UiResponse::MultiSelect {
                    request_id,
                    choices: None,
                });
            }
            AgentUpdate::Info(text) | AgentUpdate::MdInfo(text) => {
                self.notice(schema::NoticeSeverity::Info, "Tact", Some(text));
            }
            AgentUpdate::UiResponseRejected { request_id } => {
                debug!(%self.session_id, request_id, "UI response arrived after the request was answered");
            }
            AgentUpdate::TokenUsage(_)
            | AgentUpdate::TurnStats { .. }
            | AgentUpdate::ModelInfo(_)
            | AgentUpdate::RunFinished { .. }
            | AgentUpdate::StepAdded(_) => {}
            other => debug!(%self.session_id, ?other, "unmapped Tact update"),
        }
    }

    fn handle_select(
        &mut self,
        request_id: u64,
        prompt: String,
        options: Vec<String>,
        purpose: tact_protocol::UiRequestPurpose,
    ) {
        if matches!(
            purpose,
            tact_protocol::UiRequestPurpose::Permission
                | tact_protocol::UiRequestPurpose::PlanApproval
        ) {
            self.spawn_permission(request_id, prompt, options);
            return;
        }
        self.notice(
            schema::NoticeSeverity::Info,
            "Tact asked a question",
            Some(format!(
                "{prompt}\n\nT3's ACP bridge cannot forward this question yet; it was declined."
            )),
        );
        self.respond_ui(UiResponse::Select {
            request_id,
            choice: None,
        });
    }

    fn spawn_permission(&self, request_id: u64, prompt: String, options: Vec<String>) {
        if options.is_empty() {
            self.respond_ui(UiResponse::Select {
                request_id,
                choice: None,
            });
            return;
        }
        let connection = self.connection.clone();
        let request_connection = connection.clone();
        let session_id = self.session_id.clone();
        let commands = self.commands.clone();
        let fallback_commands = commands.clone();
        let permission_options = options
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
        let title = first_line(&prompt, 120);
        let spawner = request_connection.clone();
        let task = spawner.spawn(async move {
            let _ = send_update(&request_connection, &session_id, requires_action());
            let request = schema::RequestPermissionRequest::new(
                session_id.clone(),
                title,
                permission_options,
            )
            .description(Some(prompt));
            debug!(%session_id, "permission: sending request");
            let sent = request_connection.send_request(request);
            debug!(%session_id, "permission: request published; awaiting response");
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
            let _ = send_update(&request_connection, &session_id, running());
            Ok(())
        });
        if let Err(error) = task {
            debug!(%self.session_id, %error, "failed to spawn permission request");
            let _ = fallback_commands.send(SessionCommand::User(UserCommand::UiResponse(
                UiResponse::Select {
                    request_id,
                    choice: None,
                },
            )));
        }
    }

    fn respond_ui(&self, response: UiResponse) {
        let _ = self
            .commands
            .send(SessionCommand::User(UserCommand::UiResponse(response)));
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
        if self.foreground {
            self.send(idle(Some(stop_reason)));
            self.foreground = false;
        }
        self.turn_saw_text = false;
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
