use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use tact_catalog::{CommandArguments, CommandId, SettingId};
use tact_host::{HostCloseError, HostEvent, TryHostEvent};
use tact_protocol::{PermissionMode, UserCommand};
use tokio::sync::watch;
use tokio::sync::{mpsc, oneshot};

use crate::{
    AccountView, Appearance, CatalogLocale, DesktopConfig, DesktopEffect, DesktopEvent,
    DesktopSnapshot, InitialSession, PermissionModeView, ProjectRecord, ProjectRegistry, RequestId,
    RunPhase, SectionMask, SessionCatalog, SessionId, SessionPhase, StateCommit,
    commands::{CommandRoute, route_command, route_setting},
    error::{DesktopClosed, DesktopShutdownError, DesktopStartError, FailureKind, FailureView},
    intent::{DesktopIntent, GateResponse, ReaderRef, ReaderRequest, SettingValue, Submission},
    runtime::{HostBackend, HostFactory, RealHostFactory},
    state::DesktopState,
    transcript::GateResponseError,
};

const MAX_HOST_EVENTS_PER_DRAIN: usize = 256;

fn localized(locale: CatalogLocale, en: &'static str, zh: &'static str) -> &'static str {
    match locale {
        CatalogLocale::En => en,
        CatalogLocale::Zh => zh,
    }
}

fn task_status_label(
    locale: CatalogLocale,
    status: tact_protocol::TaskStatusSnapshot,
) -> &'static str {
    match (locale, status) {
        (CatalogLocale::En, tact_protocol::TaskStatusSnapshot::Pending) => "Pending",
        (CatalogLocale::Zh, tact_protocol::TaskStatusSnapshot::Pending) => "待处理",
        (CatalogLocale::En, tact_protocol::TaskStatusSnapshot::InProgress) => "In progress",
        (CatalogLocale::Zh, tact_protocol::TaskStatusSnapshot::InProgress) => "进行中",
        (CatalogLocale::En, tact_protocol::TaskStatusSnapshot::Completed) => "Completed",
        (CatalogLocale::Zh, tact_protocol::TaskStatusSnapshot::Completed) => "已完成",
    }
}

fn background_status_label(
    locale: CatalogLocale,
    status: tact_protocol::BackgroundTaskStatusSnapshot,
) -> &'static str {
    match (locale, status) {
        (CatalogLocale::En, tact_protocol::BackgroundTaskStatusSnapshot::Running) => "Running",
        (CatalogLocale::Zh, tact_protocol::BackgroundTaskStatusSnapshot::Running) => "运行中",
        (CatalogLocale::En, tact_protocol::BackgroundTaskStatusSnapshot::Completed) => "Completed",
        (CatalogLocale::Zh, tact_protocol::BackgroundTaskStatusSnapshot::Completed) => "已完成",
        (CatalogLocale::En, tact_protocol::BackgroundTaskStatusSnapshot::Error) => "Error",
        (CatalogLocale::Zh, tact_protocol::BackgroundTaskStatusSnapshot::Error) => "错误",
    }
}

fn subagent_status_label(
    locale: CatalogLocale,
    status: tact_protocol::SubagentStatusSnapshot,
) -> &'static str {
    match (locale, status) {
        (CatalogLocale::En, tact_protocol::SubagentStatusSnapshot::Running) => "Running",
        (CatalogLocale::Zh, tact_protocol::SubagentStatusSnapshot::Running) => "运行中",
        (CatalogLocale::En, tact_protocol::SubagentStatusSnapshot::Completed) => "Completed",
        (CatalogLocale::Zh, tact_protocol::SubagentStatusSnapshot::Completed) => "已完成",
        (CatalogLocale::En, tact_protocol::SubagentStatusSnapshot::Failed) => "Failed",
        (CatalogLocale::Zh, tact_protocol::SubagentStatusSnapshot::Failed) => "失败",
        (CatalogLocale::En, tact_protocol::SubagentStatusSnapshot::Cancelled) => "Cancelled",
        (CatalogLocale::Zh, tact_protocol::SubagentStatusSnapshot::Cancelled) => "已取消",
    }
}

fn start_error_message(error: DesktopStartError) -> String {
    match error {
        DesktopStartError::Config(message) | DesktopStartError::Runtime(message) => message,
    }
}

fn host_start_message(error: &tact_host::HostStartError, locale: CatalogLocale) -> String {
    match error {
        tact_host::HostStartError::Lock(_) => localized(
            locale,
            "Session is locked by another Tact process. Open another session or close the other window.",
            "会话已被另一个 Tact 进程锁定。请打开另一个会话或关闭另一个窗口。",
        )
        .to_string(),
        _ => error.to_string(),
    }
}

fn push_cli_argument(arguments: &mut Vec<String>, flag: &str, value: String) {
    arguments.push(flag.to_string());
    arguments.push(value);
}

fn cli_process_markdown(
    title: &str,
    locale: CatalogLocale,
    output: std::io::Result<std::process::Output>,
) -> String {
    let output = match output {
        Ok(output) => output,
        Err(error) => {
            return format!(
                "# {title}\n\n{}: {error}",
                localized(locale, "Failed to start tact-ui", "无法启动 tact-ui")
            );
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut markdown = format!(
        "# {title}\n\n{}: `{}`\n\n",
        localized(locale, "Exit status", "退出状态"),
        output.status
    );
    if !stdout.trim().is_empty() {
        markdown.push_str(&format!(
            "## {}\n\n```text\n",
            localized(locale, "Output", "输出")
        ));
        markdown.push_str(stdout.trim_end());
        markdown.push_str("\n```\n\n");
    }
    if !stderr.trim().is_empty() {
        markdown.push_str(&format!(
            "## {}\n\n```text\n",
            localized(locale, "Error output", "错误输出")
        ));
        markdown.push_str(stderr.trim_end());
        markdown.push_str("\n```\n");
    }
    markdown
}

/// Resolve the CLI sidecar next to the desktop executable before falling back
/// to PATH. Packaged macOS, Windows, and Linux assets ship both binaries in the
/// same directory.
pub fn tact_ui_binary_path() -> PathBuf {
    if let Some(program) = std::env::var_os("TACT_UI_BIN") {
        return PathBuf::from(program);
    }
    if let Ok(executable) = std::env::current_exe()
        && let Some(program) = sibling_tact_ui(&executable)
    {
        return program;
    }
    PathBuf::from(if cfg!(windows) {
        "tact-ui.exe"
    } else {
        "tact-ui"
    })
}

fn sibling_tact_ui(executable: &Path) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        "tact-ui.exe"
    } else {
        "tact-ui"
    };
    let candidate = executable.parent()?.join(name);
    candidate.is_file().then_some(candidate)
}

#[derive(Clone)]
pub struct DesktopController {
    intents: mpsc::UnboundedSender<ControllerMessage>,
    snapshot: Arc<RwLock<Arc<DesktopSnapshot>>>,
    thread: Arc<std::sync::Mutex<Option<std::thread::JoinHandle<()>>>>,
}

pub struct DesktopEvents {
    commits: tokio::sync::Mutex<CommitWatch>,
    urgent: tokio::sync::Mutex<mpsc::UnboundedReceiver<DesktopEvent>>,
}

type CommitWatch = (watch::Receiver<Option<Arc<StateCommit>>>, Option<u64>);

impl DesktopEvents {
    pub async fn recv(&self) -> Result<DesktopEvent, DesktopClosed> {
        let mut commits = self.commits.lock().await;
        let mut urgent = self.urgent.lock().await;
        loop {
            let (receiver, last_revision) = &mut *commits;
            if let Some(commit) = receiver.borrow().clone()
                && last_revision.is_none_or(|revision| commit.revision > revision)
            {
                *last_revision = Some(commit.revision);
                return Ok(DesktopEvent::Committed((*commit).clone()));
            }
            tokio::select! {
                changed = receiver.changed() => changed.map_err(|_| DesktopClosed)?,
                event = urgent.recv() => return event.ok_or(DesktopClosed),
            }
        }
    }
}

struct DesktopEventSink {
    commits: watch::Sender<Option<Arc<StateCommit>>>,
    urgent: mpsc::UnboundedSender<DesktopEvent>,
}

enum ControllerMessage {
    Intent(DesktopIntent),
    WaitIdle(oneshot::Sender<()>),
    Shutdown(oneshot::Sender<Result<(), DesktopShutdownError>>),
}

impl DesktopController {
    pub fn start(config: DesktopConfig) -> Result<(Self, DesktopEvents), DesktopStartError> {
        let factory: Arc<dyn HostFactory> = Arc::new(RealHostFactory::new(config.clone()));
        Self::start_with_factory(config, factory)
    }

    pub(crate) fn start_with_factory(
        config: DesktopConfig,
        factory: Arc<dyn HostFactory>,
    ) -> Result<(Self, DesktopEvents), DesktopStartError> {
        let snapshot = Arc::new(RwLock::new(Arc::new(DesktopSnapshot::empty(&config))));
        let (commit_tx, commit_rx) = watch::channel(None);
        let (urgent_tx, urgent_rx) = mpsc::unbounded_channel();
        let events = DesktopEventSink {
            commits: commit_tx,
            urgent: urgent_tx,
        };
        let (intents, intent_rx) = mpsc::unbounded_channel();
        let actor_snapshot = snapshot.clone();
        let (startup_tx, startup_rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = startup_tx.send(Err(DesktopStartError::Runtime(error.to_string())));
                    return;
                }
            };
            let actor = runtime.block_on(async move {
                let mut actor =
                    DesktopActor::new(config, factory, intent_rx, events, actor_snapshot).await?;
                actor.publish(true);
                Ok::<_, DesktopStartError>(actor)
            });
            match actor {
                Ok(actor) => {
                    let _ = startup_tx.send(Ok(()));
                    runtime.block_on(actor.run());
                }
                Err(error) => {
                    let _ = startup_tx.send(Err(error));
                }
            }
        });
        match startup_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                let _ = thread.join();
                return Err(error);
            }
            Err(_) => {
                let _ = thread.join();
                return Err(DesktopStartError::Runtime(
                    "controller startup thread stopped".into(),
                ));
            }
        }
        Ok((
            Self {
                intents,
                snapshot,
                thread: Arc::new(std::sync::Mutex::new(Some(thread))),
            },
            DesktopEvents {
                commits: tokio::sync::Mutex::new((commit_rx, None)),
                urgent: tokio::sync::Mutex::new(urgent_rx),
            },
        ))
    }

    pub fn dispatch(&self, intent: DesktopIntent) -> Result<(), DesktopClosed> {
        self.intents
            .send(ControllerMessage::Intent(intent))
            .map_err(|_| DesktopClosed)
    }

    pub fn snapshot(&self) -> Arc<DesktopSnapshot> {
        self.snapshot
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub async fn when_idle(&self) -> Result<(), DesktopClosed> {
        let (tx, rx) = oneshot::channel();
        self.intents
            .send(ControllerMessage::WaitIdle(tx))
            .map_err(|_| DesktopClosed)?;
        rx.await.map_err(|_| DesktopClosed)
    }

    pub async fn shutdown(&self) -> Result<(), DesktopShutdownError> {
        let (tx, rx) = oneshot::channel();
        self.intents
            .send(ControllerMessage::Shutdown(tx))
            .map_err(|_| DesktopShutdownError::Runtime("controller closed".into()))?;
        let result = match tokio::time::timeout(Duration::from_secs(15), rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(DesktopShutdownError::Runtime("shutdown ack lost".into())),
            Err(_) => Err(DesktopShutdownError::Runtime(
                "shutdown timed out waiting for session close".into(),
            )),
        };
        if result.is_ok()
            && let Some(thread) = self
                .thread
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take()
        {
            let _ = thread.join();
        }
        result
    }

    pub fn shutdown_blocking(&self) -> Result<(), DesktopShutdownError> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| DesktopShutdownError::Runtime(error.to_string()))?;
        runtime.block_on(self.shutdown())
    }
}

