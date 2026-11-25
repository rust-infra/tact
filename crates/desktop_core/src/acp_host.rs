//! A [`HostBackend`] backed by an out-of-process ACP agent.
//!
//! The desktop already drives one session through [`HostBackend`]: it feeds
//! [`HostEvent`]s into a reducer and writes [`UserCommand`]s back. This module
//! provides the same interface over ACP, so switching the desktop from an
//! in-process `HostSession` to an ACP agent changes nothing above
//! `HostBackend`.
//!
//! The reverse mapping (ACP `session/update` → `AgentUpdate`) lives in
//! [`crate::acp_bridge`]; this module owns only the transport and the two
//! direction adapters.
//!
//! # Why a subprocess
//!
//! An ACP agent is a separate process by construction. That buys the desktop
//! two things an in-process host cannot offer: an agent panic cannot take the
//! window with it, and the same code path can drive any ACP agent, not only
//! Tact. The cost is the vocabulary gap `acp_bridge` documents.
//!
//! # Pending requests
//!
//! `HostSession::pending_requests()` is the authoritative pending-UI snapshot
//! the desktop reconciles gates against. ACP asks its questions through
//! `session/request_permission` and `elicitation/create` requests, so this
//! backend records each one as it arrives and answers it over the connection
//! when the desktop sends the matching `UiResponse`. The reconciliation above
//! this layer is therefore unchanged.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    AcpAgent, AcpAgentConfig, Client,
    v2::{ConnectionTo, Responder, schema},
};
use async_trait::async_trait;
use tact::ui_responder::{PendingUiRequest, PendingUiRequestKind};
use tact_host::{
    HostCloseError, HostEvent, HostExit, HostExtensionService, HostSendError, HostStartError,
    SessionLockRegistry, TryHostEvent,
};
use tact_protocol::{
    AgentUpdate, PermissionMode, UiRequestOrigin, UiRequestPurpose, UiResponse, UserCommand,
};
use tokio::sync::{mpsc, oneshot};

use crate::{
    DesktopConfig, InitialSession, ProjectRecord,
    acp_bridge::{self, UpdateContext},
    runtime::HostBackend,
};

/// How many events the bridge may buffer before the desktop has to drain.
const EVENT_CAPACITY: usize = 256;

/// A permission request ACP is waiting on.
struct OpenPermission {
    responder: Responder<schema::RequestPermissionResponse>,
    options: Vec<schema::PermissionOption>,
}

/// An elicitation ACP is waiting on, with the values its form offers.
struct OpenElicitation {
    responder: Responder<schema::CreateElicitationResponse>,
    /// The form field whose value the answer fills.
    field: String,
    /// The option values in the order the desktop shows them.
    options: Vec<String>,
    multi: bool,
}

/// The ACP-backed host.
pub(crate) struct AcpHost {
    session_id: String,
    /// The live connection. Requests are sent through it directly; the
    /// connection's own task drains responses.
    connection: ConnectionTo<agent_client_protocol::Agent>,
    events: mpsc::Receiver<HostEvent>,
    permissions: Arc<Mutex<HashMap<u64, OpenPermission>>>,
    elicitations: Arc<Mutex<HashMap<u64, OpenElicitation>>>,
    /// Prompt text per request id, so `pending_requests()` can report it.
    prompts: Arc<Mutex<HashMap<u64, String>>>,
    /// Extension management is filesystem state, not agent state, so it is
    /// built locally rather than asked of the agent.
    extension_host: HostExtensionService,
    closed: bool,
}

