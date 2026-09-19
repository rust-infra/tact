//! One agent session: a background tokio runtime, the command driver, and the
//! channel ends a front end talks to.
//!
//! A desktop or terminal front end is not itself a tokio program, so the
//! session owns its own runtime on a dedicated thread:
//!
//! 1. [`SessionRuntime::start`] resolves the session id and creates its row
//!    using a short blocking `block_on` — local SQLite work only.
//! 2. The same runtime moves to a named thread, which builds the agent (LLM
//!    client, MCP handshake, hooks — the slow, networked, fallible part) and
//!    then drives commands until the command channel closes.
//!
//! Everything the caller needs is returned synchronously, so the window can be
//! visible before MCP finishes connecting. A startup failure arrives as
//! [`AgentUpdate::Error`] on [`SessionRuntime::events`] rather than as an error
//! from `start`; the receiver channels are executor-agnostic, so a front end may
//! await them on its own executor.

use std::path::{Path, PathBuf};

use tact::{consts::TactPath, store::DynSessionStore};
use tact_protocol::{AccountUpdate, AgentErrorKind, AgentUpdate, UserCommand};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::{account, builder, driver, sessions};

/// What to open when a session starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionOptions {
    /// Working directory the agent is scoped to.
    pub workdir: PathBuf,
    /// Resume this session id. `None` allocates a new one.
    pub session_id: Option<String>,
    /// When no id is given, reopen the most recent session for `workdir`
    /// instead of starting a fresh one.
    pub resume_last: bool,
}

impl SessionOptions {
    /// A fresh session in `workdir`.
    pub fn new(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
            session_id: None,
            resume_last: false,
        }
    }

    /// Resume `session_id` instead of starting a fresh session.
    pub fn resume(workdir: impl Into<PathBuf>, session_id: impl Into<String>) -> Self {
        Self {
            workdir: workdir.into(),
            session_id: Some(session_id.into()),
            resume_last: false,
        }
    }

    /// Reopen the most recent session for the workdir.
    pub fn resume_last(workdir: impl Into<PathBuf>) -> Self {
        Self {
            workdir: workdir.into(),
            session_id: None,
            resume_last: true,
        }
    }
}

/// A running (or still starting) agent session.
pub struct SessionRuntime {
    /// The session id this runtime writes history to.
    session_id: String,
    /// Working directory the agent is scoped to.
    workdir: PathBuf,
    /// Agent updates, in protocol order. Ends when the driver task ends.
    pub events: UnboundedReceiver<AgentUpdate>,
    /// Balance / usage-quota updates, when the provider supports them.
    pub account: UnboundedReceiver<AccountUpdate>,
    /// Commands handed to the driver.
    pub commands: UnboundedSender<UserCommand>,
    /// Reverse channel for `RequestSelect` / `RequestMultiSelect` answers.
    pub ui_responder: tact::ui_responder::UiResponder,
}

