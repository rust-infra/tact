use std::sync::Arc;

use async_trait::async_trait;
use tact::store::{DynSessionStore, open_sqlite_session_store};
use tact_host::{
    HostCloseError, HostExit, HostExtensionService, HostSendError, HostSession, HostSessionConfig,
    HostStartError, SessionLockRegistry, SessionSelection, TryHostEvent,
};
use tact_protocol::UserCommand;

use crate::{DesktopConfig, InitialSession, ProjectRecord};

#[async_trait]
pub(crate) trait HostBackend: Send {
    fn session_id(&self) -> &str;
    fn try_next_event(&mut self) -> TryHostEvent;
    fn send(&self, command: UserCommand) -> Result<(), HostSendError>;
    fn pending_requests(&self) -> Vec<tact::ui_responder::PendingUiRequest>;
    fn extension_host(&self) -> HostExtensionService;
    async fn close(&mut self) -> Result<HostExit, HostCloseError>;
}

#[async_trait]
impl HostBackend for HostSession {
    fn session_id(&self) -> &str {
        HostSession::session_id(self)
    }

    fn try_next_event(&mut self) -> TryHostEvent {
        HostSession::try_next_event(self)
    }

    fn send(&self, command: UserCommand) -> Result<(), HostSendError> {
        HostSession::send(self, command)
    }

    fn pending_requests(&self) -> Vec<tact::ui_responder::PendingUiRequest> {
        HostSession::pending_requests(self)
    }

    fn extension_host(&self) -> HostExtensionService {
        HostSession::extension_host(self)
    }

    async fn close(&mut self) -> Result<HostExit, HostCloseError> {
        HostSession::close(self).await
    }
}

#[async_trait]
pub(crate) trait HostFactory: Send + Sync {
    async fn start(
        &self,
        project: &ProjectRecord,
        initial: &InitialSession,
    ) -> Result<Box<dyn HostBackend>, HostStartError>;
}

pub(crate) struct RealHostFactory {
    config: DesktopConfig,
    lock_registry: Arc<SessionLockRegistry>,
}

impl RealHostFactory {
    pub fn new(config: DesktopConfig) -> Self {
        Self {
            config,
            lock_registry: crate::session_lock::registry(),
        }
    }
}

#[async_trait]
impl HostFactory for RealHostFactory {
    async fn start(
        &self,
        project: &ProjectRecord,
        initial: &InitialSession,
    ) -> Result<Box<dyn HostBackend>, HostStartError> {
        let store = open_store(&project.root)
            .await
            .map_err(|error| HostStartError::Persistence(error.to_string()))?;
        let selection = match initial {
            InitialSession::New => SessionSelection::New,
            InitialSession::ResumeLast => SessionSelection::ResumeLast,
            InitialSession::Resume(id) => SessionSelection::Resume(id.clone()),
            InitialSession::None => SessionSelection::New,
        };
        let host = HostSession::start(HostSessionConfig {
            tact_path: tact::consts::TactPath::new(project.root.clone()),
            work_dir: project.root.clone(),
            session: selection,
            args: self.config.cli_args(),
            session_store: store,
            lock_registry: self.lock_registry.clone(),
            progress_capacity: 256,
            account_polling: tact_host::account::is_supported(),
        })
        .await?;
        Ok(Box::new(host))
    }
}

async fn open_store(root: &std::path::Path) -> anyhow::Result<DynSessionStore> {
    let path = root.join(".tact/tact.db");
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    open_sqlite_session_store(&path).await
}