struct DesktopActor {
    config: DesktopConfig,
    factory: Arc<dyn HostFactory>,
    intents: mpsc::UnboundedReceiver<ControllerMessage>,
    events: DesktopEventSink,
    snapshot: Arc<RwLock<Arc<DesktopSnapshot>>>,
    state: DesktopState,
    registry: ProjectRegistry,
    project: Option<ProjectRecord>,
    catalog: Option<SessionCatalog>,
    host: Option<Box<dyn HostBackend>>,
    queue: VecDeque<Submission>,
    request_id: u64,
    last_gate_reconcile: Instant,
    restore_failure: Option<FailureView>,
}

impl DesktopActor {
    async fn new(
        config: DesktopConfig,
        factory: Arc<dyn HostFactory>,
        intents: mpsc::UnboundedReceiver<ControllerMessage>,
        events: DesktopEventSink,
        snapshot: Arc<RwLock<Arc<DesktopSnapshot>>>,
    ) -> Result<Self, DesktopStartError> {
        let registry = ProjectRegistry::open(&config.app_data_dir)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        let mut actor = Self {
            state: DesktopState::new(&config),
            config,
            factory,
            intents,
            events,
            snapshot,
            registry,
            project: None,
            catalog: None,
            host: None,
            queue: VecDeque::new(),
            request_id: 1,
            last_gate_reconcile: Instant::now(),
            restore_failure: None,
        };
        actor.state.set_projects(actor.registry.records());
        if let Some(root) = actor.config.initial_project.clone()
            && let Err(error) = actor
                .open_project(root, actor.config.initial_session.clone())
                .await
        {
            actor.set_failure(start_error_message(error), true);
        }
        Ok(actor)
    }

