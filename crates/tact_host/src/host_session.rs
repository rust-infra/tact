//! Sole owner of one live host session.

use std::{
    fmt,
    future::{Future, pending},
    path::PathBuf,
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use tact::{Agent, config::CliArgs, consts::TactPath, store::DynSessionStore};
use tact_protocol::{
    AccountUpdate, AgentUpdate, UpdateReceiver, UpdateSender, UserCommand, update_channel,
};
use tokio::{
    sync::{
        mpsc::{Receiver, Sender, UnboundedReceiver, UnboundedSender, channel, unbounded_channel},
        watch,
    },
    task::JoinHandle,
    time::timeout,
};

use crate::{
    account,
    driver::run_command_loop_with_account,
    extensions::{HostExtensionService, default_plugin_home},
    session_bootstrap::{
        Frontend, Notices, SessionSelection, UiWiring, bootstrap_session, open_session_selected,
    },
    session_lock::{SessionLockGuard, SessionLockRegistry},
};

const DEFAULT_CLOSE_GRACE: Duration = Duration::from_secs(5);

type BuildFuture = Pin<Box<dyn Future<Output = Result<Agent, HostFailure>> + Send>>;
type BuildFn = Box<dyn FnOnce(UpdateSender, UiResponder) -> BuildFuture + Send>;
type DriverFuture = Pin<Box<dyn Future<Output = Result<(), HostFailure>> + Send>>;
type DriverFn = Box<
    dyn FnOnce(
            Agent,
            UnboundedReceiver<UserCommand>,
            Option<UnboundedSender<AccountUpdate>>,
        ) -> DriverFuture
        + Send,
>;

use tact::ui_responder::UiResponder;

/// Everything needed to start one host-owned session.
pub struct HostSessionConfig {
    pub tact_path: TactPath,
    pub work_dir: PathBuf,
    /// Explicit session choice. Process flags cannot override this.
    pub session: SessionSelection,
    pub args: CliArgs,
    pub session_store: DynSessionStore,
    pub lock_registry: Arc<SessionLockRegistry>,
    pub progress_capacity: usize,
    pub account_polling: bool,
}

/// Raw streams for a frontend that still pumps the existing channel-shaped
/// TUI. The host supervisor retains lifecycle ownership, but no longer consumes
/// these streams itself.
pub struct HostFrontendChannels {
    pub agent_rx: UpdateReceiver,
    pub agent_tx: UpdateSender,
    pub account_rx: Option<UnboundedReceiver<AccountUpdate>>,
    pub command_tx: UnboundedSender<UserCommand>,
}

/// One event delivered by the host supervisor.
pub enum HostEvent {
    Agent(Box<AgentUpdate>),
    Account(AccountUpdate),
    Ready,
    StartupFailed(HostFailure),
    Disconnected,
}

impl HostEvent {
    /// Box the agent update: `AgentUpdate` is much larger than every other
    /// variant, so keeping it inline would inflate the whole channel item.
    pub fn agent(update: AgentUpdate) -> Self {
        Self::Agent(Box::new(update))
    }
}

/// Non-panicking host failure categories exposed to frontends.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostFailure {
    Config(String),
    Provider(String),
    Persistence(String),
    Bootstrap(String),
    DriverPanicked(String),
}

/// The result of a close attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostExit {
    pub session_id: String,
    pub cancelled: bool,
}

/// A start failure that happened before the supervisor was created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostStartError {
    Config(String),
    Persistence(String),
    Lock(String),
    Bootstrap(String),
}

impl fmt::Display for HostStartError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(message) => write!(formatter, "host config: {message}"),
            Self::Persistence(message) => write!(formatter, "host persistence: {message}"),
            Self::Lock(message) => write!(formatter, "host lock: {message}"),
            Self::Bootstrap(message) => write!(formatter, "host bootstrap: {message}"),
        }
    }
}

impl std::error::Error for HostStartError {}

