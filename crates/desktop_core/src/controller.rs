use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, RwLock},
    time::{Duration, Instant},
};

use chrono::Utc;
use tact_agent_api::AgentMessage;
use tact_catalog::{CommandArguments, CommandId, SettingId};
use tact_host::{HostCloseError, HostEvent, TryHostEvent};
use tact_protocol::{PermissionMode, UserCommand};
use tokio::sync::watch;
use tokio::sync::{mpsc, oneshot};

use crate::{
    AccountView, Appearance, CatalogLocale, DesktopConfig, DesktopEffect, DesktopEvent,
    DesktopSnapshot, DesktopTheme, InitialSession, PermissionModeView, ProjectId, ProjectRecord,
    ProjectRegistry, RequestId, RunPhase, SectionMask, SessionCatalog, SessionEpoch, SessionId,
    SessionPhase, SessionSummaryView, StateCommit,
    commands::{CommandRoute, route_command, route_setting},
    error::{DesktopClosed, DesktopShutdownError, DesktopStartError, FailureKind, FailureView},
    intent::{DesktopIntent, GateResponse, ReaderRef, ReaderRequest, SettingValue, Submission},
    runtime::{HostBackend, HostFactory, RealHostFactory},
    state::{DesktopState, SessionUiState},
    transcript::GateResponseError,
};

const MAX_HOST_EVENTS_PER_DRAIN: usize = 256;
const SESSION_PRELOAD_PER_PROJECT: usize = 8;
const SESSION_PRELOAD_MAX: usize = 32;
const SESSION_WARM_MAX: usize = 6;

type ParkedKey = (ProjectId, String);

fn prefetch_candidates(
    items: &[SessionSummaryView],
    current: Option<&str>,
    parked: &HashSet<String>,
) -> Vec<(ProjectId, String)> {
    let mut per_project: HashMap<ProjectId, usize> = HashMap::new();
    let mut candidates = Vec::new();
    for session in items {
        if session.locked
            || session.message_count <= 0
            || current == Some(session.id.as_str())
            || parked.contains(&session.id)
        {
            continue;
        }
        let count = per_project.entry(session.project_id.clone()).or_default();
        if *count >= SESSION_PRELOAD_PER_PROJECT {
            continue;
        }
        *count += 1;
        candidates.push((session.project_id.clone(), session.id.clone()));
        if candidates.len() >= SESSION_PRELOAD_MAX {
            break;
        }
    }
    candidates
}

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
    /// Cancels in-flight store reads so shutdown never waits on a slow disk.
    shutdown: tokio_util::sync::CancellationToken,
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
    /// Model ids the provider's own index reported, delivered off the actor's
    /// critical path so startup never waits on the network.
    ModelsDiscovered {
        main: Vec<String>,
        subagent: Vec<String>,
    },
    SessionPrefetched {
        project_id: ProjectId,
        session_id: String,
        messages: Result<Vec<AgentMessage>, String>,
        history: Result<Vec<String>, String>,
    },
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
        let shutdown = tokio_util::sync::CancellationToken::new();
        let actor_shutdown = shutdown.clone();
        // The actor keeps a sender of its own: model discovery finishes after
        // startup and re-enters the loop as a message rather than mutating
        // state from a background task.
        let actor_intents = intents.clone();
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
                let mut actor = DesktopActor::new(
                    config,
                    factory,
                    intent_rx,
                    actor_intents,
                    events,
                    actor_snapshot,
                    actor_shutdown,
                )
                .await?;
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
                shutdown,
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
        self.shutdown.cancel();
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
    /// Sender side of `intents`, for work that finishes later and must come
    /// back through the actor's own loop.
    intents_tx: mpsc::UnboundedSender<ControllerMessage>,
    events: DesktopEventSink,
    snapshot: Arc<RwLock<Arc<DesktopSnapshot>>>,
    state: DesktopState,
    registry: ProjectRegistry,
    project: Option<ProjectRecord>,
    catalog: Option<Arc<SessionCatalog>>,
    /// One lazily opened catalog per registered project, so the session list
    /// can group sessions across projects without reopening every database.
    catalogs: HashMap<ProjectId, Arc<SessionCatalog>>,
    host: Option<Box<dyn HostBackend>>,
    /// The event that woke the actor's select, consumed by the next drain.
    pending_host_event: Option<HostEvent>,
    /// Set once the host event channel has closed, so the select stops polling
    /// a receiver that can never yield again.
    host_events_closed: bool,
    queue: VecDeque<Submission>,
    request_id: u64,
    last_gate_reconcile: Instant,
    restore_failure: Option<FailureView>,
    /// Commands for the voice worker task, when voice is configured.
    voice: Option<mpsc::UnboundedSender<tact::voice::VoiceCommand>>,
    /// Sessions whose run was still alive when another session took the
    /// window. Their hosts keep running and their events keep landing in
    /// their own parked state.
    parked: HashMap<ParkedKey, ParkedSession>,
    /// True while a parked host's events are being drained into its state.
    /// The shell is showing another session, so nothing may be published.
    parked_drain: bool,
    /// Store reads for visible idle sessions, keyed by project and session.
    prefetched: HashMap<(ProjectId, String), PrefetchedSession>,
    prefetching: HashSet<(ProjectId, String)>,
    closing: Vec<ClosingHost>,
    /// Cancels in-flight store reads when the shell asks the controller to stop.
    shutdown: tokio_util::sync::CancellationToken,
}

/// A running session parked behind the visible one.
struct ParkedSession {
    project_id: ProjectId,
    host: Box<dyn HostBackend>,
    ui: SessionUiState,
    queue: VecDeque<Submission>,
    restore_failure: Option<FailureView>,
    pending_host_event: Option<HostEvent>,
    host_events_closed: bool,
    parked_at: Instant,
}

struct ClosingHost {
    project_id: ProjectId,
    task: tokio::task::JoinHandle<()>,
}

impl ParkedSession {
    fn is_idle(&self) -> bool {
        self.queue.is_empty()
            && self.ui.reducer.pending_gates().is_empty()
            && self.pending_host_event.is_none()
            && self.ui.runtime.current_phase == RunPhase::Idle
            && !matches!(
                self.ui.session.phase,
                SessionPhase::Running
                    | SessionPhase::Starting
                    | SessionPhase::Opening
                    | SessionPhase::Switching
            )
    }
}

struct PrefetchedSession {
    messages: Vec<AgentMessage>,
    history: Option<Vec<String>>,
}

/// The keyboard-shortcut reader shown by `/help`.
///
/// It is a free function so the shell's off-screen harness can render the same
/// content without going through the actor's effect channel.
pub fn help_reader(language: CatalogLocale) -> ReaderRequest {
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
    let title = localized(language, "Keyboard Shortcuts", "键盘快捷键");
    let markdown = match language {
        CatalogLocale::En => format!(
            "# Keyboard Shortcuts\n\n- {primary}+K Command Center\n- {primary}+B Toggle sessions\n- {primary}+Alt+B Toggle inspector\n- {primary}+Enter Send\n- Esc Close/cancel\n- {primary}+. Stop\n- {primary}+1..5 Inspector tabs\n- {option}+Up/Down Queue navigation\n- Space Toggle multi-select\n- Enter Activate focused control\n- Tab / Shift+Tab Traverse controls"
        ),
        CatalogLocale::Zh => format!(
            "# 键盘快捷键\n\n- {primary}+K 命令中心\n- {primary}+B 切换会话栏\n- {primary}+Alt+B 切换检查器\n- {primary}+Enter 发送\n- Esc 关闭/取消\n- {primary}+. 停止\n- {primary}+1..5 检查器标签\n- {option}+Up/Down 队列导航\n- Space 切换多选项\n- Enter 激活当前控件\n- Tab / Shift+Tab 遍历控件"
        ),
    };
    ReaderRequest {
        title: Arc::from(title),
        markdown: Arc::from(markdown),
    }
}

