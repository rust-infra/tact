//! A [`HostBackend`] that speaks ACP to an in-process agent.
//!
//! [`crate::acp_host`] reaches `tact-acp` through a child process. This module
//! does the same over `agent-client-protocol`'s `Channel` transport, which is a
//! loopback like `Stdio` but runs the agent in this process. The ACP traffic is
//! identical — same requests, same `session/update` notifications, same reverse
//! mapping through [`crate::acp_bridge`] — so this is the same frontend boundary
//! without the subprocess.
//!
//! # Why in-process
//!
//! A subprocess buys crash isolation and lets the desktop drive *any* ACP agent.
//! Running in-process gives up the first and keeps the second only as a code
//! path, but it removes the packaging requirement: a build that ships no
//! `tact-acp` binary can still drive sessions through the ACP frontend, and one
//! panic in the agent takes the window with it exactly as the in-process
//! `HostSession` already does. That is the same exposure the default backend
//! has, so nothing is regressed — the trade is packaging simplicity instead of
//! isolation.
//!
//! # Configuration
//!
//! The agent reads Tact's settings from the process globals, which the desktop
//! installs once at startup (`DesktopConfig::install_process_config`). An
//! embedder that has not done that passes the error text in `config_error`
//! instead, and every session request reports it the way the binary does.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use agent_client_protocol::{
    Channel, Client,
    v2::{ConnectionTo, Responder, schema},
};
use async_trait::async_trait;
use tact::ui_responder::{PendingUiRequest, PendingUiRequestKind};
use tact_host::{
    HostCloseError, HostEvent, HostExit, HostExtensionService, HostSendError, HostStartError,
    SessionLockRegistry, TryHostEvent,
};
use tact_protocol::{AgentUpdate, UiRequestOrigin, UiRequestPurpose, UiResponse, UserCommand};
use tokio::sync::{mpsc, oneshot};

use crate::{
    DesktopConfig, InitialSession, ProjectRecord,
    acp_bridge::{self, UpdateContext},
    runtime::HostBackend,
};

/// How many events the bridge may buffer before the desktop has to drain.
const EVENT_CAPACITY: usize = 256;

/// A permission request the in-process agent is waiting on.
struct OpenPermission {
    responder: Responder<schema::RequestPermissionResponse>,
    options: Vec<schema::PermissionOption>,
}

/// An elicitation the in-process agent is waiting on.
struct OpenElicitation {
    responder: Responder<schema::CreateElicitationResponse>,
    field: String,
    options: Vec<String>,
    multi: bool,
}

/// An ACP host whose agent runs in this process.
pub(crate) struct InProcessAcpHost {
    session_id: String,
    connection: ConnectionTo<agent_client_protocol::Agent>,
    events: mpsc::Receiver<HostEvent>,
    permissions: Arc<Mutex<HashMap<u64, OpenPermission>>>,
    elicitations: Arc<Mutex<HashMap<u64, OpenElicitation>>>,
    prompts: Arc<Mutex<HashMap<u64, String>>>,
    extension_host: HostExtensionService,
    closed: bool,
}