fn host_start_error(error: anyhow::Error) -> HostStartError {
    let message = error.to_string();
    if message.contains("is locked by process") || message.contains("already has a live host lock")
    {
        HostStartError::Lock(message)
    } else {
        HostStartError::Persistence(message)
    }
}

/// Sending into a host whose supervisor has gone away.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSendError {
    Closed,
    Disconnected,
}

impl fmt::Display for HostSendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Closed => formatter.write_str("host session is closed"),
            Self::Disconnected => formatter.write_str("host driver is disconnected"),
        }
    }
}

impl std::error::Error for HostSendError {}

/// Close failures are retryable unless the caller drops the handle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostCloseError {
    DrainTimedOut,
    JoinFailed(String),
    LockReleaseFailed(String),
}

impl fmt::Display for HostCloseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DrainTimedOut => formatter.write_str("host driver drain timed out"),
            Self::JoinFailed(message) => {
                write!(formatter, "host supervisor join failed: {message}")
            }
            Self::LockReleaseFailed(message) => {
                write!(formatter, "host session lock release failed: {message}")
            }
        }
    }
}

impl std::error::Error for HostCloseError {}

/// Non-blocking event probe.
#[allow(clippy::large_enum_variant)]
pub enum TryHostEvent {
    Empty,
    Disconnected,
    Available(HostEvent),
}

/// The one owned handle for a live Tact session.
pub struct HostSession {
    session_id: String,
    command_tx: Option<UnboundedSender<UserCommand>>,
    event_rx: Receiver<HostEvent>,
    shutdown_tx: watch::Sender<bool>,
    supervisor: Option<JoinHandle<SupervisorOutcome>>,
    lock: Option<Arc<SessionLockGuard>>,
    lock_registry: Arc<SessionLockRegistry>,
    ui_responder: UiResponder,
    extension_host: HostExtensionService,
    skill_registry: tact::skill::SharedSkillRegistry,
    close_started: bool,
    close_grace: Duration,
}

struct SupervisorOutcome {
    cancelled: bool,
}

impl HostSession {
    /// Start the production host: open/lock the session, create all channels,
    /// then perform the slow bootstrap under a cancellable supervisor.
    pub async fn start(config: HostSessionConfig) -> Result<Self, HostStartError> {
        Self::start_inner(config, false).await.map(|(host, _)| host)
    }

    /// Start with raw TUI streams. The slow bootstrap remains cancellable, but
    /// the terminal renderer keeps its existing channel-shaped event pump.
    pub async fn start_with_frontend_channels(
        config: HostSessionConfig,
    ) -> Result<(Self, HostFrontendChannels), HostStartError> {
        let (host, channels) = Self::start_inner(config, true).await?;
        Ok((
            host,
            channels.expect("frontend channels requested from host start"),
        ))
    }