impl DesktopActor {
    async fn new(
        config: DesktopConfig,
        factory: Arc<dyn HostFactory>,
        intents: mpsc::UnboundedReceiver<ControllerMessage>,
        intents_tx: mpsc::UnboundedSender<ControllerMessage>,
        events: DesktopEventSink,
        snapshot: Arc<RwLock<Arc<DesktopSnapshot>>>,
        shutdown: tokio_util::sync::CancellationToken,
    ) -> Result<Self, DesktopStartError> {
        let registry = ProjectRegistry::open(&config.app_data_dir)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        let mut actor = Self {
            state: DesktopState::new(&config),
            config,
            factory,
            intents,
            intents_tx,
            events,
            snapshot,
            registry,
            project: None,
            catalog: None,
            catalogs: HashMap::new(),
            host: None,
            pending_host_event: None,
            host_events_closed: false,
            queue: VecDeque::new(),
            request_id: 1,
            last_gate_reconcile: Instant::now(),
            restore_failure: None,
            voice: None,
            parked: HashMap::new(),
            parked_drain: false,
            prefetched: HashMap::new(),
            prefetching: HashSet::new(),
            closing: Vec::new(),
            shutdown,
        };
        if let Some(settings) = tact::config::try_settings()
            .map(|settings| settings.voice)
            .filter(|voice| voice.enabled)
        {
            let (commands, mut command_rx) = mpsc::unbounded_channel();
            let worker = tact::voice::spawn_worker(settings);
            let command_tx = worker.command_tx.clone();
            let mut event_rx = worker.event_rx;
            let urgent = actor.events.urgent.clone();
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        Some(command) = command_rx.recv() => {
                            let _ = command_tx.send(command);
                        }
                        Some(event) = event_rx.recv() => {
                            let _ = urgent.send(DesktopEvent::Voice(event));
                        }
                        else => break,
                    }
                }
                let _ = command_tx.send(tact::voice::VoiceCommand::Shutdown);
            });
            actor.voice = Some(commands);
        }
        actor.state.set_projects(actor.registry.records());
        actor.spawn_model_discovery();
        if let Some(root) = actor.config.initial_project.clone()
            && let Err(error) = actor
                .open_project(root, actor.config.initial_session.clone())
                .await
        {
            actor.set_failure(start_error_message(error), true);
        }
        Ok(actor)
    }

    /// Asks the provider for its model index and feeds the result back through
    /// the actor loop.
    ///
    /// A provider entry carries `models = [...]` only when the user wrote one,
    /// so pickers built from configuration alone can end up with a single row:
    /// the model already selected. The TUI fills that gap from `GET /models`;
    /// the desktop runs the same query here, off the startup path, because it
    /// is network I/O and a slow endpoint must not delay the first frame.
    /// Soft failures are expected — an endpoint that serves no index, or has
    /// no key, reports nothing and leaves the configured list untouched.
    fn spawn_model_discovery(&self) {
        let Some(settings) = tact::config::try_settings() else {
            return;
        };
        // `ensure_api_model_ids` reads the process-global provider and panics
        // when none was installed, so check first: a shell running without one
        // keeps configuration-only lists.
        if tact_llm::try_get_provider().is_none() {
            return;
        }
        let subagent_provider = settings
            .agent
            .subagent
            .as_ref()
            .map(|subagent| subagent.provider.clone());
        let intents = self.intents_tx.clone();
        tokio::spawn(async move {
            // Both endpoints are independent; querying them together keeps a
            // slow provider from doubling the time the picker stays short.
            let (main, subagent) = tokio::join!(tact_llm::ensure_api_model_ids(), async move {
                match subagent_provider {
                    Some(provider) => tact_llm::ensure_api_model_ids_for_provider(&provider).await,
                    None => Vec::new(),
                }
            });
            if main.is_empty() && subagent.is_empty() {
                return;
            }
            let _ = intents.send(ControllerMessage::ModelsDiscovered { main, subagent });
        });
    }

    async fn run(mut self) {
        enum Wake {
            Message(Option<ControllerMessage>),
            Host(Option<HostEvent>),
            Tick,
        }

        let mut poll =
            tokio::time::interval(self.config.update_batch.max(Duration::from_millis(1)));
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            self.drain_host_events().await;
            self.drain_parked_events().await;
            self.reap_closing_hosts();
            let host_open = self.host.is_some() && !self.host_events_closed;
            let wake = {
                let intents = &mut self.intents;
                let host = &mut self.host;
                tokio::select! {
                    message = intents.recv() => Wake::Message(message),
                    event = async {
                        match host {
                            Some(host) if host_open => host.next_event().await,
                            _ => std::future::pending().await,
                        }
                    } => Wake::Host(event),
                    _ = poll.tick() => Wake::Tick,
                }
            };
            match wake {
                Wake::Message(None) => break,
                Wake::Message(Some(message)) => {
                    if self.handle_message(message).await {
                        break;
                    }
                }
                Wake::Host(Some(event)) => self.pending_host_event = Some(event),
                Wake::Host(None) => {
                    self.host_events_closed = true;
                    self.pending_host_event = None;
                    self.mark_host_disconnected();
                }
                Wake::Tick => {}
            }
        }
        let _ = self.close_host().await;
        self.close_parked_hosts().await;
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
            ControllerMessage::ModelsDiscovered { main, subagent } => {
                // The composer is part of every snapshot, so an empty section
                // mask is enough: the shell re-syncs the pickers from the
                // snapshot on every commit.
                if self.state.merge_discovered_models(&main, &subagent) {
                    self.publish(false);
                }
                false
            }
            ControllerMessage::SessionPrefetched {
                project_id,
                session_id,
                messages,
                history,
            } => {
                self.accept_prefetched(project_id, session_id, messages, history);
                false
            }
            ControllerMessage::WaitIdle(ack) => {
                self.drain_host_events().await;
                self.drain_parked_events().await;
                let _ = ack.send(());
                false
            }
            ControllerMessage::Shutdown(ack) => {
                let result = self.close_host().await.map_err(DesktopShutdownError::Host);
                self.close_parked_hosts().await;
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
                self.handle_close_project(project_id).await;
            }
            DesktopIntent::RemoveProject { project_id } => {
                self.handle_remove_project(project_id).await?;
            }
            DesktopIntent::NewSession => {
                if self.session_switch_in_flight() {
                    return Ok(());
                }
                self.start_session(InitialSession::New).await?;
            }
            DesktopIntent::NewSessionInProject { project_id } => {
                if self.session_switch_in_flight() {
                    return Ok(());
                }
                if self.project.as_ref().map(|project| &project.id) == Some(&project_id) {
                    self.start_session(InitialSession::New).await?;
                    return Ok(());
                }
                let root = self
                    .registry
                    .records()
                    .iter()
                    .find(|project| project.id == project_id)
                    .map(|project| project.root.clone())
                    .ok_or_else(|| "project not found".to_string())?;
                self.open_project(root, InitialSession::New)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
            }
            DesktopIntent::ResumeSession { session_id } => {
                // Re-opening the session that is already open tears down a
                // healthy host and waits for its driver to drain so the same
                // session can come straight back. Clicking a row in the
                // manager more than once used to queue exactly that, one
                // teardown per click, and the app spent the whole time
                // closing nothing. There is nothing to resume: the session is
                // already there.
                if self.host.is_some()
                    && self
                        .state
                        .session
                        .id
                        .as_ref()
                        .is_some_and(|current| current.0 == session_id.0)
                    && matches!(
                        self.state.session.phase,
                        SessionPhase::Ready | SessionPhase::Running
                    )
                {
                    return Ok(());
                }
                // A click that lands mid-switch is dropped rather than queued:
                // the next switch would begin by closing a host that is still
                // starting, which is how a few fast clicks turned into a
                // minute of "Closing".
                if self.session_switch_in_flight() {
                    return Ok(());
                }
                let target = self
                    .state
                    .sessions
                    .items
                    .iter()
                    .find(|session| session.id == session_id.0)
                    .map(|session| (session.project_id.clone(), session.root_dir.clone()));
                if let Some((project_id, root)) = target
                    && self.project.as_ref().map(|project| &project.id) != Some(&project_id)
                {
                    self.open_project(root, InitialSession::Resume(session_id.0))
                        .await
                        .map_err(|error| format!("{error:?}"))?;
                    return Ok(());
                }
                // Move the rail's highlight as soon as the click lands. The
                // host teardown and history restore happen off the UI thread;
                // without this the selected row does not move until the new
                // snapshot arrives, which reads as a frozen list.
                self.state.sessions.selected = Some(session_id.clone());
                self.publish(false);
                self.start_session(InitialSession::Resume(session_id.0))
                    .await?;
            }
            DesktopIntent::ResumeLast => {
                if self.session_switch_in_flight() {
                    return Ok(());
                }
                self.start_session(InitialSession::ResumeLast).await?;
            }
            DesktopIntent::Submit(submission) | DesktopIntent::Queue(submission) => {
                self.submit(submission)?;
            }
            DesktopIntent::EditQueued { index, text } => {
                self.edit_queued(index, text);
            }
            DesktopIntent::RemoveQueued { index } => {
                self.remove_queued(index);
            }
            DesktopIntent::MoveQueued { from, to } => {
                self.move_queued(from, to);
            }
            DesktopIntent::SendQueuedNow { index } => {
                self.send_queued_now(index)?;
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
                // Pause here, not on the `TaskCancelled` update: the runtime
                // reports the run finished before it reports the cancellation,
                // and a drain that lands between the two would otherwise flush
                // the queue the user just stopped.
                self.state.runtime.queue_paused = true;
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
                        // The host owns the request. A local row that drifted
                        // must not swallow the answer while the host waits.
                        if self.host_still_waits(session_epoch, request_id, &response) {
                            self.send_command(UserCommand::UiResponse(response.clone()))?;
                            if let Some(row) = self.state.reducer.confirm_gate_response(&response) {
                                self.publish_rows(vec![row], SectionMask::TRANSCRIPT);
                            }
                        }
                        self.reconcile_gate_snapshot_now();
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
                    .catalog_for_session(&session_id.0)
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
                self.publish_rows(Vec::new(), SectionMask::SESSION);
            }
            DesktopIntent::SetSessionArchived {
                session_id,
                archived,
            } => {
                let catalog = self
                    .catalog_for_session(&session_id.0)
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
                let mut dirty = vec![id];
                if let Some(parent) = self.state.reducer.top_level_row_for(id) {
                    dirty.push(parent);
                }
                self.publish_rows(dirty, SectionMask::TRANSCRIPT);
            }
            DesktopIntent::ToggleTool { group, tool } => {
                self.state.reducer.toggle_tool(group, tool);
                let mut dirty = vec![group, tool];
                if let Some(parent) = self.state.reducer.top_level_row_for(group) {
                    dirty.push(parent);
                }
                self.publish_rows(dirty, SectionMask::TRANSCRIPT);
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
                // A restart replaces this session's run: unlike a switch it
                // must end the current host, never park it.
                self.close_host()
                    .await
                    .map_err(|error| format!("active session did not close: {error}"))?;
                self.start_session(initial).await?;
            }
            DesktopIntent::Voice(command) => {
                if let Some(voice) = &self.voice {
                    let _ = voice.send(command);
                }
            }
            DesktopIntent::Shutdown => {}
        }
        Ok(())
    }

    /// Closing a project cannot fail: it drops in-memory state and best-effort
    /// closes hosts, so it returns no error to report.
    async fn handle_close_project(&mut self, project_id: ProjectId) {
        if self.project.as_ref().map(|project| &project.id) == Some(&project_id) {
            if self.close_host().await.is_err() {
                return;
            }
            self.close_parked_hosts().await;
            let epoch = self.state.begin_session_switch();
            self.project = None;
            self.catalog = None;
            self.prefetched.clear();
            self.prefetching.clear();
            self.state.set_project(None);
            self.state.sessions.items.clear();
            self.state.sessions.selected = None;
            self.state.session = crate::SessionView {
                epoch,
                phase: SessionPhase::Empty,
                ..crate::SessionView::default()
            };
            self.publish(true);
        } else {
            self.close_project_parked_hosts(&project_id).await;
            self.forget_project_data(&project_id);
            self.publish(true);
        }
    }

    async fn handle_remove_project(&mut self, project_id: ProjectId) -> Result<(), String> {
        let active = self.project.as_ref().map(|project| &project.id) == Some(&project_id);
        if active && self.close_host().await.is_err() {
            return Ok(());
        }
        self.close_project_parked_hosts(&project_id).await;
        if !self
            .registry
            .remove(&project_id)
            .map_err(|error| error.to_string())?
        {
            return Ok(());
        }
        self.forget_project_data(&project_id);
        let remaining = self.registry.records().to_vec();
        self.state.set_projects(&remaining);
        if active {
            let epoch = self.state.begin_session_switch();
            self.project = None;
            self.catalog = None;
            self.state.sessions.selected = None;
            self.state.set_project(None);
            self.state.session = crate::SessionView {
                epoch,
                phase: SessionPhase::Empty,
                ..crate::SessionView::default()
            };
            if let Some(next) = remaining.first().cloned() {
                let initial = self
                    .parked
                    .keys()
                    .find(|(candidate, _)| candidate == &next.id)
                    .map(|(_, session)| InitialSession::Resume(session.clone()))
                    .unwrap_or(InitialSession::ResumeLast);
                self.open_project(next.root, initial)
                    .await
                    .map_err(|error| format!("{error:?}"))?;
                return Ok(());
            }
        }
        self.publish(true);
        Ok(())
    }

    fn forget_project_data(&mut self, project_id: &ProjectId) {
        self.catalogs.remove(project_id);
        self.prefetched
            .retain(|(cached_project, _), _| cached_project != project_id);
        self.prefetching
            .retain(|(cached_project, _)| cached_project != project_id);
        self.state
            .sessions
            .items
            .retain(|session| &session.project_id != project_id);
    }

    fn edit_queued(&mut self, index: usize, text: String) {
        if let Some(item) = self.queue.get_mut(index) {
            item.text = text;
        }
        self.sync_queue();
    }

    fn remove_queued(&mut self, index: usize) {
        self.queue.remove(index);
        self.sync_queue();
    }

    fn move_queued(&mut self, from: usize, to: usize) {
        if from < self.queue.len() && to < self.queue.len() && from != to {
            if let Some(item) = self.queue.remove(from) {
                self.queue.insert(to, item);
            }
            self.sync_queue();
        }
    }

    fn send_queued_now(&mut self, index: usize) -> Result<(), String> {
        let Some(item) = self
            .queue
            .get(index)
            .cloned()
            .and_then(|_| self.queue.remove(index))
        else {
            return Ok(());
        };
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
        Ok(())
    }

    async fn open_project(
        &mut self,
        root: PathBuf,
        initial: InitialSession,
    ) -> Result<(), DesktopStartError> {
        let project = self
            .registry
            .register(root)
            .map_err(|error| DesktopStartError::Config(error.to_string()))?;
        let target = match &initial {
            InitialSession::Resume(session_id) => Some((project.id.clone(), session_id.clone())),
            _ => None,
        };
        self.leave_current_session()
            .await
            .map_err(|error| DesktopStartError::Runtime(error.to_string()))?;
        self.state.set_projects(self.registry.records());
        self.state.begin_session_switch();
        self.state.sessions.selected = None;
        self.clear_queue();
        self.state.set_project(Some(&project));
        let catalog = self
            .catalogs
            .entry(project.id.clone())
            .or_insert_with(|| Arc::new(SessionCatalog::new(project.clone())))
            .clone();
        self.catalog = Some(catalog);
        self.project = Some(project.clone());
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
        if let Some(key) = target
            && let Some(parked) = self.parked.remove(&key)
        {
            let session_id = key.1;
            self.adopt_session(parked, session_id);
            self.publish(true);
            return Ok(());
        }
        // A cold start needs the project's catalog before the host can restore
        // its transcript. Warm adoption above returns before this read, so a
        // repeated cross-project switch never pays for the store scan.
        self.refresh_sessions_inner().await?;
        self.start_host(&project, initial).await?;
        self.publish(true);
        Ok(())
    }

    /// Whether a session start is still moving the window between hosts.
    ///
    /// Covers only the phases where a *teardown* is underway: `Opening` (the
    /// project is still being read) and `Switching` (`begin_session_switch`
    /// ran and the old UI state is gone). A switch that arrives there would
    /// park a half-built session and stack work behind it, so those clicks are
    /// dropped rather than queued — one deliberate switch is quick, and a
    /// queue of them looks exactly like a freeze.
    ///
    /// `Starting` is deliberately **not** covered. It means the target host
    /// exists and its `bootstrap_session` is still running, which on this
    /// platform includes connecting every MCP server at 60s init plus 30s
    /// tool-list budget per server. The window is otherwise empty during that
    /// time, so refusing the click is not protecting anything: it just pins
    /// the user to the slowest server in their config. Switching away parks
    /// the starting host (see `leave_current_session`) and starts the target.
    fn session_switch_in_flight(&self) -> bool {
        matches!(
            self.state.session.phase,
            SessionPhase::Opening | SessionPhase::Switching
        )
    }

    async fn start_session(&mut self, initial: InitialSession) -> Result<(), String> {
        let project = self
            .project
            .clone()
            .ok_or_else(|| "no project open".to_string())?;
        let parked_key = match &initial {
            InitialSession::Resume(session_id) => Some((project.id.clone(), session_id.clone())),
            _ => None,
        };
        // Returning to a session that was left running restores it in place:
        // starting a second host for the same id would collide with the
        // session lock and lose the run.
        if let Some(parked_key) = parked_key
            && self.parked.contains_key(&parked_key)
        {
            self.leave_current_session().await?;
            if let Some(parked) = self.parked.remove(&parked_key) {
                self.adopt_session(parked, parked_key.1.clone());
                self.publish(true);
                return Ok(());
            }
        }
        self.leave_current_session().await?;
        self.state.begin_session_switch();
        self.clear_queue();
        self.start_host(&project, initial)
            .await
            .map_err(start_error_message)?;
        self.publish(true);
        Ok(())
    }

    /// Takes the visible session's host out of the window without ending it.
    ///
    /// Running and idle hosts enter the same project-keyed warm pool. Idle
    /// hosts beyond the pool limit close in a detached task so the switch path
    /// never waits for a host drain.
    ///
    /// A host that has not reported `Ready` is closed instead of parked. It is
    /// still inside `bootstrap_session`, and on this platform that window is
    /// dominated by MCP connection attempts (`MCP_INIT_TIMEOUT` plus
    /// `MCP_LIST_TOOLS_TIMEOUT` per server). Parking one would keep an
    /// abandoned start alive and connecting for as long as it takes, one per
    /// click, and none of them could ever become usable: the user has already
    /// left. Switching back re-starts that session from the store.
    async fn leave_current_session(&mut self) -> Result<(), String> {
        let Some(mut host) = self.host.take() else {
            return Ok(());
        };
        let project_id = self
            .project
            .as_ref()
            .map(|project| project.id.clone())
            .ok_or_else(|| "no project open".to_string())?;
        let session_id = host.session_id().to_string();
        let ui = self.state.take_session_ui();
        let queue = std::mem::take(&mut self.queue);
        let restore_failure = self.restore_failure.take();
        let pending_host_event = self.pending_host_event.take();
        let host_events_closed = self.host_events_closed;
        self.host_events_closed = false;

        if matches!(
            ui.session.phase,
            SessionPhase::Opening | SessionPhase::Starting
        ) {
            self.closing.push(ClosingHost {
                project_id,
                task: tokio::spawn(async move {
                    let _ = host.close().await;
                }),
            });
            return Ok(());
        }

        self.parked.insert(
            (project_id.clone(), session_id),
            ParkedSession {
                project_id,
                host,
                ui,
                queue,
                restore_failure,
                pending_host_event,
                host_events_closed,
                parked_at: Instant::now(),
            },
        );
        // Eviction waits for the next parked drain. That drain folds events
        // already queued on the host into its state first, so a just-arrived
        // gate or stream event cannot be mistaken for an idle host and closed.
        Ok(())
    }

    fn evict_old_warm_sessions(&mut self) {
        loop {
            let idle_count = self
                .parked
                .values()
                .filter(|parked| parked.is_idle())
                .count();
            if idle_count <= SESSION_WARM_MAX {
                return;
            }
            let victim = self
                .parked
                .iter()
                .filter(|(_, parked)| parked.is_idle())
                .min_by_key(|(_, parked)| parked.parked_at)
                .map(|(key, _)| key.clone());
            let Some(key) = victim else {
                return;
            };
            let Some(mut parked) = self.parked.remove(&key) else {
                return;
            };
            let project_id = key.0.clone();
            self.closing.push(ClosingHost {
                project_id,
                task: tokio::spawn(async move {
                    let _ = parked.host.close().await;
                }),
            });
        }
    }

    /// Puts a parked session back in the window. Its epoch moves so stale gate
    /// responses and commits are rejected; the shell updates cached row views
    /// in place because the reducer already holds the rows.
    fn adopt_session(&mut self, parked: ParkedSession, session_id: String) {
        let ParkedSession {
            project_id: _,
            host,
            mut ui,
            queue,
            restore_failure,
            pending_host_event,
            host_events_closed,
            parked_at: _,
        } = parked;
        ui.session.epoch = self.state.adopt_session_epoch();
        self.state.put_session_ui(ui);
        self.state.sessions.selected = Some(SessionId(session_id));
        if matches!(
            self.state.session.phase,
            SessionPhase::Closing | SessionPhase::Empty | SessionPhase::Switching
        ) {
            self.state.session.phase = SessionPhase::Ready;
        }
        self.host = Some(host);
        self.queue = queue;
        self.restore_failure = restore_failure;
        self.pending_host_event = pending_host_event;
        self.host_events_closed = host_events_closed;
        self.sync_queue();
        self.reconcile_gate_snapshot_now();
    }

    /// Drains the events of every parked session into its own state.
    ///
    /// The window belongs to the visible session, so no commit and no effect
    /// leaves here: the parked state is snapshotted again when it is adopted.
    async fn drain_parked_events(&mut self) {
        if self.parked.is_empty() {
            return;
        }
        let keys: Vec<ParkedKey> = self.parked.keys().cloned().collect();
        for key in keys {
            let Some(parked) = self.parked.remove(&key) else {
                continue;
            };
            if parked.host_events_closed {
                self.parked.insert(key, parked);
                continue;
            }
            let ParkedSession {
                project_id,
                host,
                ui,
                queue,
                restore_failure,
                pending_host_event,
                host_events_closed,
                parked_at,
            } = parked;
            let visible_ui = self.state.take_session_ui();
            let visible_host = self.host.take();
            let visible_queue = std::mem::take(&mut self.queue);
            let visible_restore_failure = self.restore_failure.take();
            let visible_pending = self.pending_host_event.take();
            let visible_closed =
                std::mem::replace(&mut self.host_events_closed, host_events_closed);

            self.state.put_session_ui(ui);
            self.host = Some(host);
            self.queue = queue;
            self.restore_failure = restore_failure;
            self.pending_host_event = pending_host_event;
            self.parked_drain = true;
            self.drain_host_events().await;
            self.parked_drain = false;

            let host = self.host.take();
            let queue = std::mem::take(&mut self.queue);
            let restore_failure = self.restore_failure.take();
            let pending_host_event = self.pending_host_event.take();
            let host_events_closed =
                std::mem::replace(&mut self.host_events_closed, visible_closed);
            let ui = self.state.take_session_ui();

            self.state.put_session_ui(visible_ui);
            self.host = visible_host;
            self.queue = visible_queue;
            self.restore_failure = visible_restore_failure;
            self.pending_host_event = visible_pending;

            if let Some(host) = host {
                self.parked.insert(
                    key,
                    ParkedSession {
                        project_id,
                        host,
                        ui,
                        queue,
                        restore_failure,
                        pending_host_event,
                        host_events_closed,
                        parked_at,
                    },
                );
            }
        }
        // A parked run can finish while the window is elsewhere. It becomes
        // idle at that point, so the warm-pool cap must be enforced again.
        self.evict_old_warm_sessions();
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
        self.pending_host_event = None;
        self.host_events_closed = false;
        self.restore_failure = None;
        let (messages, history) = if let Some(cached) = self
            .prefetched
            .remove(&(project.id.clone(), session_id.clone()))
        {
            (cached.messages, cached.history)
        } else {
            let Some(store) = self
                .catalog
                .as_ref()
                .and_then(|catalog| catalog.store_handle())
            else {
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
            let loaded = tokio::select! {
                loaded = store.load_agent_session(&session_id) => loaded,
                _ = self.shutdown.cancelled() => return Ok(()),
            };
            let messages = match loaded {
                Ok(messages) => messages,
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
                        retry_label: Some(Arc::from(localized(
                            self.state.language,
                            "Retry",
                            "重试",
                        ))),
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
            };
            let history = tokio::select! {
                history = store.load_input_history(&session_id) => history,
                _ = self.shutdown.cancelled() => return Ok(()),
            }
            .ok();
            (messages, history)
        };
        self.state.reducer.seed_persisted(
            &messages,
            &crate::ReduceContext {
                session_epoch: self.state.session.epoch,
                show_hook_output: self.state.show_hook_output,
            },
        );
        self.state.composer.history = history
            .unwrap_or_default()
            .into_iter()
            .rev()
            .map(Arc::from)
            .collect::<Vec<_>>()
            .into();
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

    fn catalog_for_session(&self, session_id: &str) -> Option<Arc<SessionCatalog>> {
        let project_id = self
            .state
            .sessions
            .items
            .iter()
            .find(|session| session.id == session_id)
            .map(|session| session.project_id.clone())?;
        self.catalogs.get(&project_id).cloned()
    }

    fn prune_prefetched(&mut self) {
        let current = self.state.session.id.as_ref().map(|id| id.0.clone());
        let parked: HashSet<String> = self
            .parked
            .keys()
            .map(|(_, session_id)| session_id.clone())
            .collect();
        let visible: HashSet<(ProjectId, String)> =
            prefetch_candidates(&self.state.sessions.items, current.as_deref(), &parked)
                .into_iter()
                .collect();
        self.prefetched.retain(|key, _| visible.contains(key));
    }

    fn schedule_session_prefetch(&mut self) {
        self.prune_prefetched();
        let current = self.state.session.id.as_ref().map(|id| id.0.clone());
        let parked: HashSet<String> = self
            .parked
            .keys()
            .map(|(_, session_id)| session_id.clone())
            .collect();
        for (project_id, session_id) in
            prefetch_candidates(&self.state.sessions.items, current.as_deref(), &parked)
        {
            let key = (project_id.clone(), session_id.clone());
            if self.prefetched.contains_key(&key) || self.prefetching.contains(&key) {
                continue;
            }
            let Some(store) = self
                .catalogs
                .get(&project_id)
                .and_then(|catalog| catalog.store_handle())
            else {
                continue;
            };
            self.prefetching.insert(key);
            let intents = self.intents_tx.clone();
            let shutdown = self.shutdown.clone();
            tokio::spawn(async move {
                let keys = (project_id, session_id);
                let loaded = async {
                    let messages = store
                        .load_agent_session(&keys.1)
                        .await
                        .map_err(|error| error.to_string());
                    let history = store
                        .load_input_history(&keys.1)
                        .await
                        .map_err(|error| error.to_string());
                    (messages, history)
                };
                let result = tokio::select! {
                    result = loaded => result,
                    _ = shutdown.cancelled() => return,
                };
                let _ = intents.send(ControllerMessage::SessionPrefetched {
                    project_id: keys.0,
                    session_id: keys.1,
                    messages: result.0,
                    history: result.1,
                });
            });
        }
    }

    fn accept_prefetched(
        &mut self,
        project_id: ProjectId,
        session_id: String,
        messages: Result<Vec<AgentMessage>, String>,
        history: Result<Vec<String>, String>,
    ) {
        let key = (project_id.clone(), session_id.clone());
        self.prefetching.remove(&key);
        let current = self
            .state
            .session
            .id
            .as_ref()
            .is_some_and(|id| id.0 == session_id);
        let parked = self.parked.contains_key(&key);
        let visible = self.state.sessions.items.iter().any(|session| {
            session.project_id == project_id && session.id == session_id && !session.locked
        });
        if current || parked || !visible {
            return;
        }
        let Ok(messages) = messages else {
            return;
        };
        self.prefetched.insert(
            key,
            PrefetchedSession {
                messages,
                history: history.ok(),
            },
        );
        self.prune_prefetched();
    }

    async fn refresh_sessions_inner(&mut self) -> Result<(), DesktopStartError> {
        let projects = self.registry.records().to_vec();
        if projects.is_empty() {
            self.state.sessions.items.clear();
            self.state.sessions.loading = false;
            self.state.sessions.error = None;
            return Ok(());
        }
        self.state.sessions.loading = true;
        self.publish(false);
        let filter = self.state.sessions.filter;
        let active_project = self.project.as_ref().map(|project| project.id.clone());
        let mut items = Vec::new();
        let mut active_error = None;
        for project in &projects {
            let active = active_project.as_ref() == Some(&project.id);
            // A project that has never opened a session store has no sessions
            // to list; do not create a database just to discover that.
            if !active && !project.root.join(".tact/tact.db").is_file() {
                continue;
            }
            let catalog = self
                .catalogs
                .entry(project.id.clone())
                .or_insert_with(|| Arc::new(SessionCatalog::new(project.clone())))
                .clone();
            let result = tokio::select! {
                result = catalog.list(filter, "") => result,
                _ = self.shutdown.cancelled() => return Ok(()),
            };
            match result {
                Ok(project_items) => items.extend(project_items),
                Err(error) if active => active_error = Some(error.to_string()),
                Err(_) => {}
            }
        }
        self.catalogs
            .retain(|id, _| projects.iter().any(|project| &project.id == id));
        if let Some(error) = active_error {
            self.state.sessions.loading = false;
            self.state.sessions.error = Some(FailureView {
                kind: FailureKind::Store,
                message: Arc::from(error),
                retryable: true,
                retry_label: Some(Arc::from(localized(self.state.language, "Refresh", "刷新"))),
            });
        } else {
            items.sort_by(|left, right| {
                right
                    .search_score
                    .cmp(&left.search_score)
                    .then_with(|| right.updated_at.cmp(&left.updated_at))
            });
            self.state.set_sessions(items);
            // The store reports a lock for every session this process holds.
            // A parked session is ours to reopen, so it must not read as
            // "open in another window" and be disabled in the session rail;
            // only locks held elsewhere stay flagged.
            let owned: Vec<String> = self
                .parked
                .keys()
                .map(|(_, session_id)| session_id.clone())
                .chain(self.state.session.id.iter().map(|id| id.0.clone()))
                .collect();
            for item in &mut self.state.sessions.items {
                if owned.contains(&item.id) {
                    item.locked = false;
                }
            }
        }
        self.publish(false);
        self.schedule_session_prefetch();
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
        if let Some(store) = self
            .catalog
            .as_ref()
            .and_then(|catalog| catalog.store_handle())
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
        // A command starts outside the previous run's terminal-repaint window.
        self.state.reducer.clear_closed_run_rows();
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
            CommandId::Builtin(tact_catalog::BuiltinCommand::Goal) => {
                // The bare `/goal` opens the Goals inspector in the shell. The
                // controller has no work until a goal verb is chosen.
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
            (SettingId::ThemeFamily, SettingValue::Choice(value)) => {
                self.state.theme = DesktopTheme::from_value(value);
                self.config.theme = self.state.theme;
                persist = true;
            }
            _ => {}
        }
        if persist {
            self.config.save().map_err(|error| format!("{error:?}"))?;
        }
        self.state.refresh_settings(&self.config);
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

    /// Builds the reader payload without sending it through the effect channel.
    ///
    /// The off-screen harness uses this to render a reader whose content is
    /// controller-owned (for example the keyboard-shortcut help).
    pub fn reader_request(&self, reader: ReaderRef) -> ReaderRequest {
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
            ReaderRef::Help => help_reader(self.state.language),
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
            let event = if let Some(event) = self.pending_host_event.take() {
                event
            } else {
                match self.host.as_mut().map(|host| host.try_next_event()) {
                    Some(TryHostEvent::Available(event)) => event,
                    Some(TryHostEvent::Empty) | None => break,
                    Some(TryHostEvent::Disconnected) => {
                        self.host_events_closed = true;
                        self.mark_host_disconnected();
                        break;
                    }
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
                    if let tact_protocol::AgentUpdate::PopupMarkdown { title, source } = &update
                        && !self.parked_drain
                    {
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
                        tact_protocol::AgentUpdate::TaskComplete(_)
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
        let (mut dirty, matched_finished, matched_failed) = if agent_updates.is_empty() {
            (Vec::new(), false, false)
        } else {
            let outcome = self.state.apply_agent_batch(agent_updates);
            (
                outcome.dirty_rows,
                outcome.matched_finished,
                outcome.matched_failed,
            )
        };
        run_finished |= matched_finished;
        run_failed |= matched_failed;
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
        if run_finished || run_failed {
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

    /// Closes the sessions parked behind the visible one. Their state is
    /// dropped with the project they belong to.
    async fn close_parked_hosts(&mut self) {
        let parked = std::mem::take(&mut self.parked);
        for (_, mut session) in parked {
            let _ = session.host.close().await;
        }
        for closing in std::mem::take(&mut self.closing) {
            let _ = closing.task.await;
        }
    }

    async fn close_project_parked_hosts(&mut self, project_id: &ProjectId) {
        let keys: Vec<ParkedKey> = self
            .parked
            .keys()
            .filter(|(parked_project, _)| parked_project == project_id)
            .cloned()
            .collect();
        for key in keys {
            if let Some(mut parked) = self.parked.remove(&key) {
                let _ = parked.host.close().await;
            }
        }
        let mut remaining = Vec::new();
        for closing in std::mem::take(&mut self.closing) {
            if &closing.project_id == project_id {
                let _ = closing.task.await;
            } else {
                remaining.push(closing);
            }
        }
        self.closing = remaining;
    }

    fn reap_closing_hosts(&mut self) {
        self.closing.retain(|closing| !closing.task.is_finished());
    }

    /// Whether the host still holds the request a gate click is answering.
    fn host_still_waits(
        &self,
        session_epoch: SessionEpoch,
        request_id: u64,
        response: &tact_protocol::UiResponse,
    ) -> bool {
        if session_epoch != self.state.session.epoch {
            return false;
        }
        let Some(host) = self.host.as_ref() else {
            return false;
        };
        host.pending_requests().iter().any(|pending| {
            pending.request_id == request_id
                && matches!(
                    (pending.kind, response),
                    (
                        tact::ui_responder::PendingUiRequestKind::Select,
                        tact_protocol::UiResponse::Select { .. }
                    ) | (
                        tact::ui_responder::PendingUiRequestKind::MultiSelect,
                        tact_protocol::UiResponse::MultiSelect { .. }
                    )
                )
        })
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
            show_hook_output: self.state.show_hook_output,
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

    fn mark_host_disconnected(&mut self) {
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
    }

    fn publish_rows(&mut self, dirty: Vec<crate::RowId>, sections: SectionMask) {
        // A parked session keeps its state current while another one owns the
        // window; its commits must never reach the shell's watch.
        if self.parked_drain {
            return;
        }
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
    use std::{collections::HashSet, sync::Mutex};

    use async_trait::async_trait;
    use tact::store::{ArchiveFilter, open_sqlite_session_store};
    use tact_agent_api::{AgentMessage, AgentRole, MessageOrigin};
    use tact_catalog::SettingId;
    use tact_host::{
        HostCloseError, HostEvent, HostExit, HostExtensionService, HostSendError, HostStartError,
        TryHostEvent,
    };
    use tact_protocol::{AgentErrorKind, AgentUpdate, PermissionMode, UserCommand};

    use super::*;
    use crate::{GateResponse, InitialSession, SettingValue, Submission};

    #[test]
    fn help_reader_lists_the_primary_shortcuts() {
        let english = help_reader(CatalogLocale::En);
        assert_eq!(english.title.as_ref(), "Keyboard Shortcuts");
        assert!(english.markdown.contains("Command Center"));
        assert!(english.markdown.contains("Inspector tabs"));

        let chinese = help_reader(CatalogLocale::Zh);
        assert_eq!(chinese.title.as_ref(), "键盘快捷键");
        assert!(chinese.markdown.contains("命令中心"));
    }

    #[test]
    fn prefetch_candidates_skip_live_locked_and_empty_sessions() {
        let session =
            |project: &str, id: &str, locked: bool, message_count: i64| SessionSummaryView {
                id: id.into(),
                project_id: ProjectId(project.into()),
                root_dir: format!("/tmp/{project}").into(),
                title: id.into(),
                updated_at: Utc::now(),
                archived_at: None,
                locked,
                message_count,
                search_score: 0,
            };
        let mut items = vec![
            session("a", "a-current", false, 3),
            session("a", "a-parked", false, 3),
            session("a", "a-locked", true, 3),
            session("a", "a-empty", false, 0),
        ];
        for project in ["a", "b", "c", "d", "e"] {
            for index in 1..=10 {
                items.push(session(project, &format!("{project}-{index:02}"), false, 3));
            }
        }
        let parked = HashSet::from(["a-parked".to_string()]);
        let candidates = prefetch_candidates(&items, Some("a-current"), &parked);
        let ids: Vec<&str> = candidates.iter().map(|(_, id)| id.as_str()).collect();
        assert_eq!(ids.len(), 32, "prefetching must stay bounded");
        assert!(ids.contains(&"a-01") && ids.contains(&"a-08"));
        assert!(!ids.contains(&"a-09"), "each project contributes at most 8");
        assert!(ids.contains(&"b-08") && ids.contains(&"c-08") && ids.contains(&"d-08"));
        assert!(
            !candidates.iter().any(|(_, id)| id.starts_with("e-")),
            "the global cap wins over later projects"
        );
    }

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

        async fn next_event(&mut self) -> Option<HostEvent> {
            self.events.recv().await
        }

        fn send(&self, command: UserCommand) -> Result<(), HostSendError> {
            if let UserCommand::UiResponse(response) = &command {
                self.pending
                    .lock()
                    .unwrap()
                    .retain(|pending| pending.request_id != response.request_id());
            }
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
            initial: &InitialSession,
        ) -> Result<Box<dyn HostBackend>, HostStartError> {
            if let Some(error) = &self.start_failure {
                return Err(error.clone());
            }
            let id = match initial {
                InitialSession::Resume(id) if !id.is_empty() => id.clone(),
                _ => format!("session-{}", self.log.lock().unwrap().len()),
            };
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

    async fn seed_sessions(root: &std::path::Path, ids: &[&str]) {
        let root = std::fs::canonicalize(root).unwrap();
        let store = open_sqlite_session_store(&root.join(".tact/tact.db"))
            .await
            .unwrap();
        for id in ids {
            store
                .ensure_session_row(id, &root.display().to_string(), "")
                .await
                .unwrap();
            store.set_session_title(id, Some(id)).await.unwrap();
            store
                .append_agent_message(
                    id,
                    &AgentMessage::text(
                        AgentRole::User,
                        MessageOrigin::User,
                        format!("prompt {id}"),
                    ),
                    1,
                )
                .await
                .unwrap();
        }
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
    async fn project_switch_parks_before_opening_the_next_host() {
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

        let log = factory.log.lock().unwrap().clone();
        assert!(
            log.iter()
                .any(|entry| entry.replace('\\', "/").contains("/second:")),
            "the second project's host must open: {log:?}"
        );
        assert!(
            !log.iter().any(|entry| entry.starts_with("close:")),
            "a project switch must park the old idle host, not close it: {log:?}"
        );
    }

    #[tokio::test]
    async fn new_session_in_project_switches_project_and_starts_a_new_session() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let first_root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let second_root = std::fs::canonicalize(tmp.path().join("second")).unwrap();
        let app_data = tmp.path().join("app-data");
        let mut registry = ProjectRegistry::open(&app_data).unwrap();
        registry.register(&first_root).unwrap();
        let second_project = registry.register(&second_root).unwrap();
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;

        controller
            .dispatch(DesktopIntent::NewSessionInProject {
                project_id: second_project.id.clone(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;

        let snapshot = controller.snapshot();
        assert_eq!(
            snapshot.project.id.as_ref(),
            Some(&second_project.id),
            "the new session must belong to the requested project"
        );
        assert!(
            matches!(snapshot.session.phase, SessionPhase::Ready),
            "the requested project's new session must start: {:?}",
            snapshot.session.phase
        );
        let log = factory.log.lock().unwrap().clone();
        assert!(
            log.iter()
                .any(|entry| entry.replace('\\', "/").contains("/second:session-")),
            "the requested project's host must start a fresh session: {log:?}"
        );
        assert!(
            !log.iter().any(|entry| entry.starts_with("close:")),
            "switching to the requested project must park the old host: {log:?}"
        );
    }

    #[tokio::test]
    async fn session_list_spans_registered_projects_and_resume_switches_project() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let first_root = tmp.path().join("project");
        let second_root = tmp.path().join("second");
        let first_root = std::fs::canonicalize(first_root).unwrap();
        let second_root = std::fs::canonicalize(second_root).unwrap();
        let app_data = tmp.path().join("app-data");
        let mut registry = ProjectRegistry::open(&app_data).unwrap();
        let first_project = registry.register(&first_root).unwrap();
        let second_project = registry.register(&second_root).unwrap();
        for (root, session_id, title) in [
            (&first_root, "first-session", "First project"),
            (&second_root, "second-session", "Second project"),
        ] {
            let store = open_sqlite_session_store(&root.join(".tact/tact.db"))
                .await
                .unwrap();
            store
                .ensure_session_row(session_id, &root.display().to_string(), "")
                .await
                .unwrap();
            store
                .set_session_title(session_id, Some(title))
                .await
                .unwrap();
        }
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let sessions = controller.snapshot().sessions.items.clone();
        assert!(
            sessions.iter().any(|session| session.id == "first-session"),
            "the active project's sessions must be loaded"
        );
        assert!(
            sessions
                .iter()
                .any(|session| session.id == "second-session"),
            "other registered projects' sessions must be loaded for grouping"
        );
        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("second-session".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;
        let snapshot = controller.snapshot();
        assert_eq!(
            snapshot.project.id.as_ref(),
            Some(&second_project.id),
            "resuming a session from another project must switch projects"
        );
        assert!(
            factory
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.replace('\\', "/").contains("/second:")),
            "the second project's host must be opened"
        );
        assert_ne!(
            first_project.id, second_project.id,
            "the fixture must expose two distinct projects"
        );
    }

    #[tokio::test]
    async fn cross_project_top_two_sessions_adopt_their_warm_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let first_root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let second_root = std::fs::canonicalize(tmp.path().join("second")).unwrap();
        seed_sessions(&first_root, &["a1", "a2"]).await;
        seed_sessions(&second_root, &["b1", "b2"]).await;
        let app_data = tmp.path().join("app-data");
        let mut registry = ProjectRegistry::open(&app_data).unwrap();
        let first_project = registry.register(&first_root).unwrap();
        let second_project = registry.register(&second_root).unwrap();
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(app_data)
            .unwrap()
            .with_initial_project(first_root)
            .with_initial_session(InitialSession::Resume("a1".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;

        for _ in 0..5 {
            for (session_id, project_id, prompt) in [
                ("b1", &second_project.id, "prompt b1"),
                ("a1", &first_project.id, "prompt a1"),
            ] {
                controller
                    .dispatch(DesktopIntent::ResumeSession {
                        session_id: SessionId(session_id.into()),
                    })
                    .unwrap();
                controller.when_idle().await.unwrap();
                wait_ready(&controller).await;
                let snapshot = controller.snapshot();
                assert_eq!(snapshot.project.id.as_ref(), Some(project_id));
                assert_eq!(
                    snapshot.session.id.as_ref().map(|id| id.0.as_str()),
                    Some(session_id)
                );
                assert!(
                    snapshot.transcript.iter().any(|row| matches!(
                        &row.body,
                        crate::RowBody::User(user) if user.text.as_ref() == prompt
                    )),
                    "the adopted transcript belongs to another session: {}",
                    session_id
                );
            }
        }

        let log = factory.log.lock().unwrap().clone();
        for session in ["a1", "b1"] {
            assert_eq!(
                log.iter()
                    .filter(|entry| {
                        entry.starts_with("open:") && entry.ends_with(&format!(":{session}"))
                    })
                    .count(),
                1,
                "a warm cross-project session was restarted: {log:?}"
            );
        }
        assert!(
            !log.iter().any(|entry| entry.starts_with("close:")),
            "cross-project switching must keep both hosts warm: {log:?}"
        );
    }

    #[tokio::test]
    async fn removing_a_non_active_project_unregisters_it_only() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let first_root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let second_root = std::fs::canonicalize(tmp.path().join("second")).unwrap();
        seed_sessions(&first_root, &["a1"]).await;
        seed_sessions(&second_root, &["b1"]).await;
        let app_data = tmp.path().join("app-data");
        let mut registry = ProjectRegistry::open(&app_data).unwrap();
        let first_project = registry.register(&first_root).unwrap();
        let second_project = registry.register(&second_root).unwrap();
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(app_data)
            .unwrap()
            .with_initial_project(first_root)
            .with_initial_session(InitialSession::Resume("a1".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;

        controller
            .dispatch(DesktopIntent::RemoveProject {
                project_id: second_project.id.clone(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.project.id.as_ref(), Some(&first_project.id));
        assert!(
            !snapshot
                .projects
                .iter()
                .any(|project| project.id.as_ref() == Some(&second_project.id))
        );
        assert!(
            !snapshot
                .sessions
                .items
                .iter()
                .any(|session| session.id == "b1")
        );
        assert_eq!(
            snapshot.session.id.as_ref().map(|id| id.0.as_str()),
            Some("a1")
        );
        assert!(
            !factory
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry == "close:a1"),
            "removing another project must not touch the active host"
        );
    }

    #[tokio::test]
    async fn removing_the_active_project_switches_to_a_remaining_project() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let first_root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let second_root = std::fs::canonicalize(tmp.path().join("second")).unwrap();
        seed_sessions(&first_root, &["a1"]).await;
        seed_sessions(&second_root, &["b1"]).await;
        let app_data = tmp.path().join("app-data");
        let mut registry = ProjectRegistry::open(&app_data).unwrap();
        let first_project = registry.register(&first_root).unwrap();
        let second_project = registry.register(&second_root).unwrap();
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(app_data)
            .unwrap()
            .with_initial_project(first_root)
            .with_initial_session(InitialSession::Resume("a1".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;

        controller
            .dispatch(DesktopIntent::RemoveProject {
                project_id: first_project.id.clone(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.project.id.as_ref(), Some(&second_project.id));
        assert!(
            !snapshot
                .projects
                .iter()
                .any(|project| project.id.as_ref() == Some(&first_project.id))
        );
        let log = factory.log.lock().unwrap().clone();
        assert!(
            log.iter().any(|entry| entry == "close:a1"),
            "removing the active project must close its host: {log:?}"
        );
        assert!(
            log.iter()
                .any(|entry| entry.replace('\\', "/").contains("/second:")),
            "the next registered project must open: {log:?}"
        );
    }

    #[tokio::test]
    async fn session_archive_hides_and_restore_returns_the_session() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        seed_sessions(&root, &["s1", "s2"]).await;
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(tmp.path().join("app-data"))
            .unwrap()
            .with_initial_project(root)
            .with_initial_session(InitialSession::Resume("s1".into()));
        let (controller, _events) = DesktopController::start_with_factory(config, factory).unwrap();
        wait_ready(&controller).await;
        let session_id = SessionId("s2".into());

        controller
            .dispatch(DesktopIntent::SetSessionArchived {
                session_id: session_id.clone(),
                archived: true,
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            !controller
                .snapshot()
                .sessions
                .items
                .iter()
                .any(|session| session.id == "s2"),
            "archived sessions leave the Active list"
        );

        controller
            .dispatch(DesktopIntent::SetSessionFilter(ArchiveFilter::All))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            controller
                .snapshot()
                .sessions
                .items
                .iter()
                .find(|session| session.id == "s2")
                .is_some_and(|session| session.archived_at.is_some()),
            "the All list still exposes the archived session"
        );

        controller
            .dispatch(DesktopIntent::SetSessionArchived {
                session_id,
                archived: false,
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            controller
                .snapshot()
                .sessions
                .items
                .iter()
                .find(|session| session.id == "s2")
                .is_some_and(|session| session.archived_at.is_none()),
            "restore returns the session to the active catalog"
        );
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
    async fn resuming_the_open_session_does_not_rebuild_its_host() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let session = controller
            .snapshot()
            .session
            .id
            .clone()
            .expect("an open session");
        let before = factory.log.lock().unwrap().len();

        // A list row can deliver the click more than once. Every one of them
        // used to close a healthy host and wait for it to drain so the same
        // session could be reopened, which a few fast clicks turned into a
        // stalled app.
        for _ in 0..3 {
            controller
                .dispatch(DesktopIntent::ResumeSession {
                    session_id: session.clone(),
                })
                .unwrap();
        }
        controller.when_idle().await.unwrap();

        let log = factory.log.lock().unwrap().clone();
        assert_eq!(
            log.len(),
            before,
            "resuming the open session touched the host: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("open:"))
                .count(),
            1,
            "the session was reopened: {log:?}"
        );
    }

    /// A click that lands while a host is still starting must still switch.
    ///
    /// This is the desktop's cold-start shape: `bootstrap_session` connects
    /// every MCP server before it reports `HostEvent::Ready`, and each server
    /// has a 60s init budget plus a 30s tool-list budget. The session phase
    /// therefore sits in `Starting` for as long as that takes, with the
    /// transcript empty and nothing to type into.
    ///
    /// Switching away from it is not a conflict with it. The click parks the
    /// starting host and starts the target, exactly like any other switch;
    /// dropping the click leaves the window stuck on the slowest server.
    #[tokio::test]
    async fn a_switch_lands_while_the_host_is_still_starting() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        seed_sessions(tmp.path(), &["slow-session", "elsewhere"]).await;
        let factory = FakeFactory::deferred_ready();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(
            controller.snapshot().session.phase,
            SessionPhase::Starting,
            "the fixture must still be starting for this test to mean anything"
        );

        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("elsewhere".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();

        assert!(
            factory
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry.starts_with("open:")),
            "the switch never reached the host factory: {:?}",
            factory.log.lock().unwrap()
        );
        assert_eq!(
            controller.snapshot().sessions.selected,
            Some(SessionId("elsewhere".into())),
            "the clicked row must become the selection"
        );
    }

    /// Leaving a host that never became ready closes it instead of parking it.
    ///
    /// The cold-start window is dominated by MCP connection attempts, so a
    /// parked half-started host would keep connecting for up to a minute per
    /// server long after the user switched away. Nothing can adopt it either:
    /// the click already moved on. Closing releases the session lock and the
    /// sockets; switching back rebuilds from the store.
    #[tokio::test]
    async fn leaving_a_starting_host_closes_it_instead_of_parking() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::deferred_ready();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(
            controller.snapshot().session.phase,
            SessionPhase::Starting,
            "the fixture must still be starting for this test to mean anything"
        );

        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("elsewhere".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();

        let log = factory.log.lock().unwrap().clone();
        assert!(
            log.iter().any(|entry| entry.starts_with("close:")),
            "the abandoned starting host was not closed: {log:?}"
        );
        assert_eq!(
            controller.snapshot().sessions.selected,
            Some(SessionId("elsewhere".into())),
            "the switch must still land"
        );
    }

    #[tokio::test]
    async fn switching_sessions_keeps_the_running_host_alive() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let running = controller
            .snapshot()
            .session
            .id
            .clone()
            .expect("an open session");
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "long task".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(controller.snapshot().session.phase, SessionPhase::Running);

        // Leave for another session mid-run.
        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("elsewhere".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_ne!(
            controller.snapshot().session.id.as_ref(),
            Some(&running),
            "the switch did not land on a new session"
        );

        // The parked run keeps consuming its own events.
        {
            let events = factory.host_events.lock().unwrap();
            let parked = events.first().expect("the parked host");
            parked
                .send(HostEvent::agent(AgentUpdate::MessageStarted {
                    message_id: "parked-message".into(),
                }))
                .unwrap();
            parked
                .send(HostEvent::agent(AgentUpdate::StreamChunk(
                    "work done while away".into(),
                )))
                .unwrap();
        }
        controller.when_idle().await.unwrap();

        // Coming back restores the same host and the work it did in the
        // background, without reopening the session.
        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: running.clone(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let snapshot = controller.snapshot();
        assert_eq!(snapshot.session.id.as_ref(), Some(&running));
        assert_eq!(snapshot.session.phase, SessionPhase::Running);
        assert!(
            snapshot.transcript.iter().any(|row| matches!(
                &row.body,
                crate::RowBody::Assistant(assistant)
                    if assistant.markdown.contains("work done while away")
            )),
            "the parked transcript lost its background events ({} rows)",
            snapshot.transcript.len()
        );
        let log = factory.log.lock().unwrap().clone();
        assert!(
            !log.iter()
                .any(|entry| entry == &format!("close:{}", running.0)),
            "switching closed the running host: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("open:"))
                .count(),
            2,
            "the session was reopened instead of resumed: {log:?}"
        );
    }

    #[tokio::test]
    async fn switching_away_from_an_idle_session_keeps_it_warm() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let idle = controller
            .snapshot()
            .session
            .id
            .clone()
            .expect("an open session");
        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("elsewhere".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: idle.clone(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let log = factory.log.lock().unwrap().clone();
        assert!(
            !log.iter().any(|entry| entry.starts_with("close:")),
            "an idle session outside the old top-four list must stay warm: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.ends_with(&format!(":{}", idle.0)))
                .count(),
            1,
            "switching back to an idle host must adopt it: {log:?}"
        );
    }

    #[tokio::test]
    async fn switching_among_recent_idle_sessions_reuses_warm_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let store = open_sqlite_session_store(&root.join(".tact/tact.db"))
            .await
            .unwrap();
        for id in ["s1", "s2"] {
            store
                .ensure_session_row(id, &root.display().to_string(), "")
                .await
                .unwrap();
            store.set_session_title(id, Some(id)).await.unwrap();
            store
                .append_agent_message(
                    id,
                    &AgentMessage::text(AgentRole::User, MessageOrigin::User, id),
                    1,
                )
                .await
                .unwrap();
        }
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(tmp.path().join("app-data"))
            .unwrap()
            .with_initial_project(tmp.path().join("project"))
            .with_initial_session(InitialSession::Resume("s1".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;
        assert_eq!(
            controller
                .snapshot()
                .session
                .id
                .as_ref()
                .map(|id| id.0.as_str()),
            Some("s1")
        );
        for _ in 0..5 {
            for session in ["s2", "s1"] {
                controller
                    .dispatch(DesktopIntent::ResumeSession {
                        session_id: SessionId(session.into()),
                    })
                    .unwrap();
                controller.when_idle().await.unwrap();
                wait_ready(&controller).await;
            }
        }
        let log = factory.log.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("open:") && entry.ends_with(":s1"))
                .count(),
            1,
            "repeated switching to a warm idle session must not start another host: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("open:") && entry.ends_with(":s2"))
                .count(),
            1,
            "the other top session must open once: {log:?}"
        );
        assert!(
            !log.iter().any(|entry| entry.starts_with("close:")),
            "both warm hosts must stay parked: {log:?}"
        );
        assert_eq!(
            controller
                .snapshot()
                .session
                .id
                .as_ref()
                .map(|id| id.0.as_str()),
            Some("s1")
        );
    }

    #[tokio::test]
    async fn warm_pool_evicts_only_the_oldest_idle_host() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let ids: Vec<String> = (0..9).map(|index| format!("s{index}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        seed_sessions(&root, &refs).await;
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(tmp.path().join("app-data"))
            .unwrap()
            .with_initial_project(root)
            .with_initial_session(InitialSession::Resume("s0".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;

        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "keep this run alive".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(controller.snapshot().session.phase, SessionPhase::Running);

        for session in &ids[1..] {
            controller
                .dispatch(DesktopIntent::ResumeSession {
                    session_id: SessionId(session.clone()),
                })
                .unwrap();
            controller.when_idle().await.unwrap();
            wait_ready(&controller).await;
        }
        for _ in 0..100 {
            if factory
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry == "close:s1")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let log = factory.log.lock().unwrap().clone();
        assert!(
            log.iter().any(|entry| entry == "close:s1"),
            "the oldest idle host must be evicted: {log:?}"
        );
        assert!(
            !log.iter().any(|entry| entry == "close:s0"),
            "a running host must never be evicted: {log:?}"
        );
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("close:"))
                .count(),
            1,
            "only the idle overflow host closes: {log:?}"
        );

        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("s0".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        let log = factory.log.lock().unwrap().clone();
        assert_eq!(
            log.iter()
                .filter(|entry| entry.starts_with("open:") && entry.ends_with(":s0"))
                .count(),
            1,
            "the running host was restarted during an idle eviction: {log:?}"
        );
        assert_eq!(controller.snapshot().session.phase, SessionPhase::Running);
    }

    #[tokio::test]
    async fn warm_pool_keeps_a_parked_session_with_a_pending_gate() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let ids: Vec<String> = (0..9).map(|index| format!("s{index}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        seed_sessions(&root, &refs).await;
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(tmp.path().join("app-data"))
            .unwrap()
            .with_initial_project(root)
            .with_initial_session(InitialSession::Resume("s0".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
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
        let snapshot = controller.snapshot();
        assert_eq!(snapshot.session.phase, SessionPhase::Ready);
        assert_eq!(snapshot.pending_gates.len(), 1);

        for session in &ids[1..] {
            controller
                .dispatch(DesktopIntent::ResumeSession {
                    session_id: SessionId(session.clone()),
                })
                .unwrap();
            controller.when_idle().await.unwrap();
            wait_ready(&controller).await;
        }
        for _ in 0..100 {
            if factory
                .log
                .lock()
                .unwrap()
                .iter()
                .any(|entry| entry == "close:s1")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let log = factory.log.lock().unwrap().clone();
        assert!(
            !log.iter().any(|entry| entry == "close:s0"),
            "a pending gate must keep its host warm: {log:?}"
        );
        assert!(
            log.iter().any(|entry| entry == "close:s1"),
            "the oldest gate-free idle host must be evicted instead: {log:?}"
        );
    }

    #[tokio::test]
    async fn close_project_waits_for_parked_and_background_close_tasks() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let root = std::fs::canonicalize(tmp.path().join("project")).unwrap();
        let ids: Vec<String> = (0..8).map(|index| format!("s{index}")).collect();
        let refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        seed_sessions(&root, &refs).await;
        let factory = FakeFactory::new();
        let config = DesktopConfig::load(tmp.path().join("app-data"))
            .unwrap()
            .with_initial_project(root)
            .with_initial_session(InitialSession::Resume("s0".into()));
        let (controller, _events) =
            DesktopController::start_with_factory(config, factory.clone()).unwrap();
        wait_ready(&controller).await;
        for session in &ids[1..] {
            controller
                .dispatch(DesktopIntent::ResumeSession {
                    session_id: SessionId(session.clone()),
                })
                .unwrap();
            controller.when_idle().await.unwrap();
            wait_ready(&controller).await;
        }
        let project_id = controller
            .snapshot()
            .project
            .id
            .clone()
            .expect("active project");
        controller
            .dispatch(DesktopIntent::CloseProject { project_id })
            .unwrap();
        controller.when_idle().await.unwrap();

        let log = factory.log.lock().unwrap().clone();
        for session in &ids {
            assert_eq!(
                log.iter()
                    .filter(|entry| entry.as_str() == format!("close:{session}"))
                    .count(),
                1,
                "CloseProject did not finish closing {session}: {log:?}"
            );
        }
        assert!(controller.snapshot().project.id.is_none());
    }

    #[tokio::test]
    async fn shutdown_closes_parked_hosts() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        let running = controller
            .snapshot()
            .session
            .id
            .clone()
            .expect("an open session");
        controller
            .dispatch(DesktopIntent::Submit(Submission {
                text: "long task".into(),
                attachments: Vec::new(),
                model: None,
                permission_mode: None,
            }))
            .unwrap();
        controller.when_idle().await.unwrap();
        controller
            .dispatch(DesktopIntent::ResumeSession {
                session_id: SessionId("elsewhere".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        controller.shutdown().await.expect("shutdown");
        let log = factory.log.lock().unwrap().clone();
        assert!(
            log.iter()
                .any(|entry| entry == &format!("close:{}", running.0)),
            "shutdown left the parked host running: {log:?}"
        );
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
        let snapshot = controller.snapshot();
        let failure = snapshot.failure.as_ref().expect("typed failure");
        assert_eq!(failure.kind, FailureKind::Startup);
        assert!(!failure.retryable);
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
    async fn bare_goal_command_opens_without_a_failure() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::InvokeCommand {
                id: CommandId::Builtin(tact_catalog::BuiltinCommand::Goal),
                args: CommandArguments::default(),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert!(controller.snapshot().failure.is_none());
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
        let factory = FakeFactory::deferred_ready();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        controller.when_idle().await.unwrap();
        assert!(
            !factory
                .commands
                .lock()
                .unwrap()
                .iter()
                .any(|command| matches!(command, UserCommand::SetPermissionMode(_))),
            "the permission mode must not be sent before the host is ready"
        );
        factory.emit(HostEvent::Ready);
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
    async fn setting_change_refreshes_the_published_settings() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        controller
            .dispatch(DesktopIntent::SetSetting {
                id: SettingId::MaxTokens,
                value: SettingValue::Text("4096".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(controller.snapshot().settings.max_tokens, Some(4096));
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
    async fn theme_family_setting_persists_without_touching_the_tui_theme() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory).unwrap();
        wait_ready(&controller).await;
        assert_eq!(controller.snapshot().theme, DesktopTheme::Tact);

        controller
            .dispatch(DesktopIntent::SetSetting {
                id: SettingId::ThemeFamily,
                value: SettingValue::Choice("claude".into()),
            })
            .unwrap();
        controller.when_idle().await.unwrap();
        assert_eq!(controller.snapshot().theme, DesktopTheme::Claude);

        let loaded = DesktopConfig::load(tmp.path().join("app-data")).unwrap();
        assert_eq!(loaded.theme, DesktopTheme::Claude);
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
        factory.emit(HostEvent::agent(AgentUpdate::TurnStats {
            run_id: Some("run-1".into()),
            turns_taken: 1,
            max_turns: None,
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
    async fn a_click_on_a_withdrawn_gate_row_still_reaches_the_host() {
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

        // One host batch misses the request, so the local row reads as
        // withdrawn while the host is still waiting for the answer.
        factory.pending.lock().unwrap().clear();
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
        factory.pending.lock().unwrap().push(pending_select(1));

        let epoch = controller.snapshot().session.epoch;
        controller
            .dispatch(DesktopIntent::RespondGate {
                session_epoch: epoch,
                request_id: 1,
                response: GateResponse::Select(Some(0)),
            })
            .unwrap();
        controller.when_idle().await.unwrap();

        let responses = factory
            .commands
            .lock()
            .unwrap()
            .iter()
            .filter(|command| matches!(command, UserCommand::UiResponse(_)))
            .count();
        assert_eq!(
            responses, 1,
            "a drifted gate row must not swallow the click"
        );
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
        controller.when_idle().await.unwrap();
        controller.dispatch(DesktopIntent::CancelRun).unwrap();
        controller.when_idle().await.unwrap();
        // The runtime reports the run finished before it reports the
        // cancellation. Drain the first update on its own so the race is
        // deterministic here instead of depending on CI scheduling.
        factory.emit(HostEvent::agent(AgentUpdate::RunFinished {
            run_id: "run-1".into(),
        }));
        controller.when_idle().await.unwrap();
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
    async fn failed_run_stays_failed_after_run_failed() {
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
                run_id: Some("run-1".into()),
                turns_taken: 1,
                max_turns: None,
            },
            AgentUpdate::MessageStarted {
                message_id: "message-1".into(),
            },
            AgentUpdate::StreamChunk("partial answer".into()),
            AgentUpdate::RunFailed {
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
    async fn pending_compaction_finish_does_not_end_the_active_run() {
        let tmp = tempfile::tempdir().unwrap();
        make_project(tmp.path());
        let factory = FakeFactory::new();
        let (controller, _events) =
            DesktopController::start_with_factory(config(tmp.path()), factory.clone()).unwrap();
        wait_ready(&controller).await;
        for update in [
            AgentUpdate::RunStarted {
                run_id: "run-active".into(),
            },
            AgentUpdate::TurnStats {
                run_id: Some("run-active".into()),
                turns_taken: 1,
                max_turns: None,
            },
            AgentUpdate::StepStarted {
                idx: 0,
                tool_id: "tool-active".into(),
                tool_name: "bash".into(),
                arg_summary: "sleep".into(),
                arg_full: "sleep".into(),
                presentation: tact_protocol::ToolPresentationInfo::generic("bash"),
            },
            AgentUpdate::RunStarted {
                run_id: "run-compact".into(),
            },
            AgentUpdate::RunFinished {
                run_id: "run-compact".into(),
            },
        ] {
            factory.emit(HostEvent::agent(update));
        }
        controller.when_idle().await.unwrap();

        let snapshot = controller.snapshot();
        assert_eq!(snapshot.runtime.current_phase, RunPhase::RunningTools);
        assert!(snapshot.transcript.iter().any(|row| matches!(
            &row.body,
            crate::RowBody::ToolGroup(group)
                if group.items.iter().any(|item| matches!(
                    item,
                    crate::ToolGroupItem::Tool(tool)
                        if tool.tool_id.as_ref() == "tool-active"
                            && tool.status == crate::RowStatus::Running
                ))
        )));
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
