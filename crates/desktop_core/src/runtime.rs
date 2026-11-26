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
    async fn next_event(&mut self) -> Option<tact_host::HostEvent>;
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

    async fn next_event(&mut self) -> Option<tact_host::HostEvent> {
        HostSession::next_event(self).await
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

/// Which implementation drives a session.
///
/// The ACP frontend exists for compatibility: every session goes through the
/// same protocol an external ACP client would see, so the desktop renders what
/// any ACP client would render. The direct host stays the default because ACP
/// is a smaller vocabulary — hook rows, `SurfaceSnapshot` readers, and the
/// extensibility surface reach the desktop only on the direct path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    /// In-process `HostSession`; the default and the widest surface.
    Direct,
    /// An ACP agent on a loopback channel. Same protocol, no subprocess.
    #[cfg(feature = "acp-host")]
    AcpInProcess,
    /// An ACP agent in a child process: crash isolation, any ACP agent.
    #[cfg(feature = "acp-host")]
    AcpSubprocess,
}

/// Read the backend out of `TACT_DESKTOP_ACP`.
///
/// `1` or `inprocess` runs the agent in this process; `subprocess` (or `process`)
/// spawns it. Anything else — including `0` and an unset variable — keeps the
/// direct host. The subprocess value is the only one that needs a packaged
/// `tact-acp` binary, which is why it is not the default.
#[cfg(feature = "acp-host")]
fn backend_from_env() -> Backend {
    match std::env::var("TACT_DESKTOP_ACP").ok().as_deref() {
        Some("subprocess" | "process") => Backend::AcpSubprocess,
        Some(value) if value != "0" => Backend::AcpInProcess,
        _ => Backend::Direct,
    }
}

#[cfg(not(feature = "acp-host"))]
const fn backend_from_env() -> Backend {
    Backend::Direct
}

#[async_trait]
impl HostFactory for RealHostFactory {
    async fn start(
        &self,
        project: &ProjectRecord,
        initial: &InitialSession,
    ) -> Result<Box<dyn HostBackend>, HostStartError> {
        match backend_from_env() {
            #[cfg(feature = "acp-host")]
            Backend::AcpSubprocess => {
                return Ok(Box::new(
                    crate::acp_host::AcpHost::start(
                        &self.config,
                        project,
                        initial,
                        self.lock_registry.clone(),
                    )
                    .await?,
                ));
            }
            #[cfg(feature = "acp-host")]
            Backend::AcpInProcess => {
                return Ok(Box::new(
                    crate::acp_inprocess::InProcessAcpHost::start(
                        &self.config,
                        project,
                        initial,
                        self.lock_registry.clone(),
                    )
                    .await?,
                ));
            }
            // Without the feature the two ACP arms above do not exist, so this
            // is the only reachable one.
            _ => {}
        }
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
            extra_mcp_servers: Vec::new(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_override_keeps_the_direct_host() {
        // SAFETY: this test owns the variable for its duration and nothing else
        // in this crate reads it.
        unsafe { std::env::remove_var("TACT_DESKTOP_ACP") };
        assert_eq!(
            backend_from_env(),
            Backend::Direct,
            "an unset variable must leave the direct host in charge"
        );
    }

    #[cfg(feature = "acp-host")]
    #[test]
    fn the_acp_backend_is_chosen_by_name() {
        // SAFETY: this test owns the variable for its duration and nothing else
        // in this crate reads it.
        unsafe { std::env::set_var("TACT_DESKTOP_ACP", "1") };
        assert_eq!(
            backend_from_env(),
            Backend::AcpInProcess,
            "opting in must not require a packaged binary"
        );

        // SAFETY: see above.
        unsafe { std::env::set_var("TACT_DESKTOP_ACP", "subprocess") };
        assert_eq!(
            backend_from_env(),
            Backend::AcpSubprocess,
            "the subprocess transport must be requested by name"
        );

        // SAFETY: see above.
        unsafe { std::env::set_var("TACT_DESKTOP_ACP", "0") };
        assert_eq!(
            backend_from_env(),
            Backend::Direct,
            "an explicit 0 must keep the direct host"
        );

        // SAFETY: see above.
        unsafe { std::env::remove_var("TACT_DESKTOP_ACP") };
    }
}