impl InProcessAcpHost {
    /// Start the agent on a loopback channel and open this project's session.
    pub(crate) async fn start(
        config: &DesktopConfig,
        project: &ProjectRecord,
        initial: &InitialSession,
        _lock_registry: Arc<SessionLockRegistry>,
    ) -> Result<Self, HostStartError> {
        let (transport, agent_side) = Channel::duplex();
        let (event_tx, events) = mpsc::channel(EVENT_CAPACITY);
        let permissions: Arc<Mutex<HashMap<u64, OpenPermission>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let elicitations: Arc<Mutex<HashMap<u64, OpenElicitation>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let prompts: Arc<Mutex<HashMap<u64, String>>> = Arc::new(Mutex::new(HashMap::new()));

        // The agent half. Its config already lives in the process globals the
        // desktop installed at startup, so there is no argv to parse here.
        tokio::spawn(async move {
            if let Err(error) = tact_acp::serve(agent_side, None).await {
                tracing::warn!(%error, "in-process ACP agent stopped");
            }
        });

        let cwd = project.root.clone();
        let resume_id = match initial {
            InitialSession::Resume(id) => Some(id.clone()),
            _ => None,
        };
        let (ready_tx, ready_rx) = oneshot::channel::<
            Result<(ConnectionTo<agent_client_protocol::Agent>, String), String>,
        >();

        let handler_permissions = permissions.clone();
        let handler_elicitations = elicitations.clone();
        let handler_prompts = prompts.clone();
        let handler_events = event_tx.clone();

        tokio::spawn(async move {
            let result = Client
                .v2()
                .name("tact-desktop")
                .on_receive_request(
                    {
                        let permissions = handler_permissions.clone();
                        let prompts = handler_prompts.clone();
                        let events = handler_events.clone();
                        async move |request: schema::RequestPermissionRequest,
                                    responder: Responder<schema::RequestPermissionResponse>,
                                    _connection: ConnectionTo<agent_client_protocol::Agent>| {
                            let request_id = reserve_request_id(&prompts, &request.title);
                            let labels: Vec<String> = request
                                .options
                                .iter()
                                .map(|option| option.name.clone())
                                .collect();
                            permissions
                                .lock()
                                .expect("permissions lock")
                                .insert(request_id, OpenPermission {
                                    responder,
                                    options: request.options.clone(),
                                });
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
                        let events = handler_events.clone();
                        async move |request: schema::CreateElicitationRequest,
                                    responder: Responder<schema::CreateElicitationResponse>,
                                    _connection: ConnectionTo<agent_client_protocol::Agent>| {
                            let Some((field, options, multi)) = elicitation_options(&request) else {
                                let _ = responder.respond(declined_elicitation());
                                return Ok(());
                            };
                            let request_id = reserve_request_id(&prompts, &request.message);
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
                .connect_with(transport, async move |connection| {
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
                    std::future::pending::<()>().await;
                    Ok(())
                })
                .await;

            if let Err(error) = result {
                tracing::warn!(%error, "in-process ACP connection ended");
            }
            let _ = event_tx.send(HostEvent::Disconnected).await;
        });

        let (connection, session_id) = ready_rx
            .await
            .map_err(|_| HostStartError::Bootstrap("in-process ACP agent did not start".into()))?
            .map_err(HostStartError::Bootstrap)?;

        let extension_host = HostExtensionService::new(
            project.root.clone(),
            tact_host::shared_skill_registry(&project.root)
                .map_err(|error| HostStartError::Config(format!("skill registry: {error}")))?,
            tact_host::extensions::default_plugin_home(),
        );

        let _ = config;
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

    fn spawn_request<Req>(&self, request: Req)
    where
        Req: agent_client_protocol::JsonRpcRequest + Send + 'static,
        Req::Response: Send + 'static,
    {
        let connection = self.connection.clone();
        tokio::spawn(async move {
            if let Err(error) = connection.send_request(request).block_task().await {
                tracing::debug!(%error, "ACP request failed");
            }
        });
    }

    fn answer(&self, response: UiResponse) {
        if let Some(request) = self
            .elicitations
            .lock()
            .expect("elicitations lock")
            .remove(&request_id_of(&response))
        {
            let selected: Vec<String> = match &response {
                UiResponse::Select { choice, .. } => choice
                    .and_then(|index| request.options.get(index).cloned())
                    .into_iter()
                    .collect(),
                UiResponse::MultiSelect { choices, .. } => choices
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|index| request.options.get(index).cloned())
                    .collect(),
            };
            let reply = if selected.is_empty() {
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
            let _ = request.responder.respond(reply);
            return;
        }
        let UiResponse::Select { choice, .. } = response else {
            return;
        };
        let Some(request) = self
            .permissions
            .lock()
            .expect("permissions lock")
            .remove(&request_id_of(&response))
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
}

fn request_id_of(response: &UiResponse) -> u64 {
    match response {
        UiResponse::Select { request_id, .. } | UiResponse::MultiSelect { request_id, .. } => {
            *request_id
        }
    }
}

/// Allocate a request id that no open request is using.
///
/// The registry's own size is not a safe counter: answered requests are
/// removed, so a later question could reuse a live id.
fn reserve_request_id(prompts: &Arc<Mutex<HashMap<u64, String>>>, prompt: &str) -> u64 {
    let mut prompts = prompts.lock().expect("prompts lock");
    let next = prompts.keys().max().copied().unwrap_or(0) + 1;
    prompts.insert(next, prompt.to_string());
    next
}

fn elicitation_options(
    request: &schema::CreateElicitationRequest,
) -> Option<(String, Vec<String>, bool)> {
    let schema::ElicitationMode::Form(form) = &request.mode else {
        return None;
    };
    let (field, property) = form.requested_schema.properties.iter().next()?;
    let (values, multi) = match property {
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
impl HostBackend for InProcessAcpHost {
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
            UserCommand::SubmitTask(text) => {
                self.spawn_request(schema::PromptRequest::new(
                    schema::SessionId::new(self.session_id.clone()),
                    vec![schema::ContentBlock::Text(schema::TextContent::new(text))],
                ));
                Ok(())
            }
            UserCommand::Cancel => self
                .connection
                .send_notification(schema::CancelSessionNotification::new(
                    schema::SessionId::new(self.session_id.clone()),
                ))
                .map_err(|_| HostSendError::Disconnected),
            UserCommand::SetPermissionMode(mode) => {
                self.spawn_request(schema::SetSessionConfigOptionRequest::new(
                    schema::SessionId::new(self.session_id.clone()),
                    schema::SessionConfigId::new("permission_mode"),
                    schema::SessionConfigOptionValue::Id {
                        value: schema::SessionConfigValueId::new(mode.as_str()),
                    },
                ));
                Ok(())
            }
            UserCommand::SetModel(model) => {
                self.spawn_request(schema::SetSessionConfigOptionRequest::new(
                    schema::SessionId::new(self.session_id.clone()),
                    schema::SessionConfigId::new("model"),
                    schema::SessionConfigOptionValue::Id {
                        value: schema::SessionConfigValueId::new(model),
                    },
                ));
                Ok(())
            }
            UserCommand::UiResponse(response) => {
                self.answer(response);
                Ok(())
            }
            other => {
                tracing::debug!(?other, "no ACP mapping for this command");
                Ok(())
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

/// Run one prompt through an in-process ACP host and return the transcript text.
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
    let mut host = InProcessAcpHost::start(
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
    while !saw_idle {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            return Err("the turn did not reach an idle boundary".to_string());
        };
        let Ok(Some(event)) = tokio::time::timeout(remaining, host.next_event()).await else {
            return Err("the turn did not reach an idle boundary".to_string());
        };
        let HostEvent::Agent(update) = event else {
            continue;
        };
        saw_idle = matches!(*update, AgentUpdate::RunFinished { .. });
        reducer.apply_batch([*update], &context);
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