    async fn run(mut self) {
        let mut poll =
            tokio::time::interval(self.config.update_batch.max(Duration::from_millis(1)));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            self.drain_host_events().await;
            tokio::select! {
                message = self.intents.recv() => {
                    let Some(message) = message else { break };
                    if self.handle_message(message).await {
                        break;
                    }
                }
                _ = poll.tick() => {}
            }
        }
        let _ = self.close_host().await;
    }

    async fn handle_message(&mut self, message: ControllerMessage) -> bool {
        match message {
            ControllerMessage::Intent(DesktopIntent::Shutdown) => {
                if self.close_host().await.is_ok() {
                    let _ = self
                        .events
                        .urgent
                        .send(DesktopEvent::Effect(DesktopEffect::WindowCloseRequested));
                }
                true
            }
            ControllerMessage::Intent(intent) => {
                if let Err(message) = self.handle_intent(intent).await {
                    self.set_failure(message, false);
                }
                false
            }
            ControllerMessage::WaitIdle(ack) => {
                self.drain_host_events().await;
                let _ = ack.send(());
                false
            }
            ControllerMessage::Shutdown(ack) => {
                let result = self.close_host().await.map_err(DesktopShutdownError::Host);
                let closed = result.is_ok();
                let _ = ack.send(result);
                closed
            }
        }
    }

    async fn handle_intent(&mut self, intent: DesktopIntent) -> Result<(), String> {
        match intent {
            DesktopIntent::OpenProject { root } => {
                self.open_project(root, InitialSession::ResumeLast)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
            }
            DesktopIntent::CloseProject { project_id } => {
                if self.project.as_ref().map(|project| &project.id) == Some(&project_id)
                    && self.close_host().await.is_ok()
                {
                    let epoch = self.state.begin_session_switch();
                    self.project = None;
                    self.catalog = None;
                    self.state.set_project(None);
                    self.state.sessions.items.clear();
                    self.state.session = crate::SessionView {
                        epoch,
                        phase: SessionPhase::Empty,
                        ..crate::SessionView::default()
                    };
                    self.publish(true);
                }
            }
            DesktopIntent::NewSession => {
                self.start_session(InitialSession::New).await?;
            }
            DesktopIntent::ResumeSession { session_id } => {
                self.start_session(InitialSession::Resume(session_id.0))
                    .await?;
            }
            DesktopIntent::ResumeLast => {
                self.start_session(InitialSession::ResumeLast).await?;
            }
            DesktopIntent::Submit(submission) | DesktopIntent::Queue(submission) => {
                self.submit(submission)?;
            }
            DesktopIntent::EditQueued { index, text } => {
                if let Some(item) = self.queue.get_mut(index) {
                    item.text = text;
                }
                self.sync_queue();
            }
            DesktopIntent::RemoveQueued { index } => {
                if index < self.queue.len() {
                    self.queue.remove(index);
                }
                self.sync_queue();
            }
            DesktopIntent::MoveQueued { from, to } => {
                if from < self.queue.len() && to < self.queue.len() && from != to {
                    if let Some(item) = self.queue.remove(from) {
                        self.queue.insert(to, item);
                    }
                    self.sync_queue();
                }
            }
            DesktopIntent::SendQueuedNow { index } => {
                if let Some(item) = self
                    .queue
                    .get(index)
                    .cloned()
                    .and_then(|_| self.queue.remove(index))
                {
                    if self.state.runtime.current_phase == RunPhase::Idle
                        && matches!(
                            self.state.session.phase,
                            SessionPhase::Ready | SessionPhase::Running
                        )
                    {
                        self.state.runtime.queue_paused = false;
                        if let Err(error) = self.submit_now(item.clone()) {
                            self.queue.insert(index.min(self.queue.len()), item);
                            self.sync_queue();
                            return Err(error);
                        }
                    } else {
                        self.queue.push_front(item);
                    }
                    self.sync_queue();
                }
            }
            DesktopIntent::CancelRun => {
                if !matches!(
                    self.state.runtime.current_phase,
                    RunPhase::Waiting
                        | RunPhase::Thinking
                        | RunPhase::Streaming
                        | RunPhase::RunningTools
                        | RunPhase::WaitingForGate
                ) {
                    return Ok(());
                }
                self.send_command(UserCommand::Cancel)?;
                self.state.runtime.current_phase = RunPhase::Cancelling;
                self.publish(false);
            }
            DesktopIntent::RespondGate {
                session_epoch,
                request_id,
                response,
            } => {
                let response = match response {
                    GateResponse::Select(choice) => {
                        tact_protocol::UiResponse::Select { request_id, choice }
                    }
                    GateResponse::MultiSelect(choices) => tact_protocol::UiResponse::MultiSelect {
                        request_id,
                        choices,
                    },
                };
                if let Err(error) = self
                    .state
                    .reducer
                    .validate_gate_response(session_epoch, &response)
                {
                    if matches!(
                        error,
                        GateResponseError::Unknown
                            | GateResponseError::Stale
                            | GateResponseError::AlreadyAnswered
                    ) {
                        return Ok(());
                    }
                    return Err(format!("gate response rejected: {error:?}"));
                }
                self.send_command(UserCommand::UiResponse(response.clone()))?;
                let dirty = self
                    .state
                    .reducer
                    .apply_gate_response(session_epoch, &response)
                    .map_err(|error| format!("gate response rejected: {error:?}"))?;
                self.publish_rows(vec![dirty], SectionMask::TRANSCRIPT);
            }
            DesktopIntent::SetGateDraft {
                session_epoch,
                request_id,
                selected,
            } => {
                let dirty = self.state.reducer.set_gate_draft(
                    session_epoch,
                    RequestId(request_id),
                    selected,
                );
                if let Some(row) = dirty {
                    self.publish_rows(vec![row], SectionMask::TRANSCRIPT);
                }
            }
            DesktopIntent::InvokeCommand { id, args } => {
                self.invoke_command(id, args).await?;
            }
            DesktopIntent::RenameSession { session_id, title } => {
                let catalog = self
                    .catalog
                    .as_ref()
                    .ok_or_else(|| "no project open".to_string())?;
                catalog
                    .rename(&session_id.0, &title)
                    .await
                    .map_err(|error| error.to_string())?;
                self.refresh_sessions().await?;
            }
            DesktopIntent::SetSessionFilter(filter) => {
                self.state.set_filter(filter);
                self.refresh_sessions().await?;
            }
            DesktopIntent::SetSessionQuery(query) => {
                self.state.set_query(query);
                self.refresh_sessions().await?;
            }
            DesktopIntent::SetSessionArchived {
                session_id,
                archived,
            } => {
                let catalog = self
                    .catalog
                    .as_ref()
                    .ok_or_else(|| "no project open".to_string())?;
                catalog
                    .set_archived(&session_id.0, archived)
                    .await
                    .map_err(|error| error.to_string())?;
                self.refresh_sessions().await?;
            }
            DesktopIntent::RefreshSessions => self.refresh_sessions().await?,
            DesktopIntent::SetInspector(tab) => {
                self.state.set_inspector(tab);
                self.publish(false);
            }
            DesktopIntent::ToggleGroup { id } => {
                self.state.reducer.toggle_group(id);
                self.publish_rows(vec![id], SectionMask::TRANSCRIPT);
            }
            DesktopIntent::ToggleTool { group, tool } => {
                self.state.reducer.toggle_tool(group, tool);
                self.publish_rows(vec![group, tool], SectionMask::TRANSCRIPT);
            }
            DesktopIntent::OpenReader(reader) => self.open_reader(reader),
            DesktopIntent::SetTailFollowing(follow) => {
                self.state.tail_following = follow;
                self.publish(false);
            }
            DesktopIntent::BackToBottom => {
                self.state.tail_following = true;
                self.publish(false);
            }
            DesktopIntent::CancelSubagent { child_id } => {
                self.send_command(UserCommand::CancelSubagent { child_id })?;
            }
            DesktopIntent::CancelBackgroundTask { task_id } => {
                self.send_command(UserCommand::CancelBackgroundTask { task_id })?;
            }
            DesktopIntent::SetSkillEnabled { name, enabled } => {
                let host = self
                    .host
                    .as_ref()
                    .ok_or_else(|| "no active session".to_string())?;
                self.state.extensions.snapshot = host
                    .extension_host()
                    .set_skill_enabled(&name, enabled)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                self.publish(false);
            }
            DesktopIntent::SetSetting { id, value } => {
                self.set_setting(id, value).await?;
            }
            DesktopIntent::RetryProvider | DesktopIntent::RestartSession => {
                let initial = self
                    .state
                    .session
                    .id
                    .as_ref()
                    .map(|id| InitialSession::Resume(id.0.clone()))
                    .unwrap_or(InitialSession::ResumeLast);
                self.start_session(initial).await?;
            }
            DesktopIntent::Shutdown => {}
        }
        Ok(())
    }

    async fn open_project(
        &mut self,
        root: PathBuf,
        initial: InitialSession,
    ) -> Result<(), DesktopStartError> {
        if self.host.is_some()
            && let Err(error) = self.close_host().await
        {
            return Err(DesktopStartError::Runtime(format!(
                "active session did not close: {error}"
            )));
        }
        let project = self
            .registry
            .register(root)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        self.state.set_projects(self.registry.records());
        self.state.begin_session_switch();
        self.clear_queue();
        self.state.set_project(Some(&project));
        self.catalog = Some(SessionCatalog::new(project.clone()));
        self.project = Some(project.clone());
        self.refresh_sessions_inner().await?;
        if !project.available {
            self.state.session.phase = SessionPhase::Failed {
                reason: Arc::from(localized(
                    self.state.language,
                    "Project folder is unavailable",
                    "项目文件夹不可用",
                )),
            };
            self.state.runtime.current_phase = RunPhase::Failed;
            self.state.failure = Some(FailureView {
                kind: FailureKind::SessionOpen,
                message: Arc::from(localized(
                    self.state.language,
                    "Project folder is unavailable",
                    "项目文件夹不可用",
                )),
                retryable: true,
                retry_label: Some(Arc::from(localized(self.state.language, "Retry", "重试"))),
            });
            self.publish(true);
            return Ok(());
        }
        self.start_host(&project, initial).await?;
        self.publish(true);
        Ok(())
    }

    async fn start_session(&mut self, initial: InitialSession) -> Result<(), String> {
        let project = self
            .project
            .clone()
            .ok_or_else(|| "no project open".to_string())?;
        self.close_host()
            .await
            .map_err(|error| format!("active session did not close: {error}"))?;
        self.state.begin_session_switch();
        self.clear_queue();
        self.start_host(&project, initial)
            .await
            .map_err(start_error_message)?;
        self.publish(true);
        Ok(())
    }

    async fn start_host(
        &mut self,
        project: &ProjectRecord,
        initial: InitialSession,
    ) -> Result<(), DesktopStartError> {
        self.state.session.phase = SessionPhase::Starting;
        self.state.session.root_dir = Some(project.root.clone());
        let host = self
            .factory
            .start(project, &initial)
            .await
            .map_err(|error| {
                DesktopStartError::Runtime(host_start_message(&error, self.state.language))
            })?;
        self.state.session.id = Some(SessionId(host.session_id().to_string()));
        self.state.sessions.selected = self.state.session.id.clone();
        let session_id = host.session_id().to_string();
        self.host = Some(host);
        self.restore_failure = None;
        let Some(store) = self.catalog.as_ref().and_then(SessionCatalog::store_handle) else {
            let failure = FailureView {
                kind: FailureKind::Store,
                message: Arc::from(localized(
                    self.state.language,
                    "Session store is unavailable; the transcript cannot be restored.",
                    "会话存储不可用，无法恢复对话记录。",
                )),
                retryable: true,
                retry_label: Some(Arc::from(localized(self.state.language, "Retry", "重试"))),
            };
            self.restore_failure = Some(failure.clone());
            self.state.failure = Some(failure);
            self.state.session.phase = SessionPhase::Failed {
                reason: Arc::from(localized(
                    self.state.language,
                    "Session restore failed",
                    "会话恢复失败",
                )),
            };
            return Ok(());
        };
        match store.load_agent_session(&session_id).await {
            Ok(messages) => {
                self.state.reducer.seed_persisted(
                    &messages,
                    &crate::ReduceContext {
                        session_epoch: self.state.session.epoch,
                    },
                );
                if let Ok(history) = store.load_input_history(&session_id).await {
                    self.state.composer.history = history
                        .into_iter()
                        .rev()
                        .map(Arc::from)
                        .collect::<Vec<_>>()
                        .into();
                }
            }
            Err(error) => {
                let failure = FailureView {
                    kind: FailureKind::Store,
                    message: Arc::from(format!(
                        "{}: {error}",
                        localized(
                            self.state.language,
                            "Failed to restore the saved transcript",
                            "恢复已保存的对话记录失败",
                        )
                    )),
                    retryable: true,
                    retry_label: Some(Arc::from(localized(self.state.language, "Retry", "重试"))),
                };
                self.restore_failure = Some(failure.clone());
                self.state.failure = Some(failure);
                self.state.session.phase = SessionPhase::Failed {
                    reason: Arc::from(localized(
                        self.state.language,
                        "Session restore failed",
                        "会话恢复失败",
                    )),
                };
                return Ok(());
            }
        }
        self.refresh_sessions_inner().await?;
        if !self
            .state
            .sessions
            .items
            .iter()
            .any(|session| session.id == session_id)
        {
            self.state.sessions.items.insert(
                0,
                crate::SessionSummaryView {
                    id: session_id,
                    project_id: project.id.clone(),
                    root_dir: project.root.clone(),
                    title: localized(self.state.language, "New session", "新会话").into(),
                    updated_at: Utc::now(),
                    archived_at: None,
                    locked: true,
                    message_count: 0,
                    search_score: i32::MAX,
                },
            );
        }
        Ok(())
    }

    async fn refresh_sessions(&mut self) -> Result<(), String> {
        self.refresh_sessions_inner()
            .await
            .map_err(|error| format!("{error:?}"))
    }

    async fn refresh_sessions_inner(&mut self) -> Result<(), DesktopStartError> {
        if self.catalog.is_none() {
            self.state.sessions.items.clear();
            return Ok(());
        }
        self.state.sessions.loading = true;
        self.publish(false);
        let filter = self.state.sessions.filter;
        let query = self.state.sessions.query.clone();
        let result = self
            .catalog
            .as_ref()
            .expect("catalog checked above")
            .list(filter, &query)
            .await;
        match result {
            Ok(items) => self.state.set_sessions(items),
            Err(error) => {
                self.state.sessions.loading = false;
                self.state.sessions.error = Some(FailureView {
                    kind: FailureKind::Store,
                    message: Arc::from(error.to_string()),
                    retryable: true,
                    retry_label: Some(Arc::from(localized(self.state.language, "Refresh", "刷新"))),
                });
            }
        }
        self.publish(false);
        Ok(())
    }

    fn submit(&mut self, submission: Submission) -> Result<(), String> {
        if submission.text.trim().is_empty() {
            return Ok(());
        }
        if self.host.is_none() {
            self.queue.push_back(submission);
            self.sync_queue();
            return Ok(());
        }
        if !matches!(
            self.state.session.phase,
            SessionPhase::Ready | SessionPhase::Running
        ) || self.state.runtime.current_phase != RunPhase::Idle
        {
            self.queue.push_back(submission);
            self.sync_queue();
            return Ok(());
        }
        if let Err(error) = self.submit_now(submission.clone()) {
            self.queue.push_front(submission);
            self.sync_queue();
            return Err(error);
        }
        Ok(())
    }

    fn submit_now(&mut self, submission: Submission) -> Result<(), String> {
        if let Some(model) = submission.model.as_ref() {
            self.send_command(UserCommand::SetModel(model.clone()))?;
            self.config.host.model = Some(model.clone());
            self.state.composer.model = Arc::from(model.as_str());
        }
        if let Some(permission_mode) = submission.permission_mode {
            self.send_command(UserCommand::SetPermissionMode(match permission_mode {
                PermissionModeView::Default => PermissionMode::Default,
                PermissionModeView::Plan => PermissionMode::Plan,
                PermissionModeView::Auto => PermissionMode::Auto,
            }))?;
            self.config.host.permission_mode = permission_mode;
            self.state.composer.permission_mode = permission_mode;
        }
        let mut text = submission.text;
        for attachment in &submission.attachments {
            text.push_str(&format!(" @{}", attachment.display()));
        }
        self.send_command(UserCommand::SubmitTask(text.clone()))?;
        // Input history is per session and persisted by the same store as the
        // transcript. A failed write must not block the submitted turn.
        if let Some(store) = self.catalog.as_ref().and_then(SessionCatalog::store_handle)
            && let Some(session_id) = self.state.session.id.as_ref().map(|id| id.0.clone())
        {
            let text_for_history = text.clone();
            tokio::spawn(async move {
                let _ = store
                    .append_input_history(&session_id, &text_for_history)
                    .await;
            });
        }
        let mut history = Vec::with_capacity(self.state.composer.history.len() + 1);
        history.push(Arc::<str>::from(text.as_str()));
        history.extend(
            self.state
                .composer
                .history
                .iter()
                .filter(|entry| entry.as_ref() != text)
                .cloned(),
        );
        history.truncate(100);
        self.state.composer.history = history.into();
        let row = self
            .state
            .reducer
            .push_user(text.clone(), submission.attachments);
        self.state.runtime.current_phase = RunPhase::Waiting;
        self.state.session.phase = SessionPhase::Running;
        self.publish_rows(
            vec![row],
            SectionMask::TRANSCRIPT.union(SectionMask::SESSION),
        );
        Ok(())
    }

    async fn invoke_command(
        &mut self,
        id: CommandId,
        args: CommandArguments,
    ) -> Result<(), String> {
        match route_command(&id, &args) {
            CommandRoute::User(command) => self.send_command(command),
            CommandRoute::Surface(query) => {
                let request_id = self.request_id;
                self.request_id = self.request_id.saturating_add(1);
                self.send_command(UserCommand::QuerySurface { request_id, query })
            }
            CommandRoute::Reader(reader) => {
                self.open_reader(reader);
                Ok(())
            }
            CommandRoute::Skill { name, args } => {
                let host = self
                    .host
                    .as_ref()
                    .ok_or_else(|| "no active session".to_string())?;
                let registry = host.extension_host().skill_registry();
                let task = {
                    let registry = tact::skill::lock_skills(&registry);
                    let skill = registry
                        .skills()
                        .get(&name)
                        .ok_or_else(|| format!("unknown skill: {name}"))?;
                    tact::skill::format_skill_agent_task(skill, &args)
                };
                self.send_command(UserCommand::SubmitTask(task))
            }
            CommandRoute::Extension(action) => {
                let host = self
                    .host
                    .as_ref()
                    .ok_or_else(|| "no active session".to_string())?;
                let snapshot = host
                    .extension_host()
                    .plugin_action(action)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                self.state.extensions.snapshot = snapshot;
                self.publish(false);
                Ok(())
            }
            CommandRoute::CliProcess(args) => self.run_cli_process(args).await,
            CommandRoute::Error(message) => Err(message),
            CommandRoute::Shell(id) => self.run_shell_command(id).await,
            CommandRoute::CloseWindow => {
                let _ = self
                    .events
                    .urgent
                    .send(DesktopEvent::Effect(DesktopEffect::WindowCloseRequested));
                Ok(())
            }
            CommandRoute::Noop => Ok(()),
        }
    }

    async fn run_cli_process(&self, args: Vec<String>) -> Result<(), String> {
        let title = format!("tact-ui {}", args.join(" "));
        let program = tact_ui_binary_path();
        let mut command_args = self.desktop_cli_arguments();
        command_args.extend(args.clone());
        let working_dir = self
            .project
            .as_ref()
            .map(|project| project.root.clone())
            .or_else(|| self.state.session.root_dir.clone());
        let api_key = self.config.host.api_key.clone();
        let locale = self.state.language;
        let events = self.events.urgent.clone();
        std::thread::spawn(move || {
            let mut command = std::process::Command::new(program);
            if let Some(working_dir) = working_dir {
                command.current_dir(working_dir);
            }
            if let Some(api_key) = api_key {
                command.env("TACT_API_KEY", api_key);
            }
            let output = command.args(command_args).output();
            let markdown = cli_process_markdown(&title, locale, output);
            let _ = events.send(DesktopEvent::Effect(DesktopEffect::OpenReader(
                ReaderRequest {
                    title: Arc::from(title),
                    markdown: Arc::from(markdown),
                },
            )));
        });
        Ok(())
    }

    fn desktop_cli_arguments(&self) -> Vec<String> {
        let mut arguments = Vec::new();
        let host = &self.config.host;
        if let Some(path) = &host.config_path {
            push_cli_argument(&mut arguments, "--config", path.display().to_string());
        }
        if let Some(provider) = &host.provider {
            push_cli_argument(&mut arguments, "--provider", provider.clone());
        }
        if let Some(model) = &host.model {
            push_cli_argument(&mut arguments, "--model", model.clone());
        }
        if let Some(base_url) = &host.base_url {
            push_cli_argument(&mut arguments, "--base-url", base_url.clone());
        }
        if let Some(max_tokens) = host.max_tokens {
            push_cli_argument(&mut arguments, "--max-tokens", max_tokens.to_string());
        }
        if let Some(context_window) = host.model_context_window {
            push_cli_argument(
                &mut arguments,
                "--model-context-window",
                context_window.to_string(),
            );
        }
        if let Some(snapshot) = host.snapshot_max_items {
            push_cli_argument(&mut arguments, "--snapshot-max-items", snapshot.to_string());
        }
        if host.micro_compact == Some(false) {
            arguments.push("--no-micro-compact".into());
        }
        if host.tokio_console {
            arguments.push("--tokio-console".into());
        }
        if host.skill_body_auto_inject {
            arguments.push("--skill-body-auto-inject".into());
        }
        push_cli_argument(
            &mut arguments,
            "--permission-mode",
            match host.permission_mode {
                PermissionModeView::Default => "default",
                PermissionModeView::Plan => "plan",
                PermissionModeView::Auto => "auto",
            }
            .to_string(),
        );
        arguments
    }

    async fn run_shell_command(&mut self, id: CommandId) -> Result<(), String> {
        match id {
            CommandId::Builtin(tact_catalog::BuiltinCommand::Theme) => {
                self.state.appearance = match self.state.appearance {
                    Appearance::Light => Appearance::Dark,
                    Appearance::Dark => Appearance::Light,
                };
                self.config.appearance = self.state.appearance;
                let _ = self.config.save();
                self.publish(false);
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::Lang) => {
                self.state.language = match self.state.language {
                    tact_catalog::CatalogLocale::En => tact_catalog::CatalogLocale::Zh,
                    tact_catalog::CatalogLocale::Zh => tact_catalog::CatalogLocale::En,
                };
                self.config.language = self.state.language;
                self.config.save().map_err(|error| format!("{error:?}"))?;
                self.publish(false);
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::Save) => {
                let markdown = self.transcript_markdown();
                let _ =
                    self.events
                        .urgent
                        .send(DesktopEvent::Effect(DesktopEffect::SaveTranscript {
                            markdown,
                            suggested_name: "tact-session.md".to_string(),
                        }));
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::HookOutput) => {
                self.state.show_hook_output = !self.state.show_hook_output;
                self.config.show_hook_output = self.state.show_hook_output;
                let _ = self.config.save();
                self.publish(false);
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::ModelSubagent) => {
                let _ = self
                    .events
                    .urgent
                    .send(DesktopEvent::Effect(DesktopEffect::Notify {
                        title: localized(self.state.language, "Subagent model", "子代理模型")
                            .into(),
                        body: localized(
                            self.state.language,
                            "Choose a model in Settings; it applies to the next child.",
                            "请在设置中选择模型；它将应用于下一个子代理。",
                        )
                        .into(),
                    }));
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::TasksDag) => {
                self.open_reader(ReaderRef::TasksDag);
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::Background) => {
                self.open_reader(ReaderRef::Background(String::new()));
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::Skill) => {
                let host = self
                    .host
                    .as_ref()
                    .ok_or_else(|| "no active session".to_string())?;
                self.state.extensions.snapshot = host
                    .extension_host()
                    .reload_skills()
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                self.publish(false);
            }
            CommandId::Builtin(tact_catalog::BuiltinCommand::Plugin) => {
                let host = self
                    .host
                    .as_ref()
                    .ok_or_else(|| "no active session".to_string())?;
                self.state.extensions.snapshot = host
                    .extension_host()
                    .snapshot()
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                self.publish(false);
            }
            CommandId::Cli(tact_catalog::CliEntry::ResumeLast) => {
                self.start_session(InitialSession::ResumeLast).await?;
            }
            CommandId::Cli(tact_catalog::CliEntry::ListSessions) => {
                self.refresh_sessions().await?;
            }
            CommandId::Cli(
                tact_catalog::CliEntry::PluginList | tact_catalog::CliEntry::MarketplaceList,
            ) => {
                let host = self
                    .host
                    .as_ref()
                    .ok_or_else(|| "no active session".to_string())?;
                self.state.extensions.snapshot = host
                    .extension_host()
                    .snapshot()
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                self.publish(false);
            }
            CommandId::Cli(entry) => {
                return Err(format!(
                    "CLI entry `{}` has no executable desktop route",
                    entry.as_str()
                ));
            }
            CommandId::BuiltinPath { root, path } => match (root, path.as_slice()) {
                (tact_catalog::BuiltinCommand::Skill, ["skill", "reload"]) => {
                    let host = self
                        .host
                        .as_ref()
                        .ok_or_else(|| "no active session".to_string())?;
                    self.state.extensions.snapshot = host
                        .extension_host()
                        .reload_skills()
                        .await
                        .map_err(|error| format!("{error:?}"))?;
                    self.publish(false);
                }
                (tact_catalog::BuiltinCommand::Skill, ["skill", "list"])
                | (tact_catalog::BuiltinCommand::Plugin, _) => {
                    let host = self
                        .host
                        .as_ref()
                        .ok_or_else(|| "no active session".to_string())?;
                    self.state.extensions.snapshot = host
                        .extension_host()
                        .snapshot()
                        .await
                        .map_err(|error| format!("{error:?}"))?;
                    self.publish(false);
                }
                (root, path) => {
                    return Err(format!(
                        "builtin path `{}` has no executable desktop route",
                        std::iter::once(root.name())
                            .chain(path.iter().copied())
                            .collect::<Vec<_>>()
                            .join(" ")
                    ));
                }
            },
            CommandId::Setting(id) => {
                let _ = self
                    .events
                    .urgent
                    .send(DesktopEvent::Effect(DesktopEffect::Notify {
                        title: localized(self.state.language, "Settings", "设置").into(),
                        body: format!(
                            "{} {}",
                            localized(self.state.language, "Open settings for", "打开设置："),
                            id.label().get(self.state.language)
                        ),
                    }));
            }
            other => {
                return Err(format!(
                    "command `{other:?}` has no executable desktop route"
                ));
            }
        }
        Ok(())
    }

    async fn set_setting(&mut self, id: SettingId, value: SettingValue) -> Result<(), String> {
        if id == SettingId::PermissionMode {
            let valid = matches!(
                &value,
                SettingValue::Choice(value)
                    if matches!(value.as_str(), "default" | "plan" | "auto")
            );
            if !valid {
                return Err("invalid permission mode".into());
            }
        }
        if id == SettingId::ReasoningEffort {
            let valid = matches!(
                &value,
                SettingValue::Choice(value)
                    if matches!(value.as_str(), "low" | "medium" | "high" | "xhigh" | "max")
            );
            if !valid {
                return Err("invalid reasoning effort".into());
            }
        }
        let mut persist = false;
        match (id, &value) {
            (SettingId::Model, SettingValue::Text(model)) => {
                self.config.host.model = Some(model.clone());
                self.state.composer.model = Arc::from(model.clone());
                persist = true;
            }
            (SettingId::PermissionMode, SettingValue::Choice(mode)) => {
                self.config.host.permission_mode = match mode.as_str() {
                    "plan" => PermissionModeView::Plan,
                    "auto" => PermissionModeView::Auto,
                    _ => PermissionModeView::Default,
                };
                self.state.composer.permission_mode = self.config.host.permission_mode;
                persist = true;
            }
            (SettingId::ReasoningEffort, SettingValue::Choice(effort)) => {
                self.state.composer.effort = Some(Arc::from(effort.as_str()));
            }
            (SettingId::ThinkingBudget, SettingValue::Text(budget)) => {
                self.state.composer.thinking_budget = budget.parse().ok();
            }
            (SettingId::Provider, SettingValue::Text(value)) => {
                self.config.host.provider = Some(value.clone());
                persist = true;
            }
            (SettingId::ApiKey, SettingValue::Text(value)) => {
                self.config.host.api_key = Some(value.clone());
                persist = true;
            }
            (SettingId::BaseUrl, SettingValue::Text(value)) => {
                self.config.host.base_url = Some(value.clone());
                persist = true;
            }
            (SettingId::MaxTokens, SettingValue::Text(value)) => {
                self.config.host.max_tokens = Some(
                    value
                        .parse()
                        .map_err(|_| "max tokens must be a positive integer".to_string())?,
                );
                persist = true;
            }
            (SettingId::ModelContextWindow, SettingValue::Text(value)) => {
                self.config.host.model_context_window = Some(
                    value
                        .parse()
                        .map_err(|_| "context window must be an integer".to_string())?,
                );
                persist = true;
            }
            (SettingId::SnapshotMaxItems, SettingValue::Text(value)) => {
                self.config.host.snapshot_max_items = Some(
                    value
                        .parse()
                        .map_err(|_| "snapshot size must be an integer".to_string())?,
                );
                persist = true;
            }
            (SettingId::ConfigPath, SettingValue::Text(value)) => {
                self.config.host.config_path = Some(PathBuf::from(value));
                persist = true;
            }
            (SettingId::MicroCompact, SettingValue::Bool(value)) => {
                self.config.host.micro_compact = Some(*value);
                persist = true;
            }
            (SettingId::TokioConsole, SettingValue::Bool(value)) => {
                self.config.host.tokio_console = *value;
                persist = true;
            }
            (SettingId::SkillBodyAutoInject, SettingValue::Bool(value)) => {
                self.config.host.skill_body_auto_inject = *value;
                persist = true;
            }
            (SettingId::Notifications, SettingValue::Bool(value)) => {
                self.config.notifications = *value;
                self.state.notifications = *value;
                persist = true;
            }
            (SettingId::HookOutput, SettingValue::Bool(value)) => {
                self.config.show_hook_output = *value;
                self.state.show_hook_output = *value;
                persist = true;
            }
            _ => {}
        }
        if persist {
            self.config.save().map_err(|error| format!("{error:?}"))?;
        }
        let route = route_setting(id, &value);
        match route {
            CommandRoute::Noop => Err(format!("invalid value for setting `{}`", id.as_str())),
            CommandRoute::User(command) => {
                let result = self.send_command(command);
                self.publish(false);
                result
            }
            _ => {
                let restart_required = matches!(
                    id,
                    SettingId::Provider
                        | SettingId::ApiKey
                        | SettingId::BaseUrl
                        | SettingId::MaxTokens
                        | SettingId::ModelContextWindow
                        | SettingId::SnapshotMaxItems
                        | SettingId::MicroCompact
                        | SettingId::TokioConsole
                        | SettingId::SkillBodyAutoInject
                        | SettingId::ConfigPath
                );
                if matches!(id, SettingId::SubagentModel)
                    && let SettingValue::Text(model) = &value
                {
                    self.state.composer.subagent_model = Arc::from(model.as_str());
                    if !self
                        .state
                        .composer
                        .subagent_models
                        .iter()
                        .any(|candidate| candidate.as_ref() == model)
                    {
                        let mut models = self.state.composer.subagent_models.to_vec();
                        models.insert(0, Arc::from(model.as_str()));
                        self.state.composer.subagent_models = models.into();
                    }
                    self.config
                        .persist_subagent_model(model)
                        .map_err(|error| format!("{error:?}"))?;
                }
                if matches!(id, SettingId::HookOutput)
                    && let SettingValue::Bool(value) = value
                {
                    self.state.show_hook_output = value;
                }
                if matches!(id, SettingId::Theme)
                    && let SettingValue::Choice(value) = &value
                {
                    self.state.appearance = if value == "dark" {
                        Appearance::Dark
                    } else {
                        Appearance::Light
                    };
                    self.config.appearance = self.state.appearance;
                    let _ = self.config.save();
                }
                if matches!(id, SettingId::Language)
                    && let SettingValue::Choice(value) = &value
                {
                    self.state.language = if value == "zh" {
                        tact_catalog::CatalogLocale::Zh
                    } else {
                        tact_catalog::CatalogLocale::En
                    };
                    self.config.language = self.state.language;
                    let _ = self.config.save();
                }
                let _ = self
                    .events
                    .urgent
                    .send(DesktopEvent::Effect(DesktopEffect::Notify {
                        title: localized(self.state.language, "Settings", "设置").into(),
                        body: if restart_required {
                            localized(self.state.language, "Requires restart", "需要重启")
                                .to_string()
                        } else {
                            localized(self.state.language, "Updated", "已更新").to_string()
                        },
                    }));
                self.publish(false);
                Ok(())
            }
        }
    }

    fn open_reader(&self, reader: ReaderRef) {
        let request = self.reader_request(reader);
        let _ = self
            .events
            .urgent
            .send(DesktopEvent::Effect(DesktopEffect::OpenReader(request)));
    }

    fn reader_request(&self, reader: ReaderRef) -> ReaderRequest {
        let view = self.state.reducer.view();
        match reader {
            ReaderRef::ToolOutput(row) | ReaderRef::Diff(row) | ReaderRef::Thinking(row) => {
                for item in view.iter() {
                    match (&item.body, item.id == row) {
                        (crate::RowBody::Thinking(thinking), true) => {
                            return ReaderRequest {
                                title: Arc::from(localized(
                                    self.state.language,
                                    "Thinking",
                                    "思考",
                                )),
                                markdown: thinking.markdown.clone(),
                            };
                        }
                        (crate::RowBody::Diff(diff), true) => {
                            return ReaderRequest {
                                title: Arc::from(diff.path.as_deref().unwrap_or(localized(
                                    self.state.language,
                                    "Diff",
                                    "差异",
                                ))),
                                markdown: diff.patch.clone(),
                            };
                        }
                        (crate::RowBody::ToolGroup(group), _) => {
                            if let Some(tool) = group.items.iter().find_map(|item| match item {
                                crate::ToolGroupItem::Tool(tool) if tool.id == row => Some(tool),
                                _ => None,
                            }) {
                                return ReaderRequest {
                                    title: Arc::from(tool.name.as_ref()),
                                    markdown: tool.output.detail.clone().unwrap_or_default(),
                                };
                            }
                        }
                        _ => {}
                    }
                }
                ReaderRequest {
                    title: Arc::from(localized(self.state.language, "Reader", "阅读器")),
                    markdown: Arc::from(localized(
                        self.state.language,
                        "Nothing to show.",
                        "没有可显示的内容。",
                    )),
                }
            }
            ReaderRef::Help => {
                let primary = if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Ctrl"
                };
                let option = if cfg!(target_os = "macos") {
                    "Option"
                } else {
                    "Alt"
                };
                let title = localized(self.state.language, "Keyboard Shortcuts", "键盘快捷键");
                let markdown = match self.state.language {
                    CatalogLocale::En => format!(
                        "# Keyboard Shortcuts\n\n- {primary}+K Command Center\n- {primary}+B Toggle sessions\n- {primary}+Alt+B Toggle inspector\n- {primary}+Enter Send\n- Esc Close/cancel\n- {primary}+. Stop\n- {primary}+1..4 Inspector tabs\n- {option}+Up/Down Queue navigation\n- Space Toggle multi-select\n- Enter Activate focused control\n- Tab / Shift+Tab Traverse controls"
                    ),
                    CatalogLocale::Zh => format!(
                        "# 键盘快捷键\n\n- {primary}+K 命令中心\n- {primary}+B 切换会话栏\n- {primary}+Alt+B 切换检查器\n- {primary}+Enter 发送\n- Esc 关闭/取消\n- {primary}+. 停止\n- {primary}+1..4 检查器标签\n- {option}+Up/Down 队列导航\n- Space 切换多选项\n- Enter 激活当前控件\n- Tab / Shift+Tab 遍历控件"
                    ),
                };
                ReaderRequest {
                    title: Arc::from(title),
                    markdown: Arc::from(markdown),
                }
            }
            ReaderRef::Stats => ReaderRequest {
                title: Arc::from(localized(
                    self.state.language,
                    "Session Statistics",
                    "会话统计",
                )),
                markdown: Arc::from(format!(
                    "{}\n\n{}: {}\n{}: {}",
                    localized(self.state.language, "# Session Statistics", "# 会话统计"),
                    localized(self.state.language, "Turns", "轮次"),
                    self.state.runtime.turns_taken,
                    localized(self.state.language, "Tokens", "Token"),
                    self.state.runtime.usage.total_tokens
                )),
            },
            ReaderRef::TasksDag => {
                let mut markdown = format!(
                    "{}\n\n",
                    localized(self.state.language, "# Task DAG", "# 任务 DAG")
                );
                if self.state.tasks.is_empty() {
                    markdown.push_str(localized(
                        self.state.language,
                        "No tasks in this session.",
                        "此会话中没有任务。",
                    ));
                } else {
                    for task in &self.state.tasks {
                        markdown.push_str(&format!(
                            "- **#{} {}** — {}, {}: {}\n",
                            task.id,
                            task.subject,
                            task_status_label(self.state.language, task.status),
                            localized(self.state.language, "owner", "负责人"),
                            task.owner
                        ));
                        if !task.blocked_by.is_empty() {
                            markdown.push_str(&format!(
                                "  - {}: {}\n",
                                localized(self.state.language, "blocked by", "阻塞于"),
                                task.blocked_by
                                    .iter()
                                    .map(ToString::to_string)
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    }
                }
                ReaderRequest {
                    title: Arc::from(localized(self.state.language, "Task DAG", "任务 DAG")),
                    markdown: Arc::from(markdown),
                }
            }
            ReaderRef::History => ReaderRequest {
                title: Arc::from(localized(self.state.language, "History", "历史")),
                markdown: Arc::from(match &self.state.surface {
                    Some(tact_protocol::SurfaceSnapshot::History(entries)) => {
                        let mut markdown = format!(
                            "{}\n\n",
                            localized(self.state.language, "# History", "# 历史")
                        );
                        for entry in entries {
                            markdown.push_str(&format!(
                                "- **{}** ({}) — {}\n",
                                entry.task, entry.timestamp, entry.summary
                            ));
                        }
                        markdown
                    }
                    _ => localized(
                        self.state.language,
                        "History is loading. Run /history again.",
                        "历史正在加载，请再次运行 /history。",
                    )
                    .to_string(),
                }),
            },
            ReaderRef::Balance => ReaderRequest {
                title: Arc::from(localized(self.state.language, "Account", "账户")),
                markdown: Arc::from(match &self.state.account {
                    Some(AccountView::Balance { currency, total }) => {
                        format!("{total} {currency}")
                    }
                    Some(AccountView::UsageQuota { .. }) => {
                        localized(self.state.language, "Usage quota available", "用量配额可用")
                            .to_string()
                    }
                    Some(AccountView::Error(error)) => error.to_string(),
                    None => localized(
                        self.state.language,
                        "No account snapshot available.",
                        "暂无账户快照。",
                    )
                    .to_string(),
                }),
            },
            ReaderRef::SystemPrompt => ReaderRequest {
                title: Arc::from(localized(self.state.language, "System prompt", "系统提示")),
                markdown: Arc::from(match &self.state.surface {
                    Some(tact_protocol::SurfaceSnapshot::SystemPrompt(prompt)) => prompt.clone(),
                    _ => localized(
                        self.state.language,
                        "System prompt is loading. Run /view-system-prompt again.",
                        "系统提示正在加载，请再次运行 /view-system-prompt。",
                    )
                    .to_string(),
                }),
            },
            ReaderRef::Subagent(child_id) => {
                let summary = self
                    .state
                    .subagents
                    .iter()
                    .find(|agent| agent.child_id.as_ref() == child_id)
                    .map(|agent| {
                        format!(
                            "# {} {}\n\n- {}: {}\n- {}: {} ms\n- {}: {}\n",
                            localized(self.state.language, "Subagent", "子代理"),
                            agent.child_id,
                            localized(self.state.language, "Status", "状态"),
                            subagent_status_label(self.state.language, agent.status),
                            localized(self.state.language, "Elapsed", "耗时"),
                            agent.elapsed_ms,
                            localized(self.state.language, "Summary", "摘要"),
                            agent.summary
                        )
                    })
                    .unwrap_or_else(|| {
                        format!(
                            "# {} {child_id}\n\n{}",
                            localized(self.state.language, "Subagent", "子代理"),
                            localized(
                                self.state.language,
                                "No status snapshot available.",
                                "暂无状态快照。"
                            )
                        )
                    });
                ReaderRequest {
                    title: Arc::from(format!(
                        "{} {child_id}",
                        localized(self.state.language, "Subagent", "子代理")
                    )),
                    markdown: Arc::from(summary),
                }
            }
            ReaderRef::Background(task_id) => {
                let markdown = if task_id.is_empty() {
                    let mut markdown = format!(
                        "{}\n\n",
                        localized(self.state.language, "# Background tasks", "# 后台任务")
                    );
                    if self.state.background.is_empty() {
                        markdown.push_str(localized(
                            self.state.language,
                            "No background tasks are running.",
                            "当前没有运行中的后台任务。",
                        ));
                    } else {
                        for task in &self.state.background {
                            markdown.push_str(&format!(
                                "- **{}** `{}` — {}\n",
                                task.id,
                                task.command,
                                background_status_label(self.state.language, task.status)
                            ));
                        }
                    }
                    markdown
                } else {
                    self.state
                        .background
                        .iter()
                        .find(|task| task.id.as_ref() == task_id)
                        .map(|task| {
                            format!(
                                "# {} {}\n\n- {}: `{}`\n- {}: {}\n- {}: {}\n- {}: {}\n- {}: {}\n\n```text\n{}\n```",
                                localized(self.state.language, "Background", "后台"),
                                task.id,
                                localized(self.state.language, "Command", "命令"),
                                task.command,
                                localized(self.state.language, "Status", "状态"),
                                background_status_label(self.state.language, task.status),
                                localized(self.state.language, "Started", "开始时间"),
                                task.started_at,
                                localized(self.state.language, "Finished", "结束时间"),
                                task.finished_at
                                    .map(|timestamp| timestamp.to_string())
                                    .unwrap_or_else(|| "—".to_string()),
                                localized(self.state.language, "Output path", "输出路径"),
                                task.output_path.as_deref().unwrap_or("—"),
                                task.output_tail
                            )
                        })
                        .unwrap_or_else(|| {
                            format!(
                                "# {} {task_id}\n\n{}",
                                localized(self.state.language, "Background", "后台"),
                                localized(
                                    self.state.language,
                                    "No task snapshot available.",
                                    "暂无任务快照。"
                                )
                            )
                        })
                };
                ReaderRequest {
                    title: Arc::from(localized(
                        self.state.language,
                        "Background task",
                        "后台任务",
                    )),
                    markdown: Arc::from(markdown),
                }
            }
        }
    }

    fn transcript_markdown(&self) -> String {
        let mut markdown = String::new();
        for row in self.state.reducer.view().iter() {
            match &row.body {
                crate::RowBody::User(user) => {
                    markdown.push_str(&format!("# User\n\n{}\n\n", user.text));
                }
                crate::RowBody::Assistant(assistant) => {
                    markdown.push_str(&format!("# Assistant\n\n{}\n\n", assistant.markdown));
                }
                crate::RowBody::Thinking(thinking) => {
                    markdown.push_str(&format!(
                        "<details><summary>Thinking</summary>\n\n{}\n\n</details>\n\n",
                        thinking.markdown
                    ));
                }
                crate::RowBody::ToolGroup(group) => {
                    markdown.push_str("## Tools\n\n");
                    for item in group.items.iter() {
                        if let crate::ToolGroupItem::Tool(tool) = item {
                            markdown.push_str(&format!(
                                "- `{}` {}: {}\n",
                                tool.name, tool.arg_summary, tool.output.preview
                            ));
                        }
                    }
                    markdown.push('\n');
                }
                _ => {}
            }
        }
        markdown
    }

    fn sync_queue(&mut self) {
        self.state.composer.queue = self
            .queue
            .iter()
            .enumerate()
            .map(|(index, item)| crate::QueuedPromptView {
                id: index as u64,
                text: Arc::from(item.text.clone()),
                attachments: item.attachments.clone().into(),
            })
            .collect::<Vec<_>>()
            .into();
        self.publish(false);
    }

    fn clear_queue(&mut self) {
        self.queue.clear();
        self.state.composer.queue = Arc::from([]);
    }

    async fn drain_host_events(&mut self) {
        let mut agent_updates = Vec::new();
        let mut account_updates = Vec::new();
        let mut force_publish = false;
        let mut run_finished = false;
        let mut run_cancelled = false;
        let mut run_failed = false;
        let mut became_ready = false;
        let mut gates_hinted = false;
        let mut drained = 0usize;
        loop {
            if drained >= MAX_HOST_EVENTS_PER_DRAIN {
                break;
            }
            drained += 1;
            let event = match self.host.as_mut().map(|host| host.try_next_event()) {
                Some(TryHostEvent::Available(event)) => event,
                Some(TryHostEvent::Empty) | None => break,
                Some(TryHostEvent::Disconnected) => {
                    if !matches!(self.state.session.phase, SessionPhase::Failed { .. }) {
                        let reason = localized(
                            self.state.language,
                            "Host event channel closed",
                            "宿主事件通道已关闭",
                        )
                        .to_string();
                        self.mark_disconnected(&reason);
                    }
                    self.publish(true);
                    break;
                }
            };
            match event {
                HostEvent::Agent(update) => {
                    let update = *update;
                    gates_hinted |= matches!(
                        &update,
                        tact_protocol::AgentUpdate::RequestSelect { .. }
                            | tact_protocol::AgentUpdate::RequestMultiSelect { .. }
                    );
                    if let tact_protocol::AgentUpdate::PopupMarkdown { title, source } = &update {
                        let _ = self.events.urgent.send(DesktopEvent::Effect(
                            DesktopEffect::OpenReader(ReaderRequest {
                                title: Arc::from(title.as_str()),
                                markdown: Arc::from(source.as_str()),
                            }),
                        ));
                    }
                    run_failed |= matches!(&update, tact_protocol::AgentUpdate::Error(_));
                    run_finished |= matches!(
                        &update,
                        tact_protocol::AgentUpdate::RunFinished { .. }
                            | tact_protocol::AgentUpdate::TaskComplete(_)
                            | tact_protocol::AgentUpdate::TaskIdle
                    );
                    run_cancelled |= matches!(&update, tact_protocol::AgentUpdate::TaskCancelled);
                    agent_updates.push(update);
                }
                HostEvent::Account(update) => account_updates.push(update),
                HostEvent::Ready => {
                    if let Some(failure) = self.restore_failure.clone() {
                        self.state.failure = Some(failure);
                        self.state.session.phase = SessionPhase::Failed {
                            reason: Arc::from(localized(
                                self.state.language,
                                "Session restore failed",
                                "会话恢复失败",
                            )),
                        };
                        self.state.runtime.current_phase = RunPhase::Failed;
                    } else {
                        self.state.session.phase = SessionPhase::Ready;
                        self.state.runtime.current_phase = RunPhase::Idle;
                        self.state.failure = None;
                    }
                    let mode = match self.config.host.permission_mode {
                        PermissionModeView::Default => PermissionMode::Default,
                        PermissionModeView::Plan => PermissionMode::Plan,
                        PermissionModeView::Auto => PermissionMode::Auto,
                    };
                    let _ = self.send_command(UserCommand::SetPermissionMode(mode));
                    let _ = self.send_command(UserCommand::SyncBackgroundTasks);
                    became_ready = self.restore_failure.is_none();
                    force_publish = true;
                }
                HostEvent::StartupFailed(failure) => {
                    self.state.session.phase = SessionPhase::Failed {
                        reason: Arc::from(format!("{failure:?}")),
                    };
                    self.state.failure = Some(FailureView::from_host(failure, self.state.language));
                    self.state.runtime.current_phase = RunPhase::Failed;
                    force_publish = true;
                }
                HostEvent::Disconnected => {
                    if !matches!(self.state.session.phase, SessionPhase::Failed { .. }) {
                        let reason =
                            localized(self.state.language, "Runtime disconnected", "运行时已断开")
                                .to_string();
                        self.mark_disconnected(&reason);
                    }
                    force_publish = true;
                }
            }
        }
        let had_agent_updates = !agent_updates.is_empty();
        let mut dirty = if agent_updates.is_empty() {
            Vec::new()
        } else {
            self.state.apply_agent_batch(agent_updates)
        };
        // Typed snapshots and runtime-only updates do not create transcript
        // rows. Publish them even when the reducer has no dirty row so the
        // shell sees the new surface/runtime state.
        if had_agent_updates && dirty.is_empty() {
            force_publish = true;
        }
        if gates_hinted {
            self.reconcile_gate_snapshot_now();
        }
        if !account_updates.is_empty() {
            for update in account_updates {
                self.state.apply_account_update(update);
            }
            force_publish = true;
        }
        if run_finished || run_cancelled || run_failed {
            self.state.runtime.current_phase = if run_failed {
                RunPhase::Failed
            } else {
                RunPhase::Idle
            };
            self.state.session.phase = SessionPhase::Ready;
            self.state.runtime.queue_paused |= run_cancelled;
        }
        if became_ready
            && let Some(extension_host) = self.host.as_ref().map(|host| host.extension_host())
            && let Ok(snapshot) = extension_host.snapshot().await
        {
            self.state.extensions.snapshot = snapshot;
            force_publish = true;
        }
        if !dirty.is_empty() || force_publish {
            let sections = if force_publish {
                SectionMask::SESSION
                    .union(SectionMask::TRANSCRIPT)
                    .union(SectionMask::INSPECTOR)
                    .union(SectionMask::GATES)
                    .union(SectionMask::COMMANDS)
                    .union(SectionMask::EXTENSIONS)
                    .union(SectionMask::FAILURE)
            } else {
                SectionMask::TRANSCRIPT
            };
            self.publish_rows(std::mem::take(&mut dirty), sections);
        }
        if run_finished && !run_cancelled && !run_failed {
            self.flush_queue();
        }
        if became_ready {
            self.flush_queue();
        }
        if run_finished {
            let _ = self.refresh_sessions().await;
        }
        self.reconcile_gate_snapshot();
    }

    fn flush_queue(&mut self) {
        if self.host.is_none()
            || self.state.runtime.current_phase != RunPhase::Idle
            || self.state.runtime.queue_paused
            || !matches!(
                self.state.session.phase,
                SessionPhase::Ready | SessionPhase::Running
            )
        {
            return;
        }
        let Some(submission) = self.queue.pop_front() else {
            return;
        };
        self.sync_queue();
        if let Err(error) = self.submit_now(submission.clone()) {
            self.queue.push_front(submission);
            self.sync_queue();
            self.set_failure(error, true);
        }
    }

    async fn close_host(&mut self) -> Result<(), HostCloseError> {
        if self.host.is_none() {
            return Ok(());
        }
        self.state.session.phase = SessionPhase::Closing;
        self.publish(false);
        let result = self
            .host
            .as_mut()
            .expect("host checked above")
            .close()
            .await;
        match result {
            Ok(_) => {
                self.host = None;
                self.state.session.phase = SessionPhase::Empty;
                self.publish(false);
                Ok(())
            }
            Err(HostCloseError::DrainTimedOut) => {
                self.state.failure = Some(FailureView {
                    kind: FailureKind::DriverClosed,
                    message: Arc::from(localized(
                        self.state.language,
                        "Waiting for session to stop",
                        "正在等待会话停止",
                    )),
                    retryable: true,
                    retry_label: Some(Arc::from(localized(self.state.language, "Retry", "重试"))),
                });
                self.state.session.phase = SessionPhase::Closing;
                self.publish(true);
                Err(HostCloseError::DrainTimedOut)
            }
            Err(error) => {
                self.state.failure = Some(FailureView {
                    kind: FailureKind::DriverClosed,
                    message: Arc::from(error.to_string()),
                    retryable: true,
                    retry_label: Some(Arc::from(localized(self.state.language, "Retry", "重试"))),
                });
                self.publish(true);
                Err(error)
            }
        }
    }

    fn send_command(&self, command: UserCommand) -> Result<(), String> {
        match self.host.as_ref() {
            Some(host) => host.send(command).map_err(|error| format!("{error:?}")),
            None => Err("no active session".to_string()),
        }
    }

    fn reconcile_gate_snapshot(&mut self) {
        self.reconcile_gate_snapshot_with(false);
    }

    fn reconcile_gate_snapshot_now(&mut self) {
        self.reconcile_gate_snapshot_with(true);
    }

    fn reconcile_gate_snapshot_with(&mut self, force: bool) {
        if !force && self.last_gate_reconcile.elapsed() < self.config.gate_reconcile {
            return;
        }
        self.last_gate_reconcile = Instant::now();
        let Some(host) = self.host.as_ref() else {
            return;
        };
        let pending = host.pending_requests();
        let context = crate::ReduceContext {
            session_epoch: self.state.session.epoch,
        };
        let delta = self.state.reducer.reconcile_gates(&pending, &context);
        if !delta.dirty_rows.is_empty() {
            self.publish_rows(delta.dirty_rows, SectionMask::TRANSCRIPT);
        }
    }

    fn set_failure(&mut self, message: impl Into<Arc<str>>, retryable: bool) {
        let message: Arc<str> = message.into();
        if matches!(
            self.state.session.phase,
            SessionPhase::Opening | SessionPhase::Starting | SessionPhase::Switching
        ) {
            self.state.session.phase = SessionPhase::Failed {
                reason: message.clone(),
            };
            self.state.runtime.current_phase = RunPhase::Failed;
        }
        self.state.failure = Some(FailureView {
            kind: FailureKind::Other,
            message,
            retryable,
            retry_label: retryable
                .then(|| Arc::from(localized(self.state.language, "Retry", "重试"))),
        });
        self.publish(false);
    }

    fn mark_disconnected(&mut self, reason: &str) {
        self.state.session.phase = SessionPhase::Disconnected {
            reason: Arc::from(reason),
        };
        self.state.runtime.current_phase = RunPhase::Failed;
        self.state.failure = Some(FailureView {
            kind: FailureKind::RuntimeDisconnected,
            message: Arc::from(reason),
            retryable: true,
            retry_label: Some(Arc::from(localized(self.state.language, "Retry", "重试"))),
        });
    }

    fn publish_rows(&mut self, dirty: Vec<crate::RowId>, sections: SectionMask) {
        let commit = self.state.commit(dirty, sections);
        self.write_commit(commit);
    }

    fn publish(&mut self, force: bool) {
        let sections = if force {
            SectionMask::SESSION
                .union(SectionMask::TRANSCRIPT)
                .union(SectionMask::INSPECTOR)
                .union(SectionMask::GATES)
                .union(SectionMask::COMMANDS)
                .union(SectionMask::EXTENSIONS)
                .union(SectionMask::FAILURE)
        } else {
            SectionMask::default()
        };
        self.publish_rows(Vec::new(), sections);
    }

    fn write_commit(&self, commit: StateCommit) {
        *self
            .snapshot
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = commit.snapshot.clone();
        self.events.commits.send_replace(Some(Arc::new(commit)));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use async_trait::async_trait;
    use tact::store::ArchiveFilter;
    use tact_catalog::SettingId;
    use tact_host::{
        HostCloseError, HostEvent, HostExit, HostExtensionService, HostSendError, HostStartError,
        TryHostEvent,
    };
    use tact_protocol::{AgentErrorKind, AgentUpdate, PermissionMode, UserCommand};

    use super::*;
    use crate::{GateResponse, InitialSession, SettingValue, Submission};

    struct FakeFactory {
        log: Arc<Mutex<Vec<String>>>,
        commands: Arc<Mutex<Vec<UserCommand>>>,
        close_results: Mutex<VecDeque<Result<(), HostCloseError>>>,
        startup_failure: bool,
        emit_ready: bool,
        start_failure: Option<HostStartError>,
        host_events: Arc<Mutex<Vec<mpsc::UnboundedSender<HostEvent>>>>,
        pending: Arc<Mutex<Vec<tact::ui_responder::PendingUiRequest>>>,
    }

    impl FakeFactory {
        fn new() -> Arc<Self> {
            Arc::new(Self {
                log: Arc::new(Mutex::new(Vec::new())),
                commands: Arc::new(Mutex::new(Vec::new())),
                close_results: Mutex::new(VecDeque::new()),
                startup_failure: false,
                emit_ready: true,
                start_failure: None,
                host_events: Arc::new(Mutex::new(Vec::new())),
                pending: Arc::new(Mutex::new(Vec::new())),
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                log: Arc::new(Mutex::new(Vec::new())),
                commands: Arc::new(Mutex::new(Vec::new())),
                close_results: Mutex::new(VecDeque::new()),
                startup_failure: true,
                emit_ready: true,
                start_failure: None,
                host_events: Arc::new(Mutex::new(Vec::new())),
                pending: Arc::new(Mutex::new(Vec::new())),
            })
        }

        fn deferred_ready() -> Arc<Self> {
            Arc::new(Self {
                log: Arc::new(Mutex::new(Vec::new())),
                commands: Arc::new(Mutex::new(Vec::new())),
                close_results: Mutex::new(VecDeque::new()),
                startup_failure: false,
                emit_ready: false,
                start_failure: None,
                host_events: Arc::new(Mutex::new(Vec::new())),
                pending: Arc::new(Mutex::new(Vec::new())),
            })
        }

        fn start_failing() -> Arc<Self> {
            Arc::new(Self {
                log: Arc::new(Mutex::new(Vec::new())),
                commands: Arc::new(Mutex::new(Vec::new())),
                close_results: Mutex::new(VecDeque::new()),
                startup_failure: false,
                emit_ready: false,
                start_failure: Some(HostStartError::Lock("already locked".into())),
                host_events: Arc::new(Mutex::new(Vec::new())),
                pending: Arc::new(Mutex::new(Vec::new())),
            })
        }

        fn with_close_results(
            results: impl IntoIterator<Item = Result<(), HostCloseError>>,
        ) -> Arc<Self> {
            Arc::new(Self {
                log: Arc::new(Mutex::new(Vec::new())),
                commands: Arc::new(Mutex::new(Vec::new())),
                close_results: Mutex::new(results.into_iter().collect()),
                startup_failure: false,
                emit_ready: true,
                start_failure: None,
                host_events: Arc::new(Mutex::new(Vec::new())),
                pending: Arc::new(Mutex::new(Vec::new())),
            })
        }

        fn emit(&self, event: HostEvent) {
            let events = self.host_events.lock().unwrap();
            events.last().expect("host started").send(event).unwrap();
        }
    }

    struct FakeHost {
        id: String,
        events: mpsc::UnboundedReceiver<HostEvent>,
        _events_tx: mpsc::UnboundedSender<HostEvent>,
        log: Arc<Mutex<Vec<String>>>,
        commands: Arc<Mutex<Vec<UserCommand>>>,
        close_results: Arc<Mutex<VecDeque<Result<(), HostCloseError>>>>,
        extension_host: HostExtensionService,
        pending: Arc<Mutex<Vec<tact::ui_responder::PendingUiRequest>>>,
    }

    #[async_trait]
    impl HostBackend for FakeHost {
        fn session_id(&self) -> &str {
            &self.id
        }

        fn try_next_event(&mut self) -> TryHostEvent {
            match self.events.try_recv() {
                Ok(event) => TryHostEvent::Available(event),
                Err(mpsc::error::TryRecvError::Empty) => TryHostEvent::Empty,
                Err(mpsc::error::TryRecvError::Disconnected) => TryHostEvent::Disconnected,
            }
        }

        fn send(&self, command: UserCommand) -> Result<(), HostSendError> {
            self.commands.lock().unwrap().push(command);
            Ok(())
        }

        fn pending_requests(&self) -> Vec<tact::ui_responder::PendingUiRequest> {
            self.pending.lock().unwrap().clone()
        }

        fn extension_host(&self) -> HostExtensionService {
            self.extension_host.clone()
        }

        async fn close(&mut self) -> Result<HostExit, HostCloseError> {
            self.log.lock().unwrap().push(format!("close:{}", self.id));
            let result = self
                .close_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()));
            result.map(|()| HostExit {
                session_id: self.id.clone(),
                cancelled: true,
            })
        }
    }

    #[async_trait]
    impl HostFactory for FakeFactory {
        async fn start(
            &self,
            project: &ProjectRecord,
            _initial: &InitialSession,
        ) -> Result<Box<dyn HostBackend>, HostStartError> {
            if let Some(error) = &self.start_failure {
                return Err(error.clone());
            }
            let id = format!("session-{}", self.log.lock().unwrap().len());
            self.log
                .lock()
                .unwrap()
                .push(format!("open:{}:{id}", project.root.display()));
            let (tx, rx) = mpsc::unbounded_channel();
            let event = if self.startup_failure {
                HostEvent::StartupFailed(tact_host::HostFailure::Bootstrap("no provider".into()))
            } else {
                HostEvent::Ready
            };
            if self.emit_ready {
                tx.send(event).unwrap();
            }
            self.host_events.lock().unwrap().push(tx.clone());
            let skill_registry =
                tact::skill::shared_skill_registry(&project.root).expect("skill registry");
            Ok(Box::new(FakeHost {
                id,
                events: rx,
                _events_tx: tx,
                log: self.log.clone(),
                commands: self.commands.clone(),
                close_results: Arc::new(Mutex::new(std::mem::take(
                    &mut *self.close_results.lock().unwrap(),
                ))),
                extension_host: HostExtensionService::new(
                    project.root.clone(),
                    skill_registry,
                    None,
                ),
                pending: self.pending.clone(),
            }))
        }
    }

    async fn wait_ready(controller: &DesktopController) {
        for _ in 0..100 {
            controller.when_idle().await.expect("idle");
            if matches!(
                controller.snapshot().session.phase,
                SessionPhase::Ready | SessionPhase::Failed { .. }
            ) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let snapshot = controller.snapshot();
        panic!(
            "controller never became ready: phase={:?} failure={:?}",
            snapshot.session.phase, snapshot.failure
        );
    }

    fn config(root: &std::path::Path) -> DesktopConfig {
        DesktopConfig::load(root.join("app-data"))
            .unwrap()
            .with_initial_project(root.join("project"))
            .with_initial_session(InitialSession::New)
    }

    fn config_without_project(root: &std::path::Path) -> DesktopConfig {
        DesktopConfig::load(root.join("app-data"))
            .unwrap()
            .with_initial_session(InitialSession::New)
    }

    fn make_project(root: &std::path::Path) {
        std::fs::create_dir_all(root.join("project/.tact")).unwrap();
        std::fs::create_dir_all(root.join("second/.tact")).unwrap();
    }

    fn pending_select(request_id: u64) -> tact::ui_responder::PendingUiRequest {
        tact::ui_responder::PendingUiRequest {
            request_id,
            prompt: "Allow?".into(),
            options: vec!["Allow".into()],
            kind: tact::ui_responder::PendingUiRequestKind::Select,
            log_confirm: false,
            from_subagent: false,
            purpose: tact_protocol::UiRequestPurpose::Permission,
            origin: tact_protocol::UiRequestOrigin::main(),
        }
    }

    fn write_skill(root: &std::path::Path, name: &str) {
        let dir = root.join(".tact/skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {name} skill\n---\n\n# {name}\n"),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn project_switch_closes_before_opening() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;

        controller
            .dispatch(DesktopIntent::OpenProject {
                root: tmp.path().join("second"),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;

        let log = factory.log.lock().unwrap();
        let close = log
            .iter()
            .position(|entry| entry.starts_with("close:"))
            .unwrap();
        let second_open = log
            .iter()
            .rposition(|entry| entry.contains("/second:"))
            .unwrap();
        assert!(close < second_open, "{log:?}");
    }

    #[tokio::test]
    async fn session_epoch_changes_on_switch() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        let epoch = controller.snapshot().session.epoch;
        controller.dispatch(DesktopIntent::NewSession).unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;
        assert!(controller.snapshot().session.epoch > epoch);
    }

    #[tokio::test]
    async fn shutdown_joins_host_and_stops_event_pump() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller.shutdown().await.expect("shutdown");
        assert!(
            factory
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.starts_with("close:"))
        );
    }

    #[tokio::test]
    async fn close_timeout_keeps_the_handle_retryable() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::with_close_results([Err(HostCloseError::DrainTimedOut), Ok(())]);
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        assert!(matches!(
            controller.shutdown().await,
            Err(DesktopShutdownError::Host(HostCloseError::DrainTimedOut))
        ));
        assert!(matches!(
            controller.snapshot().session.phase,
            SessionPhase::Closing
        ));
        controller.shutdown().await.expect("retry close");
    }

    #[tokio::test]
    async fn startup_failure_is_a_typed_desktop_failure() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::failing();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        assert!(matches!(
            controller.snapshot().session.phase,
            SessionPhase::Failed { .. }
        ));
        assert!(controller.snapshot().failure.is_some());
    }

    #[tokio::test]
    async fn session_filter_and_query_intents_update_session_state() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::SetSessionFilter(ArchiveFilter::All))
            .unwrap();
        controller
            .dispatch(DesktopIntent::SetSessionQuery("auth".into()))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(controller.snapshot().sessions.filter, ArchiveFilter::All);
        assert_eq!(controller.snapshot().sessions.query, "auth");
    }

    #[tokio::test]
    async fn back_to_bottom_resets_tail_following() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::SetTailFollowing(false))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(!controller.snapshot().tail_following);
        controller.dispatch(DesktopIntent::BackToBottom).unwrap();
        controller.when_idle().await.unwrap();
        assert!(controller.snapshot().tail_following);
    }

    #[tokio::test]
    async fn controller_never_reinitializes_process_config() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let before = DesktopConfig::process_config_install_count();
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        for root in [tmp.path().join("second"), tmp.path().join("project")] {
            controller
                .dispatch(DesktopIntent::OpenProject { root })
                .unwrap();
            controller.when_idle().await.unwrap();
            wait_ready(&controller).await;
        }
        controller.dispatch(DesktopIntent::NewSession).unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;
        assert_eq!(DesktopConfig::process_config_install_count(), before);
    }

    #[tokio::test]
    async fn permission_mode_is_reapplied_after_ready() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        assert!(
            factory
                .commands
                .lock()
                .unwrap()
                .iter()
                .any(|command| matches!(
                    command,
                    UserCommand::SetPermissionMode(PermissionMode::Default)
                ))
        );
    }

    #[tokio::test]
    async fn startup_global_setting_reports_restart_required() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::SetSetting {
                id: SettingId::Provider,
                value: SettingValue::Text("openai".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        loop {
            if let DesktopEvent::Effect(DesktopEffect::Notify { body, .. }) =
                events.recv().await.unwrap()
            {
                assert_eq!(body, "Requires restart");
                break;
            }
        }
    }

    #[tokio::test]
    async fn startup_settings_persist_for_next_launch() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        for (id, value) in [
            (SettingId::Provider, SettingValue::Text("openai".into())),
            (SettingId::MaxTokens, SettingValue::Text("4096".into())),
            (SettingId::Notifications, SettingValue::Bool(false)),
        ] {
            controller
                .dispatch(DesktopIntent::SetSetting { id, value })
                .unwrap();
            controller.when_idle().await.unwrap();
        }
        let loaded = DesktopConfig::load(tmp.path().join("app-data")).unwrap();
        assert_eq!(loaded.host.provider.as_deref(), Some("openai"));
        assert_eq!(loaded.host.max_tokens, Some(4096));
        assert!(!loaded.notifications);
    }

    #[tokio::test]
    async fn live_setting_routes_to_user_command() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::SetSetting {
                id: SettingId::Model,
                value: SettingValue::Text("new-model".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            factory
                .commands
                .lock()
                .unwrap()
                .iter()
                .any(|command| matches!(
                    command,
                    UserCommand::SetModel(model) if model == "new-model"
                ))
        );
    }

    #[tokio::test]
    async fn surface_snapshot_without_transcript_rows_is_published() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::InvokeCommand {
                id: tact_catalog::CommandId::Builtin(tact_catalog::BuiltinCommand::Stats),
                args: tact_catalog::CommandArguments::default(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();

        let snapshot = tact_protocol::StatsSnapshot {
            turns_taken: 3,
            total_tokens: 42,
            ..tact_protocol::StatsSnapshot::default()
        };
        factory.emit(HostEvent::agent(AgentUpdate::SurfaceSnapshot {
            request_id: 1,
            snapshot: tact_protocol::SurfaceSnapshot::Stats(snapshot.clone()),
        }));
        controller.when_idle().await.unwrap();

        assert_eq!(
            controller.snapshot().surface,
            Some(tact_protocol::SurfaceSnapshot::Stats(snapshot))
        );
    }

    #[tokio::test]
    async fn next_prompt_after_run_finished_is_submitted() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "first".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        factory.emit(HostEvent::agent(AgentUpdate::RunStarted {
            run_id: "run-1".into(),
        }));
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "second".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        factory.emit(HostEvent::agent(AgentUpdate::RunFinished {
            run_id: "run-1".into(),
        }));
        controller.when_idle().await.unwrap();
        let commands = factory.commands.lock().unwrap();
        assert!(
            commands
                .iter()
                .any(|command| matches!(command, UserCommand::SubmitTask(text) if text == "first"))
        );
        assert!(
            commands.iter().any(
                |command| matches!(command, UserCommand::SubmitTask(text) if text == "second")
            )
        );
        assert!(controller.snapshot().composer.queue.is_empty());
    }

    #[tokio::test]
    async fn submission_model_reaches_the_live_session() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "use this model".into(),
                attachments: Vec::new(),
                model: Some("chosen-model".into()),
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(factory.commands.lock().unwrap().iter().any(|command| {
            matches!(command, UserCommand::SetModel(model) if model == "chosen-model")
        }));
    }

    #[tokio::test]
    async fn invalid_permission_mode_is_rejected_not_coerced_to_default() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let before = factory
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| matches!(command, UserCommand::SetPermissionMode(_)))
            .count();
        controller
            .dispatch(DesktopIntent::SetSetting {
                id: SettingId::PermissionMode,
                value: SettingValue::Choice("unsafe".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let after = factory
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| matches!(command, UserCommand::SetPermissionMode(_)))
            .count();
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn close_project_timeout_keeps_project_attached() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::with_close_results([Err(HostCloseError::DrainTimedOut)]);
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        let project = controller.snapshot().project.id.clone().unwrap();
        controller
            .dispatch(DesktopIntent::CloseProject {
                project_id: project,
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let snapshot = controller.snapshot();
        assert!(snapshot.project.id.is_some());
        assert_eq!(snapshot.session.phase, SessionPhase::Closing);
        assert!(
            snapshot
                .failure
                .as_ref()
                .is_some_and(|failure| failure.retryable)
        );
    }

    #[tokio::test]
    async fn stale_gate_token_cannot_answer_reopened_session() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let stale_epoch = controller.snapshot().session.epoch;
        let project = controller.snapshot().project.id.clone().unwrap();
        controller
            .dispatch(DesktopIntent::CloseProject {
                project_id: project,
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::OpenProject {
                root: tmp.path().join("project"),
            })
            .unwrap();
        wait_ready(&controller).await;
        assert_ne!(controller.snapshot().session.epoch, stale_epoch);
        controller
            .dispatch(DesktopIntent::RespondGate {
                session_epoch: stale_epoch,
                request_id: 0,
                response: GateResponse::Select(Some(0)),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            !factory
                .commands
                .lock()
                .unwrap()
                .iter()
                .any(|command| { matches!(command, UserCommand::UiResponse(_)) })
        );
    }

    #[tokio::test]
    async fn stale_gate_click_during_switch_does_not_poison_the_new_session() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        let stale_epoch = controller.snapshot().session.epoch;
        let project = controller.snapshot().project.id.clone().unwrap();
        controller
            .dispatch(DesktopIntent::CloseProject {
                project_id: project,
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::OpenProject {
                root: tmp.path().join("project"),
            })
            .unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::RespondGate {
                session_epoch: stale_epoch,
                request_id: 0,
                response: GateResponse::Select(Some(0)),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            controller.snapshot().failure.is_none(),
            "a late gate click surfaced as a new-session failure: {:?}",
            controller.snapshot().failure
        );
    }

    #[tokio::test]
    async fn duplicate_gate_click_is_a_no_op() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        factory.pending.lock().unwrap().push(pending_select(1));
        factory.emit(HostEvent::agent(AgentUpdate::RequestSelect {
            request_id: 1,
            prompt: "Allow?".into(),
            options: vec!["Allow".into()],
            log_confirm: false,
            from_subagent: false,
            purpose: tact_protocol::UiRequestPurpose::Permission,
            origin: tact_protocol::UiRequestOrigin::main(),
        }));
        controller.when_idle().await.unwrap();
        let epoch = controller.snapshot().session.epoch;
        let intent = || DesktopIntent::RespondGate {
            session_epoch: epoch,
            request_id: 1,
            response: GateResponse::Select(Some(0)),
        };
        controller.dispatch(intent()).unwrap();
        controller.dispatch(intent()).unwrap();
        controller.when_idle().await.unwrap();
        let responses = factory
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| matches!(command, UserCommand::UiResponse(_)))
            .count();
        assert_eq!(responses, 1);
        assert_eq!(controller.snapshot().failure, None);
    }

    #[tokio::test]
    async fn manual_submit_after_cancel_keeps_the_queue_paused() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "first".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "queued after cancel".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        factory.emit(HostEvent::agent(AgentUpdate::RunStarted {
            run_id: "run-1".into(),
        }));
        factory.emit(HostEvent::agent(AgentUpdate::RunFinished {
            run_id: "run-1".into(),
        }));
        factory.emit(HostEvent::agent(AgentUpdate::TaskCancelled));
        controller.when_idle().await.unwrap();
        assert!(controller.snapshot().runtime.queue_paused);
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "manual".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        factory.emit(HostEvent::agent(AgentUpdate::RunStarted {
            run_id: "run-2".into(),
        }));
        factory.emit(HostEvent::agent(AgentUpdate::RunFinished {
            run_id: "run-2".into(),
        }));
        controller.when_idle().await.unwrap();
        let submitted = factory
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter_map(|command| match command {
                UserCommand::SubmitTask(text) => Some(text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(submitted, ["first", "manual"]);
    }

    #[tokio::test]
    async fn chinese_readers_do_not_expose_debug_status_labels() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let mut config = config(tmp.path());
        config.language = CatalogLocale::Zh;
        let (controller, events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;

        factory.emit(HostEvent::agent(AgentUpdate::TasksChanged {
            tasks: vec![tact_protocol::TaskSnapshot {
                id: 7,
                subject: "验证中文阅读器".into(),
                status: tact_protocol::TaskStatusSnapshot::InProgress,
                session_id: "session-0".into(),
                owner: "main".into(),
                ..tact_protocol::TaskSnapshot::default()
            }],
            reason: tact_protocol::TasksChangeReason::Updated,
        }));
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::InvokeCommand {
                id: CommandId::Builtin(tact_catalog::BuiltinCommand::TasksDag),
                args: CommandArguments::default(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();

        let reader = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let DesktopEvent::Effect(DesktopEffect::OpenReader(reader)) =
                    events.recv().await.unwrap()
                {
                    break reader;
                }
            }
        })
        .await
        .expect("Tasks DAG reader opens");
        assert!(reader.markdown.contains("进行中"));
        assert!(!reader.markdown.contains("InProgress"));
        controller.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn desktop_skill_routes_submit_the_expanded_skill_body() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let skill_dir = tmp.path().join("project/.tact/skills/demo");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: demo\ndescription: Demo skill\n---\n\n# Demo\n\nFix the issue.",
        )
        .unwrap();
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::InvokeCommand {
                id: CommandId::Builtin(tact_catalog::BuiltinCommand::Skill),
                args: CommandArguments {
                    tokens: vec!["demo".into(), "fix auth".into()],
                },
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let task = factory
            .commands
            .lock()
            .unwrap()
            .iter()
            .find_map(|command| match command {
                UserCommand::SubmitTask(task) => Some(task.clone()),
                _ => None,
            })
            .expect("skill task submitted");
        assert!(task.contains("<skill name=\"demo\""));
        assert!(task.contains("ARGUMENTS: fix auth"));
        controller.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn ready_sessions_publish_project_skills() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        write_skill(&tmp.path().join("project"), "demo");
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        let skills = controller
            .snapshot()
            .extensions
            .snapshot
            .skills
            .iter()
            .filter(|skill| skill.name == "demo")
            .map(|skill| skill.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(skills, ["demo"]);
        controller.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn project_switch_replaces_project_skills() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        write_skill(&tmp.path().join("project"), "alpha");
        write_skill(&tmp.path().join("second"), "beta");
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::OpenProject {
                root: tmp.path().join("second"),
            })
            .unwrap();
        wait_ready(&controller).await;
        let relevant = controller
            .snapshot()
            .extensions
            .snapshot
            .skills
            .iter()
            .filter(|skill| matches!(skill.name.as_str(), "alpha" | "beta"))
            .map(|skill| skill.name.clone())
            .collect::<Vec<_>>();
        assert_eq!(relevant, ["beta"]);
        controller.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn stream_updates_are_coalesced_before_the_snapshot() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let before = controller.snapshot().revision;
        factory.emit(HostEvent::agent(AgentUpdate::MessageStarted {
            message_id: "message-1".into(),
        }));
        for _ in 0..50 {
            factory.emit(HostEvent::agent(AgentUpdate::StreamChunk("x".into())));
        }
        controller.when_idle().await.unwrap();
        let after = controller.snapshot().revision;
        assert!(
            after.saturating_sub(before) <= 2,
            "50 chunks produced {} commits",
            after - before
        );
    }

    #[tokio::test]
    async fn ready_flushes_a_prompt_queued_during_startup() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::deferred_ready();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "queued during startup".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        factory.emit(HostEvent::Ready);
        controller.when_idle().await.unwrap();
        assert!(factory.commands.lock().unwrap().iter().any(
            |command| matches!(command, UserCommand::SubmitTask(text) if text == "queued during startup")
        ));
    }

    #[tokio::test]
    async fn project_switch_clears_the_previous_queue() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "running".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "old queue".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(controller.snapshot().composer.queue.len(), 1);
        controller
            .dispatch(DesktopIntent::OpenProject {
                root: tmp.path().join("second"),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(controller.snapshot().composer.queue.is_empty());
        factory.commands.lock().unwrap().clear();
        factory.emit(HostEvent::agent(AgentUpdate::TaskComplete("new".into())));
        controller.when_idle().await.unwrap();
        assert!(!factory.commands.lock().unwrap().iter().any(
            |command| matches!(command, UserCommand::SubmitTask(text) if text == "old queue")
        ));
    }

    #[tokio::test]
    async fn failed_run_stays_failed_after_run_finished() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        for update in [
            AgentUpdate::RunStarted {
                run_id: "run-1".into(),
            },
            AgentUpdate::TurnStats {
                turns_taken: 1,
                max_turns: None,
            },
            AgentUpdate::MessageStarted {
                message_id: "message-1".into(),
            },
            AgentUpdate::StreamChunk("partial answer".into()),
            AgentUpdate::RunFinished {
                run_id: "run-1".into(),
            },
            AgentUpdate::Error(AgentErrorKind::Other("provider failed".into())),
        ] {
            factory.emit(HostEvent::agent(update));
        }
        controller.when_idle().await.unwrap();
        let snapshot = controller.snapshot();
        let assistant_status = snapshot.transcript.iter().find_map(|row| match &row.body {
            crate::RowBody::Assistant(assistant) => Some(assistant.status),
            _ => None,
        });
        assert_eq!(snapshot.runtime.current_phase, RunPhase::Failed);
        assert_eq!(assistant_status, Some(crate::RowStatus::Failed));
    }

    #[tokio::test]
    async fn start_failure_enters_failed_phase() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::start_failing();
        let (controller, _events) =
            DesktopController::start_with_factory(config_without_project(tmp.path()), factory)
                .unwrap();
        controller
            .dispatch(DesktopIntent::OpenProject {
                root: tmp.path().join("project"),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let snapshot = controller.snapshot();
        assert!(matches!(
            snapshot.session.phase,
            SessionPhase::Failed { .. }
        ));
        assert!(snapshot.failure.is_some());
    }

    #[tokio::test]
    async fn disconnect_exposes_a_retryable_failure() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        factory.emit(HostEvent::Disconnected);
        controller.when_idle().await.unwrap();
        let snapshot = controller.snapshot();
        assert!(matches!(
            snapshot.session.phase,
            SessionPhase::Disconnected { .. }
        ));
        assert!(
            snapshot
                .failure
                .as_ref()
                .is_some_and(|failure| failure.retryable)
        );
    }

    #[tokio::test]
    async fn reasoning_effort_updates_composer_state() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::SetSetting {
                id: SettingId::ReasoningEffort,
                value: SettingValue::Choice("high".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(
            controller.snapshot().composer.effort.as_deref(),
            Some("high")
        );
    }

    #[tokio::test]
    async fn newly_started_session_appears_in_the_rail() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        let snapshot = controller.snapshot();
        let active = snapshot.session.id.as_ref().expect("active session");
        assert!(
            snapshot
                .sessions
                .items
                .iter()
                .any(|session| session.id.as_str() == active.0.as_str())
        );
    }
}