impl AcpHost {
    /// Connect to the ACP agent and open this project's session.
    pub(crate) async fn start(
        config: &DesktopConfig,
        project: &ProjectRecord,
        initial: &InitialSession,
        _lock_registry: Arc<SessionLockRegistry>,
    ) -> Result<Self, HostStartError> {
        let program = acp_agent_program();
        let mut launch = AcpAgentConfig::new(program);
        if let Some(path) = &config.host.config_path {
            launch = launch.arg("--config").arg(path.display().to_string());
        }
        if let Some(provider) = &config.host.provider {
            launch = launch.arg("--provider").arg(provider.clone());
        }
        if let Some(model) = &config.host.model {
            launch = launch.arg("--model").arg(model.clone());
        }
        if let Some(api_key) = &config.host.api_key {
            launch = launch.env("TACT_API_KEY", api_key.clone());
        }

        let (event_tx, events) = mpsc::channel(EVENT_CAPACITY);
        let permissions: Arc<Mutex<HashMap<u64, OpenPermission>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let elicitations: Arc<Mutex<HashMap<u64, OpenElicitation>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let prompts: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));
        let next_request_id = Arc::new(Mutex::new(1u64));
        let cwd = project.root.clone();
        let resume_id = match initial {
            InitialSession::Resume(id) => Some(id.clone()),
            _ => None,
        };

        let (ready_tx, ready_rx) = oneshot::channel::<
            Result<(ConnectionTo<agent_client_protocol::Agent>, String), String>,
        >();

        // The permission handler needs the connection's request id allocator
        // and the event sink; both are shared with the session task below.
        let handler_permissions = permissions.clone();
        let handler_elicitations = elicitations.clone();
        let handler_prompts = prompts.clone();
        let handler_next_id = next_request_id.clone();
        let handler_events = event_tx.clone();
        // The spawn below takes its own sender; this one stays here so the
        // readiness event can be queued once the handshake answers.
        let ready_events = event_tx.clone();

        tokio::spawn(async move {
            let result = Client
                .v2()
                .name("tact-desktop")
                .on_receive_request(
                    {
                        let permissions = handler_permissions.clone();
                        let prompts = handler_prompts.clone();
                        let next_id = handler_next_id.clone();
                        let events = handler_events.clone();
                        async move |request: schema::RequestPermissionRequest,
                                    responder: Responder<schema::RequestPermissionResponse>,
                                    _connection: ConnectionTo<agent_client_protocol::Agent>| {
                            let request_id = {
                                let mut next = next_id.lock().expect("request id lock");
                                let id = *next;
                                *next += 1;
                                id
                            };
                            let labels: Vec<String> = request
                                .options
                                .iter()
                                .map(|option| option.name.clone())
                                .collect();
                            prompts
                                .lock()
                                .expect("prompts lock")
                                .insert(request_id, request.title.clone());
                            permissions
                                .lock()
                                .expect("permissions lock")
                                .insert(request_id, OpenPermission {
                                    responder,
                                    options: request.options.clone(),
                                });
                            // A permission prompt is Tact's single-select
                            // gate, so it reaches the desktop as the same
                            // `RequestSelect` the in-process host sends.
                            let _ = events
                                .send(HostEvent::agent(AgentUpdate::RequestSelect {
                                    request_id,
                                    prompt: request.title.clone(),
                                    options: labels,
                                    log_confirm: false,
                                    from_subagent: false,
                                    purpose: UiRequestPurpose::Permission,
                                    origin: UiRequestOrigin::default(),
                                }))
                                .await;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_request(
                    {
                        let elicitations = handler_elicitations.clone();
                        let prompts = handler_prompts.clone();
                        let next_id = handler_next_id.clone();
                        let events = handler_events.clone();
                        async move |request: schema::CreateElicitationRequest,
                                    responder: Responder<schema::CreateElicitationResponse>,
                                    _connection: ConnectionTo<agent_client_protocol::Agent>| {
                            let Some((field, options, multi)) = elicitation_options(&request)
                            else {
                                // A form this backend cannot render is
                                // declined rather than left hanging: an
                                // unanswered elicitation blocks the agent.
                                let _ = responder.respond(declined_elicitation());
                                return Ok(());
                            };
                            let request_id = {
                                let mut next = next_id.lock().expect("request id lock");
                                let id = *next;
                                *next += 1;
                                id
                            };
                            prompts
                                .lock()
                                .expect("prompts lock")
                                .insert(request_id, request.message.clone());
                            elicitations
                                .lock()
                                .expect("elicitations lock")
                                .insert(request_id, OpenElicitation {
                                    responder,
                                    field,
                                    options: options.clone(),
                                    multi,
                                });
                            let update = if multi {
                                AgentUpdate::RequestMultiSelect {
                                    request_id,
                                    prompt: request.message.clone(),
                                    options,
                                    from_subagent: false,
                                    purpose: UiRequestPurpose::Other,
                                    origin: UiRequestOrigin::default(),
                                }
                            } else {
                                AgentUpdate::RequestSelect {
                                    request_id,
                                    prompt: request.message.clone(),
                                    options,
                                    log_confirm: false,
                                    from_subagent: false,
                                    purpose: UiRequestPurpose::Other,
                                    origin: UiRequestOrigin::default(),
                                }
                            };
                            let _ = events.send(HostEvent::agent(update)).await;
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_request!(),
                )
                .on_receive_notification(
                    {
                        let events = handler_events.clone();
                        async move |notification: schema::UpdateSessionNotification,
                                    _connection: ConnectionTo<agent_client_protocol::Agent>| {
                            let context = UpdateContext {
                                session_id: notification.session_id.0.as_ref(),
                                is_child: false,
                                parent_tool_id: None,
                            };
                            for update in
                                acp_bridge::to_agent_updates(&notification.update, &context)
                            {
                                if events.send(HostEvent::agent(update)).await.is_err() {
                                    break;
                                }
                            }
                            Ok(())
                        }
                    },
                    agent_client_protocol::on_receive_notification!(),
                )
                .connect_with(AcpAgent::new(launch), async move |connection| {
                    let initialize = connection
                        .send_request(schema::InitializeRequest::new(
                            agent_client_protocol::schema::ProtocolVersion::V2,
                            schema::Implementation::new("tact-desktop", env!("CARGO_PKG_VERSION")),
                        ))
                        .block_task()
                        .await?;
                    if initialize.capabilities.session.is_none() {
                        let _ = ready_tx.send(Err(
                            "the ACP agent did not advertise the v2 session capability".to_string(),
                        ));
                        return Ok(());
                    }
                    // `session/resume` echoes no id: the caller already named
                    // it, so a successful response is the whole answer.
                    let opened = match &resume_id {
                        Some(id) => connection
                            .send_request(schema::ResumeSessionRequest::new(
                                schema::SessionId::new(id.clone()),
                                cwd.clone(),
                            ))
                            .block_task()
                            .await
                            .map(|_| schema::SessionId::new(id.clone())),
                        None => connection
                            .send_request(schema::NewSessionRequest::new(cwd.clone()))
                            .block_task()
                            .await
                            .map(|response| response.session_id),
                    };
                    match opened {
                        Ok(session_id) => {
                            let _ =
                                ready_tx.send(Ok((connection.clone(), session_id.0.to_string())));
                        }
                        Err(error) => {
                            let _ = ready_tx.send(Err(format!("session setup failed: {error}")));
                            return Ok(());
                        }
                    }
                    // The session lives as long as this connection does; the
                    // desktop closes it by dropping the host.
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await;

            if let Err(error) = result {
                tracing::warn!(%error, "ACP agent connection ended");
            }
            let _ = event_tx.send(HostEvent::Disconnected).await;
        });

        let (connection, session_id) = ready_rx
            .await
            .map_err(|_| HostStartError::Bootstrap("ACP agent did not start".to_string()))?
            .map_err(HostStartError::Bootstrap)?;

        let extension_host = HostExtensionService::new(
            project.root.clone(),
            tact_host::shared_skill_registry(&project.root)
                .map_err(|error| HostStartError::Config(format!("skill registry: {error}")))?,
            tact_host::extensions::default_plugin_home(),
        );

        // The handshake above *is* the readiness boundary: the agent answered
        // `initialize` and opened the session. The controller treats a missing
        // `Ready` as "still starting", so without this the session stayed in
        // `SessionPhase::Starting` forever: the permission mode was never
        // re-applied, background tasks were never synced, the extensions
        // snapshot was never published, and the composer never took focus.
        let _ = ready_events.send(HostEvent::Ready).await;

        Ok(Self {
            session_id,
            connection,
            events,
            permissions,
            elicitations,
            prompts,
            extension_host,
            closed: false,
        })
    }

    fn spawn_request<Req>(&self, request: Req) -> Result<(), HostSendError>
    where
        Req: agent_client_protocol::JsonRpcRequest + Send + 'static,
        Req::Response: Send + 'static,
    {
        let connection = self.connection.clone();
        tokio::spawn(async move {
            // `block_task` must not run on the dispatch loop; every send from
            // the desktop happens on the actor's task, so this hop is what
            // keeps a response from deadlocking the loop it waits on.
            if let Err(error) = connection.send_request(request).block_task().await {
                tracing::debug!(%error, "ACP request failed");
            }
        });
        Ok(())
    }
}

/// Where the ACP agent binary lives.
///
/// Packaged builds ship `tact-acp` beside the desktop executable, the same way
/// they ship `tact-ui`. `TACT_ACP_BIN` overrides both for development.
fn acp_agent_program() -> PathBuf {
    if let Some(program) = std::env::var_os("TACT_ACP_BIN") {
        return PathBuf::from(program);
    }
    let name = if cfg!(windows) {
        "tact-acp.exe"
    } else {
        "tact-acp"
    };
    if let Ok(executable) = std::env::current_exe()
        && let Some(directory) = executable.parent()
    {
        let sibling = directory.join(name);
        if sibling.is_file() {
            return sibling;
        }
    }
    PathBuf::from(name)
}

/// The selectable values of an elicitation form, and whether it takes many.
///
/// Tact only knows single- and multi-select, so only that shape is rendered. A
/// form with other field types returns `None` and is declined.
fn elicitation_options(
    request: &schema::CreateElicitationRequest,
) -> Option<(String, Vec<String>, bool)> {
    let schema::ElicitationMode::Form(form) = &request.mode else {
        return None;
    };
    let (field, property) = form.requested_schema.properties.iter().next()?;
    let (values, multi) = match property {
        // A single-select form offers its choices as a string enum.
        schema::ElicitationPropertySchema::String(string) => (
            string.enum_values.clone().unwrap_or_else(|| {
                string
                    .one_of
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .map(|option| option.value)
                    .collect()
            }),
            false,
        ),
        // A multi-select form offers its choices as an array of strings.
        schema::ElicitationPropertySchema::Array(array) => {
            let values = match &array.items {
                schema::MultiSelectItems::String(items) => items.values.clone(),
                schema::MultiSelectItems::Titled(items) => items
                    .options
                    .iter()
                    .map(|option| option.value.clone())
                    .collect(),
                _ => Vec::new(),
            };
            (values, true)
        }
        _ => return None,
    };
    (!values.is_empty()).then_some((field.clone(), values, multi))
}

fn declined_elicitation() -> schema::CreateElicitationResponse {
    schema::CreateElicitationResponse::new(schema::ElicitationAction::Decline)
}

#[async_trait]
impl HostBackend for AcpHost {
    fn session_id(&self) -> &str {
        &self.session_id
    }

    async fn next_event(&mut self) -> Option<HostEvent> {
        self.events.recv().await
    }

    fn try_next_event(&mut self) -> TryHostEvent {
        match self.events.try_recv() {
            Ok(event) => TryHostEvent::Available(event),
            Err(mpsc::error::TryRecvError::Empty) => TryHostEvent::Empty,
            Err(mpsc::error::TryRecvError::Disconnected) => TryHostEvent::Disconnected,
        }
    }

    fn send(&self, command: UserCommand) -> Result<(), HostSendError> {
        if self.closed {
            return Err(HostSendError::Closed);
        }
        match command {
            UserCommand::SubmitTask(text) => self
                .spawn_request(schema::PromptRequest::new(
                    schema::SessionId::new(self.session_id.clone()),
                    vec![schema::ContentBlock::Text(schema::TextContent::new(text))],
                ))
                .map_err(|_| HostSendError::Closed),
            UserCommand::Cancel => self
                .connection
                .send_notification(schema::CancelSessionNotification::new(
                    schema::SessionId::new(self.session_id.clone()),
                ))
                .map_err(|_| HostSendError::Disconnected),
            UserCommand::SetPermissionMode(mode) => self
                .spawn_request(schema::SetSessionConfigOptionRequest::new(
                    schema::SessionId::new(self.session_id.clone()),
                    schema::SessionConfigId::new("permission_mode"),
                    schema::SessionConfigOptionValue::Id {
                        value: schema::SessionConfigValueId::new(permission_mode_value(mode)),
                    },
                ))
                .map_err(|_| HostSendError::Closed),
            UserCommand::SetModel(model) => self
                .spawn_request(schema::SetSessionConfigOptionRequest::new(
                    schema::SessionId::new(self.session_id.clone()),
                    schema::SessionConfigId::new("model"),
                    schema::SessionConfigOptionValue::Id {
                        value: schema::SessionConfigValueId::new(model),
                    },
                ))
                .map_err(|_| HostSendError::Closed),
            UserCommand::UiResponse(response) => {
                self.answer(response);
                Ok(())
            }
            // Everything else is a Tact console command with no ACP surface.
            // Reporting success would tell the desktop the operation was
            // queued when nothing was sent: `/compact`, a Goal action, and a
            // reasoning-tier change would all silently do nothing. The caller
            // surfaces this instead.
            other => {
                tracing::debug!(
                    ?other,
                    command = other.name(),
                    "no ACP mapping for this command"
                );
                Err(HostSendError::Unsupported(other.name()))
            }
        }
    }

    fn pending_requests(&self) -> Vec<PendingUiRequest> {
        let permissions = self.permissions.lock().expect("permissions lock");
        let elicitations = self.elicitations.lock().expect("elicitations lock");
        let prompts = self.prompts.lock().expect("prompts lock");
        let mut pending: Vec<PendingUiRequest> = permissions
            .iter()
            .map(|(request_id, request)| PendingUiRequest {
                request_id: *request_id,
                prompt: prompts.get(request_id).cloned().unwrap_or_default(),
                options: request
                    .options
                    .iter()
                    .map(|option| option.name.clone())
                    .collect(),
                kind: PendingUiRequestKind::Select,
                log_confirm: false,
                from_subagent: false,
                purpose: UiRequestPurpose::Permission,
                origin: UiRequestOrigin::default(),
            })
            .chain(
                elicitations
                    .iter()
                    .map(|(request_id, request)| PendingUiRequest {
                        request_id: *request_id,
                        prompt: prompts.get(request_id).cloned().unwrap_or_default(),
                        options: request.options.clone(),
                        kind: if request.multi {
                            PendingUiRequestKind::MultiSelect
                        } else {
                            PendingUiRequestKind::Select
                        },
                        log_confirm: false,
                        from_subagent: false,
                        purpose: UiRequestPurpose::Other,
                        origin: UiRequestOrigin::default(),
                    }),
            )
            .collect();
        pending.sort_by_key(|request| request.request_id);
        pending
    }

    fn extension_host(&self) -> HostExtensionService {
        self.extension_host.clone()
    }

    async fn close(&mut self) -> Result<HostExit, HostCloseError> {
        self.closed = true;
        let _ = self
            .connection
            .send_request(schema::CloseSessionRequest::new(schema::SessionId::new(
                self.session_id.clone(),
            )))
            .block_task()
            .await;
        Ok(HostExit {
            session_id: self.session_id.clone(),
            cancelled: false,
        })
    }
}

impl AcpHost {
    /// Answer a pending ACP request with the desktop's selection.
    fn answer(&self, response: UiResponse) {
        match response {
            UiResponse::Select { request_id, choice } => {
                // A single-select answer may belong to either a permission
                // prompt or a single-select elicitation; only one registry
                // holds the id.
                if let Some(request) = self
                    .elicitations
                    .lock()
                    .expect("elicitations lock")
                    .remove(&request_id)
                {
                    let value = choice.and_then(|index| request.options.get(index).cloned());
                    let response = match value {
                        Some(value) => schema::CreateElicitationResponse::new(
                            schema::ElicitationAction::Accept(
                                schema::ElicitationAcceptAction::new().content(
                                    std::collections::BTreeMap::from([(
                                        request.field,
                                        schema::ElicitationContentValue::String(value),
                                    )]),
                                ),
                            ),
                        ),
                        None => declined_elicitation(),
                    };
                    let _ = request.responder.respond(response);
                    return;
                }
                let Some(request) = self
                    .permissions
                    .lock()
                    .expect("permissions lock")
                    .remove(&request_id)
                else {
                    return;
                };
                let outcome = match choice.and_then(|index| request.options.get(index)) {
                    Some(option) => schema::RequestPermissionOutcome::Selected(
                        schema::SelectedPermissionOutcome::new(option.option_id.clone()),
                    ),
                    None => schema::CancelledPermissionOutcome::new().into(),
                };
                let _ = request
                    .responder
                    .respond(schema::RequestPermissionResponse::new(outcome));
            }
            UiResponse::MultiSelect {
                request_id,
                choices,
            } => {
                let Some(request) = self
                    .elicitations
                    .lock()
                    .expect("elicitations lock")
                    .remove(&request_id)
                else {
                    return;
                };
                let selected: Vec<String> = choices
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|index| request.options.get(index).cloned())
                    .collect();
                let response = if selected.is_empty() {
                    // No selection is a decline, not an empty acceptance:
                    // Tact reads "cancelled" for an empty multi-select.
                    declined_elicitation()
                } else {
                    let value = if request.multi {
                        schema::ElicitationContentValue::StringArray(selected)
                    } else {
                        schema::ElicitationContentValue::String(
                            selected.into_iter().next().unwrap_or_default(),
                        )
                    };
                    schema::CreateElicitationResponse::new(schema::ElicitationAction::Accept(
                        schema::ElicitationAcceptAction::new()
                            .content(std::collections::BTreeMap::from([(request.field, value)])),
                    ))
                };
                let _ = request.responder.respond(response);
            }
        }
    }
}

fn permission_mode_value(mode: PermissionMode) -> String {
    mode.as_str().to_string()
}

/// Start an ACP-backed host for `config`'s project and return its session id.
///
/// Test-only entry point: the production path constructs `AcpHost` through
/// `RealHostFactory`, which needs a `ProjectRecord` the caller does not have
/// here. This runs the same handshake so an integration test can assert on it
/// without duplicating the launch arguments.
#[cfg(feature = "acp-host")]
pub async fn session_id_for_test(
    config: &DesktopConfig,
    project_root: &std::path::Path,
) -> Result<String, String> {
    let mut registry = crate::project::ProjectRegistry::open(&config.app_data_dir)
        .map_err(|error| error.to_string())?;
    let record = registry
        .register(project_root)
        .map_err(|error| error.to_string())?;
    let host = AcpHost::start(
        config,
        &record,
        &InitialSession::New,
        crate::session_lock::registry(),
    )
    .await
    .map_err(|error| error.to_string())?;
    Ok(host.session_id().to_string())
}

/// Run one prompt through an ACP-backed host and return the transcript text.
///
/// Test-only entry point that exercises the whole path the desktop uses: the
/// agent is spawned, the prompt goes out as `session/prompt`, and every
/// `session/update` is fed through the real reducer. An assertion on the
/// returned text therefore proves the reverse mapping reaches the transcript,
/// not merely that a process started.
#[cfg(feature = "acp-host")]
pub async fn transcript_after_prompt(
    config: &DesktopConfig,
    project_root: &std::path::Path,
    prompt: &str,
) -> Result<String, String> {
    use crate::transcript::RowBody;

    let mut registry = crate::project::ProjectRegistry::open(&config.app_data_dir)
        .map_err(|error| error.to_string())?;
    let record = registry
        .register(project_root)
        .map_err(|error| error.to_string())?;
    let mut host = AcpHost::start(
        config,
        &record,
        &InitialSession::New,
        crate::session_lock::registry(),
    )
    .await
    .map_err(|error| error.to_string())?;
    host.send(UserCommand::SubmitTask(prompt.to_string()))
        .map_err(|error| error.to_string())?;

    let mut reducer = crate::transcript::TranscriptReducer::new();
    let context = crate::transcript::ReduceContext {
        session_epoch: crate::transcript::SessionEpoch(0),
        show_hook_output: false,
    };
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(45);
    let mut saw_idle = false;
    let mut saw_ready = false;
    while !saw_idle {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            return Err("the turn did not reach an idle boundary".to_string());
        };
        let Ok(Some(event)) = tokio::time::timeout(remaining, host.next_event()).await else {
            return Err("the turn did not reach an idle boundary".to_string());
        };
        let update = match event {
            // The controller keys "session is usable" off this event: without
            // it the session never leaves `SessionPhase::Starting`.
            HostEvent::Ready => {
                saw_ready = true;
                continue;
            }
            HostEvent::Agent(update) => update,
            _ => continue,
        };
        // A run that never activated cannot be finished, so the sequence this
        // loop accepts is exactly the one the reducer honours.
        saw_idle = matches!(*update, AgentUpdate::RunFinished { .. });
        reducer.apply_batch([*update], &context);
    }
    if !saw_ready {
        return Err("the subprocess ACP host never reported Ready".to_string());
    }

    let view = reducer.view();
    let mut text = String::new();
    for row in view.iter() {
        if let RowBody::Assistant(assistant) = &row.body {
            text.push_str(&assistant.markdown);
        }
    }
    Ok(text)
}