impl SessionRuntime {
    /// Start a session and return its handles.
    ///
    /// Fails only for local setup (session store, session row, thread spawn).
    /// Agent construction itself is deferred and reports through `events`.
    pub fn start(options: SessionOptions) -> anyhow::Result<Self> {
        let tact_path = TactPath::new(options.workdir.clone());
        let workdir = tact_path.workdir().to_path_buf();

        let (agent_tx, agent_rx) = unbounded_channel();
        let (account_tx, account_rx) = unbounded_channel();
        let (user_cmd_tx, user_cmd_rx) = unbounded_channel();
        let ui_responder = tact::ui_responder::UiResponder::new();

        // A multi-thread runtime: local setup runs on it here, then it moves to
        // the session thread to drive the agent.
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .thread_name("tact-session")
            .build()?;

        let (session_id, session_store) = runtime.block_on(async {
            let store =
                tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
            let root_dir = tact_path.workdir().display().to_string();
            let session_id = resolve_session_id(&options, &store, &root_dir).await?;
            store.ensure_session_row(&session_id, &root_dir, "").await?;
            store.touch_session(&session_id, &root_dir).await?;
            anyhow::Ok((session_id, store))
        })?;

        let skill_registry = tact::skill::shared_skill_registry(tact_path.workdir())?;
        let thread_name = format!("tact-session-{}", sessions::short_id(&session_id));
        // The driver thread takes ownership of the session identity; the handle
        // below keeps its own copies for the front end.
        let thread_session_id = session_id.clone();
        let thread_workdir = workdir.clone();
        let thread_ui_responder = ui_responder.clone();

        std::thread::Builder::new()
            .name(thread_name)
            .spawn(move || {
                let account_tx = account_tx;
                runtime.block_on(async move {
                    let account_enabled = account::is_supported();
                    let driver_account_tx = if account_enabled {
                        let startup_tx = account_tx.clone();
                        tokio::spawn(async move {
                            match account::query_once().await {
                                Ok(result) => {
                                    let _ = startup_tx.send(account::into_update(result));
                                }
                                Err(err) => {
                                    let _ = startup_tx.send(AccountUpdate::Error(err));
                                }
                            }
                        });
                        account::spawn_poller(account_tx.clone());
                        Some(account_tx)
                    } else {
                        None
                    };

                    let agent = builder::build_agent(
                        tact_path,
                        agent_tx.clone(),
                        skill_registry,
                        thread_session_id,
                        session_store,
                        thread_workdir.clone(),
                        thread_ui_responder,
                    )
                    .await;

                    match agent {
                        Ok(agent) => {
                            driver::run_command_loop_with_account(
                                agent,
                                user_cmd_rx,
                                thread_workdir,
                                driver_account_tx,
                            )
                            .await;
                        }
                        Err(err) => {
                            let _ = agent_tx.send(AgentUpdate::Error(AgentErrorKind::Other(
                                format!("startup failed: {err:#}"),
                            )));
                        }
                    }
                });
            })?;

        Ok(Self {
            session_id,
            workdir,
            events: agent_rx,
            account: account_rx,
            commands: user_cmd_tx,
            ui_responder,
        })
    }

    /// The session id this runtime writes history to.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Working directory the agent is scoped to.
    pub fn workdir(&self) -> &Path {
        &self.workdir
    }
}

/// Resolve which session the caller asked for: an explicit id, the most recent
/// session for the workdir, or a fresh UUID.
async fn resolve_session_id(
    options: &SessionOptions,
    store: &DynSessionStore,
    root_dir: &str,
) -> anyhow::Result<String> {
    if let Some(id) = &options.session_id {
        return Ok(id.clone());
    }
    if options.resume_last {
        let sessions = store.list_sessions(Some(root_dir)).await?;
        if let Some(session) = sessions.into_iter().next() {
            return Ok(session.id);
        }
    }
    Ok(uuid::Uuid::new_v4().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tact_session_resume_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp workspace");
        dir
    }

    async fn store_for(workspace: &Path) -> DynSessionStore {
        let tact_path = TactPath::new(workspace.to_path_buf());
        tact::store::open_sqlite_session_store(&tact_path.session_db_path())
            .await
            .expect("store")
    }

    #[tokio::test]
    async fn an_explicit_id_resumes_that_session() {
        let workspace = temp_workspace();
        let store = store_for(&workspace).await;
        let root = workspace.display().to_string();
        store
            .ensure_session_row("kept-session", &root, "")
            .await
            .expect("row");

        let resolved = resolve_session_id(
            &SessionOptions::resume(&workspace, "kept-session"),
            &store,
            &root,
        )
        .await
        .expect("resolve");

        assert_eq!(resolved, "kept-session", "resume reuses the stored id");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[tokio::test]
    async fn resume_last_reopens_the_only_stored_session() {
        let workspace = temp_workspace();
        let store = store_for(&workspace).await;
        let root = workspace.display().to_string();
        store
            .ensure_session_row("only-session", &root, "")
            .await
            .expect("row");

        let resolved = resolve_session_id(&SessionOptions::resume_last(&workspace), &store, &root)
            .await
            .expect("resolve");

        assert_eq!(resolved, "only-session");
        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[tokio::test]
    async fn a_plain_start_ignores_stored_sessions() {
        let workspace = temp_workspace();
        let store = store_for(&workspace).await;
        let root = workspace.display().to_string();
        store
            .ensure_session_row("existing", &root, "")
            .await
            .expect("row");

        let resolved = resolve_session_id(&SessionOptions::new(&workspace), &store, &root)
            .await
            .expect("resolve");

        assert_ne!(resolved, "existing", "a new session must not adopt history");
        assert_eq!(resolved.len(), 36, "a fresh session gets a UUID");
        let _ = std::fs::remove_dir_all(&workspace);
    }
}