    async fn start_inner(
        config: HostSessionConfig,
        bridge_frontend: bool,
    ) -> Result<(Self, Option<HostFrontendChannels>), HostStartError> {
        let (session_id, lock) = open_session_selected(
            config.session.clone(),
            &config.args,
            &config.tact_path,
            &config.session_store,
            &config.lock_registry,
        )
        .await
        .map_err(host_start_error)?;

        let tact_path = config.tact_path.clone();
        let work_dir = config.work_dir.clone();
        let store = config.session_store.clone();
        let build_session_id = session_id.clone();
        let release_lock_on_error = lock.clone();
        let skill_registry = match tact::skill::shared_skill_registry(tact_path.workdir()) {
            Ok(registry) => registry,
            Err(error) => {
                let _ = release_lock_on_error.release().await;
                config.lock_registry.unregister_released(&session_id).await;
                return Err(HostStartError::Config(error.to_string()));
            }
        };
        let extension_host = HostExtensionService::new(
            work_dir.clone(),
            skill_registry.clone(),
            default_plugin_home(),
        );
        let build_skill_registry = skill_registry.clone();
        let build = Box::new(move |tx: UpdateSender, responder: UiResponder| {
            let tact_path = tact_path.clone();
            let work_dir = work_dir.clone();
            let store = store.clone();
            let session_id = build_session_id.clone();
            let skill_registry = build_skill_registry.clone();
            Box::pin(async move {
                let notices = Notices::Ui(tx.clone());
                bootstrap_session(
                    &tact_path,
                    work_dir,
                    skill_registry,
                    session_id,
                    store,
                    Frontend::Interactive(UiWiring { tx, responder }),
                    notices,
                )
                .await
                .map_err(|error| HostFailure::Bootstrap(error.to_string()))
            }) as BuildFuture
        });

        let driver_work_dir = config.work_dir.clone();
        let driver = Box::new(move |agent, rx, account_tx| {
            let work_dir = driver_work_dir.clone();
            Box::pin(async move {
                run_command_loop_with_account(agent, rx, work_dir, account_tx).await;
                Ok(())
            }) as DriverFuture
        });

        Ok(Self::spawn(
            session_id,
            lock,
            config.lock_registry.clone(),
            config.progress_capacity,
            config.account_polling,
            build,
            driver,
            DEFAULT_CLOSE_GRACE,
            bridge_frontend,
            extension_host,
            skill_registry,
        ))
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn(
        session_id: String,
        lock: Arc<SessionLockGuard>,
        lock_registry: Arc<SessionLockRegistry>,
        progress_capacity: usize,
        account_polling: bool,
        build: BuildFn,
        driver: DriverFn,
        close_grace: Duration,
        bridge_frontend: bool,
        extension_host: HostExtensionService,
        skill_registry: tact::skill::SharedSkillRegistry,
    ) -> (Self, Option<HostFrontendChannels>) {
        let (agent_tx, agent_rx) = update_channel(progress_capacity);
        let (account_tx, account_rx) = unbounded_channel();
        let (command_tx, command_rx) = unbounded_channel();
        let (event_tx, event_rx) = channel(progress_capacity.max(64));
        let (shutdown_tx, shutdown_rx) = watch::channel(false);
        let responder = UiResponder::new();
        let frontend_agent_tx = agent_tx.clone();
        let frontend_command_tx = command_tx.clone();
        let (supervisor_agent_rx, frontend_agent_rx) = if bridge_frontend {
            (None, Some(agent_rx))
        } else {
            (Some(agent_rx), None)
        };
        let (supervisor_account_rx, frontend_account_rx) = if bridge_frontend {
            (None, account_polling.then_some(account_rx))
        } else {
            (account_polling.then_some(account_rx), None)
        };
        let supervisor = tokio::spawn(supervise(SupervisorConfig {
            agent_tx,
            agent_rx: supervisor_agent_rx,
            account_tx,
            account_rx: supervisor_account_rx,
            command_tx: Some(command_tx.clone()),
            command_rx,
            event_tx,
            shutdown_rx,
            responder: responder.clone(),
            build,
            driver,
            account_polling,
        }));

        let host = Self {
            session_id,
            command_tx: Some(command_tx),
            event_rx,
            shutdown_tx,
            supervisor: Some(supervisor),
            lock: Some(lock),
            lock_registry,
            ui_responder: responder,
            extension_host,
            skill_registry,
            close_started: false,
            close_grace,
        };
        let frontend = frontend_agent_rx.map(|agent_rx| HostFrontendChannels {
            agent_rx,
            agent_tx: frontend_agent_tx,
            account_rx: frontend_account_rx,
            command_tx: frontend_command_tx,
        });
        (host, frontend)
    }

    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// The controller actor is the only production caller.
    pub async fn next_event(&mut self) -> Option<HostEvent> {
        self.event_rx.recv().await
    }

    pub fn try_next_event(&mut self) -> TryHostEvent {
        match self.event_rx.try_recv() {
            Ok(event) => TryHostEvent::Available(event),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => TryHostEvent::Empty,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => TryHostEvent::Disconnected,
        }
    }

    pub fn send(&self, command: UserCommand) -> Result<(), HostSendError> {
        match &self.command_tx {
            Some(tx) => tx.send(command).map_err(|_| HostSendError::Disconnected),
            None => Err(HostSendError::Closed),
        }
    }

    pub fn pending_requests(&self) -> Vec<tact::ui_responder::PendingUiRequest> {
        self.ui_responder.snapshot()
    }

    pub fn ui_responder(&self) -> UiResponder {
        self.ui_responder.clone()
    }

    pub fn extension_host(&self) -> HostExtensionService {
        self.extension_host.clone()
    }

    pub fn skill_registry(&self) -> tact::skill::SharedSkillRegistry {
        self.skill_registry.clone()
    }

    /// Retryable teardown: a timeout leaves this handle and its lock intact.
    pub async fn close(&mut self) -> Result<HostExit, HostCloseError> {
        let Some(supervisor) = self.supervisor.as_mut() else {
            return self.release_lock_and_exit(false).await;
        };

        if !self.close_started {
            self.close_started = true;
            self.ui_responder.shutdown();
            let _ = self.shutdown_tx.send(true);
            self.command_tx.take();
        }

        let event_rx = &mut self.event_rx;
        let outcome = match timeout(self.close_grace, async {
            loop {
                tokio::select! {
                    result = &mut *supervisor => break result,
                    _ = tokio::time::sleep(Duration::from_millis(5)) => {
                        while event_rx.try_recv().is_ok() {}
                    }
                }
            }
        })
        .await
        {
            Ok(Ok(outcome)) => outcome,
            Ok(Err(error)) => {
                self.supervisor = None;
                let _ = self.release_lock_and_exit(false).await;
                return Err(HostCloseError::JoinFailed(error.to_string()));
            }
            Err(_) => return Err(HostCloseError::DrainTimedOut),
        };
        self.supervisor = None;
        self.release_lock_and_exit(outcome.cancelled).await
    }

    async fn release_lock_and_exit(&mut self, cancelled: bool) -> Result<HostExit, HostCloseError> {
        if let Some(lock) = self.lock.as_ref() {
            lock.release()
                .await
                .map_err(|error| HostCloseError::LockReleaseFailed(error.to_string()))?;
        }
        self.lock = None;
        self.lock_registry
            .unregister_released(&self.session_id)
            .await;
        Ok(HostExit {
            session_id: self.session_id.clone(),
            cancelled,
        })
    }

    #[cfg(test)]
    async fn start_for_test(
        config: HostSessionConfig,
        build: BuildFn,
        driver: DriverFn,
        close_grace: Duration,
    ) -> Result<Self, HostStartError> {
        let (session_id, lock) = open_session_selected(
            config.session,
            &config.args,
            &config.tact_path,
            &config.session_store,
            &config.lock_registry,
        )
        .await
        .map_err(host_start_error)?;
        let skill_registry = tact::skill::shared_skill_registry(config.tact_path.workdir())
            .map_err(|error| HostStartError::Config(error.to_string()))?;
        let extension_host = HostExtensionService::new(
            config.work_dir.clone(),
            skill_registry.clone(),
            default_plugin_home(),
        );
        Ok(Self::spawn(
            session_id,
            lock,
            config.lock_registry.clone(),
            config.progress_capacity,
            config.account_polling,
            build,
            driver,
            close_grace,
            false,
            extension_host,
            skill_registry,
        )
        .0)
    }
}

struct SupervisorConfig {
    agent_tx: UpdateSender,
    agent_rx: Option<UpdateReceiver>,
    account_tx: UnboundedSender<AccountUpdate>,
    account_rx: Option<UnboundedReceiver<AccountUpdate>>,
    command_tx: Option<UnboundedSender<UserCommand>>,
    command_rx: UnboundedReceiver<UserCommand>,
    event_tx: Sender<HostEvent>,
    shutdown_rx: watch::Receiver<bool>,
    responder: UiResponder,
    build: BuildFn,
    driver: DriverFn,
    account_polling: bool,
}

async fn supervise(mut config: SupervisorConfig) -> SupervisorOutcome {
    let mut shutdown_rx = config.shutdown_rx.clone();
    let mut build = (config.build)(config.agent_tx.clone(), config.responder.clone());
    let agent = tokio::select! {
        biased;
        result = &mut build => match result {
            Ok(agent) => agent,
            Err(error) => {
                drop(build);
                let _ = config.event_tx.send(HostEvent::StartupFailed(error)).await;
                return SupervisorOutcome { cancelled: false };
            }
        },
        _ = wait_for_shutdown(&mut shutdown_rx) => {
            drop(build);
            return SupervisorOutcome { cancelled: true };
        }
    };

    let _ = config.event_tx.send(HostEvent::Ready).await;
    if let Ok(tasks) = agent
        .tool_context
        .background_manager
        .snapshot(agent.session_id())
        .await
    {
        agent.emit_update(AgentUpdate::BackgroundTasksChanged { tasks });
    }
    if config.account_polling && account::is_supported() {
        let tx = config.account_tx.clone();
        tokio::spawn(async move {
            match account::query_once().await {
                Ok(result) => {
                    let _ = tx.send(account::into_update(result));
                }
                Err(error) => {
                    let _ = tx.send(AccountUpdate::Error(error));
                }
            }
        });
        account::spawn_poller(config.account_tx.clone());
    }

    let account_polling = config.account_polling;
    let mut account_tx = account_polling.then(|| config.account_tx.clone());
    let mut driver = tokio::spawn((config.driver)(
        agent,
        config.command_rx,
        account_tx.clone(),
    ));
    let mut cancelled = false;

    loop {
        tokio::select! {
            biased;
            _ = wait_for_shutdown(&mut shutdown_rx), if !cancelled => {
                cancelled = true;
                if let Some(command_tx) = config.command_tx.as_ref() {
                    let _ = command_tx.send(UserCommand::Cancel);
                }
                config.command_tx.take();
                account_tx.take();
            }
            result = &mut driver => {
                if let Err(error) = result {
                    let _ = config
                        .event_tx
                        .send(HostEvent::StartupFailed(HostFailure::DriverPanicked(
                            error.to_string(),
                        )))
                        .await;
                }
                let _ = config.event_tx.send(HostEvent::Disconnected).await;
                return SupervisorOutcome { cancelled };
            }
            update = recv_agent(&mut config.agent_rx) => {
                let Some(update) = update else {
                    config.agent_rx = None;
                    continue;
                };
                if config
                    .event_tx
                    .send(HostEvent::Agent(Box::new(update)))
                    .await
                    .is_err()
                {
                    request_shutdown(&mut config.command_tx, &mut account_tx);
                    cancelled = true;
                }
            }
            account = recv_account(&mut config.account_rx) => {
                let Some(account) = account else {
                    config.account_rx = None;
                    continue;
                };
                if config.event_tx.send(HostEvent::Account(account)).await.is_err() {
                    request_shutdown(&mut config.command_tx, &mut account_tx);
                    cancelled = true;
                }
            }
        }
    }
}

fn request_shutdown(
    command_tx: &mut Option<UnboundedSender<UserCommand>>,
    account_tx: &mut Option<UnboundedSender<AccountUpdate>>,
) {
    if let Some(command_tx) = command_tx.as_ref() {
        let _ = command_tx.send(UserCommand::Cancel);
    }
    command_tx.take();
    account_tx.take();
}

async fn wait_for_shutdown(rx: &mut watch::Receiver<bool>) {
    if *rx.borrow() {
        return;
    }
    while rx.changed().await.is_ok() {
        if *rx.borrow() {
            return;
        }
    }
    pending::<()>().await;
}

async fn recv_account(rx: &mut Option<UnboundedReceiver<AccountUpdate>>) -> Option<AccountUpdate> {
    match rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

async fn recv_agent(rx: &mut Option<UpdateReceiver>) -> Option<AgentUpdate> {
    match rx {
        Some(rx) => rx.recv().await,
        None => pending().await,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use clap::Parser;
    use tact::{
        config::CliArgs,
        consts::TactPath,
        store::{DynSessionStore, open_sqlite_session_store},
    };
    use tact_llm::MockClient;

    use super::*;
    use crate::test_support::build_test_agent;

    struct Harness {
        _tmp: tempfile::TempDir,
        root: PathBuf,
        store: DynSessionStore,
        registry: Arc<SessionLockRegistry>,
    }

    impl Harness {
        async fn new() -> Self {
            let tmp = tempfile::tempdir().expect("tempdir");
            let root = tmp.path().to_path_buf();
            std::fs::create_dir_all(root.join(".tact")).expect("tact dir");
            let store = open_sqlite_session_store(&root.join(".tact/tact.db"))
                .await
                .expect("session store");
            Self {
                _tmp: tmp,
                root,
                store,
                registry: SessionLockRegistry::new(),
            }
        }

        fn config(&self) -> HostSessionConfig {
            HostSessionConfig {
                tact_path: TactPath::new(self.root.clone()),
                work_dir: self.root.clone(),
                session: SessionSelection::New,
                args: CliArgs::parse_from(["tact"]),
                session_store: self.store.clone(),
                lock_registry: self.registry.clone(),
                progress_capacity: 16,
                account_polling: false,
            }
        }
    }

    async fn assert_lock_conflict(store: &DynSessionStore, session_id: &str) {
        let error = match SessionLockGuard::acquire(store.clone(), session_id).await {
            Ok(lock) => {
                lock.release().await.expect("release unexpected lock");
                panic!("session lock was unexpectedly free");
            }
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("locked by process"),
            "unexpected lock error: {error:#}"
        );
    }

    fn fake_build() -> BuildFn {
        Box::new(|_, _| {
            Box::pin(async {
                let (agent, _) = build_test_agent(MockClient::new(vec![]), None);
                Ok(agent)
            }) as BuildFuture
        })
    }

    #[tokio::test]
    async fn session_switch_cancels_closes_awaits_and_releases_lock() {
        let harness = Harness::new().await;
        let stopped = Arc::new(AtomicBool::new(false));
        let stopped_by_driver = stopped.clone();
        let driver: DriverFn = Box::new(move |_agent, mut rx, _account| {
            Box::pin(async move {
                while let Some(command) = rx.recv().await {
                    if matches!(command, UserCommand::Cancel) {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        stopped_by_driver.store(true, Ordering::SeqCst);
                        return Ok(());
                    }
                }
                Ok(())
            }) as DriverFuture
        });

        let mut host = HostSession::start_for_test(
            harness.config(),
            fake_build(),
            driver,
            Duration::from_secs(1),
        )
        .await
        .expect("host start");
        assert!(matches!(host.next_event().await, Some(HostEvent::Ready)));
        assert_lock_conflict(&harness.store, host.session_id()).await;

        let exit = host.close().await.expect("close");
        assert!(exit.cancelled);
        assert!(stopped.load(Ordering::SeqCst));
        let lock = SessionLockGuard::acquire(harness.store.clone(), &exit.session_id)
            .await
            .expect("lock released after driver joined");
        lock.release().await.expect("probe release");
    }

    #[tokio::test]
    async fn driver_timeout_does_not_release_the_lock_and_close_is_retryable() {
        let harness = Harness::new().await;
        let driver: DriverFn = Box::new(|_agent, mut rx, _account| {
            Box::pin(async move {
                while let Some(command) = rx.recv().await {
                    if matches!(command, UserCommand::Cancel) {
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        return Ok(());
                    }
                }
                Ok(())
            }) as DriverFuture
        });

        let mut host = HostSession::start_for_test(
            harness.config(),
            fake_build(),
            driver,
            Duration::from_millis(20),
        )
        .await
        .expect("host start");
        assert!(matches!(host.next_event().await, Some(HostEvent::Ready)));
        assert!(matches!(
            host.close().await,
            Err(HostCloseError::DrainTimedOut)
        ));
        assert_lock_conflict(&harness.store, host.session_id()).await;

        tokio::time::sleep(Duration::from_millis(120)).await;
        host.close_grace = Duration::from_secs(1);
        let exit = host.close().await.expect("retry close");
        let lock = SessionLockGuard::acquire(harness.store.clone(), &exit.session_id)
            .await
            .expect("retry released the lock");
        lock.release().await.expect("probe release");
    }

    #[tokio::test]
    async fn close_during_startup_drops_bootstrap_and_releases() {
        let harness = Harness::new().await;
        let build: BuildFn = Box::new(|_, _| Box::pin(pending()) as BuildFuture);
        let driver: DriverFn =
            Box::new(|_agent, _rx, _account| Box::pin(async { Ok(()) }) as DriverFuture);
        let mut host =
            HostSession::start_for_test(harness.config(), build, driver, Duration::from_secs(1))
                .await
                .expect("host start");

        let exit = host.close().await.expect("close during startup");
        assert!(exit.cancelled);
        let lock = SessionLockGuard::acquire(harness.store.clone(), &exit.session_id)
            .await
            .expect("startup close released the lock");
        lock.release().await.expect("probe release");
    }

    #[tokio::test]
    async fn close_shuts_down_a_pending_ui_responder() {
        let harness = Harness::new().await;
        let driver: DriverFn = Box::new(|_agent, mut rx, _account| {
            Box::pin(async move {
                while let Some(command) = rx.recv().await {
                    if matches!(command, UserCommand::Cancel) {
                        return Ok(());
                    }
                }
                Ok(())
            }) as DriverFuture
        });
        let mut host = HostSession::start_for_test(
            harness.config(),
            fake_build(),
            driver,
            Duration::from_secs(1),
        )
        .await
        .expect("host start");
        assert!(matches!(host.next_event().await, Some(HostEvent::Ready)));
        let responder = host.ui_responder();
        let (tx, _rx) = unbounded_channel();
        let waiter = tokio::spawn(async move {
            responder
                .request_select(
                    &tx,
                    "pending".into(),
                    vec!["allow".into()],
                    false,
                    tact_protocol::UiRequestPurpose::Permission,
                    tact_protocol::UiRequestOrigin::main(),
                )
                .await
        });
        tokio::task::yield_now().await;
        host.close().await.expect("close");
        assert_eq!(
            waiter.await.expect("waiter task"),
            Err(tact::ui_responder::UiRequestError::Closed)
        );
    }

    #[tokio::test]
    async fn close_join_failure_releases_the_lock_and_stays_retryable() {
        let harness = Harness::new().await;
        let build: BuildFn = Box::new(|_, _| {
            Box::pin(async {
                panic!("bootstrap panic");
                #[allow(unreachable_code)]
                Err(HostFailure::Bootstrap("unreachable".into()))
            }) as BuildFuture
        });
        let driver: DriverFn =
            Box::new(|_agent, _rx, _account| Box::pin(async { Ok(()) }) as DriverFuture);
        let mut host =
            HostSession::start_for_test(harness.config(), build, driver, Duration::from_secs(1))
                .await
                .expect("host start");
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(matches!(
            host.close().await,
            Err(HostCloseError::JoinFailed(_))
        ));
        let lock = SessionLockGuard::acquire(harness.store.clone(), host.session_id())
            .await
            .expect("failed close released the lock");
        lock.release().await.expect("probe release");
        assert!(host.close().await.is_ok());
    }

    #[tokio::test]
    async fn active_lock_conflict_is_a_typed_start_error() {
        let harness = Harness::new().await;
        harness
            .store
            .ensure_session_row("locked-session", &harness.root.display().to_string(), "")
            .await
            .expect("session row");
        let held = SessionLockGuard::acquire(harness.store.clone(), "locked-session")
            .await
            .expect("hold lock");
        let mut config = harness.config();
        config.session = SessionSelection::Resume("locked-session".into());
        let error = match HostSession::start(config).await {
            Ok(_) => panic!("held lock must reject start"),
            Err(error) => error,
        };
        assert!(
            matches!(error, HostStartError::Lock(_)),
            "expected lock error, got {error:?}"
        );
        held.release().await.expect("release lock");
    }
}
