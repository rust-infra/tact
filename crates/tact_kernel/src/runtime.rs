//! The runtime service contracts a host talks to.
//!
//! These are the ports between a *view* and the *runtime*, not between the
//! runtime and its capabilities. A view asks "list the sessions in this
//! workspace" and "close the one you opened"; it never sees how a session is
//! stored, which is why the types here are opaque handles and neutral summaries.
//!
//! They live in the kernel because both a view and every runtime implementation
//! need the same shape. The extension crate implements them; a host drives them.

use std::path::Path;

use async_trait::async_trait;

/// An opaque session identity.
///
/// A newtype rather than a bare `String`: a session id and a run id and a tool
/// id are all strings, and passing the wrong one is a bug the compiler can
/// catch here.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(String);

impl SessionId {
    /// Wrap an id the runtime produced.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// A session a host can list.
///
/// Deliberately shallow: enough to draw a picker row, and nothing that would
/// make a view depend on the transcript format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: SessionId,
    /// The workspace the session was opened in.
    pub root_dir: String,
    pub created_at: String,
    pub updated_at: String,
    pub message_count: i64,
}

/// How a session is being opened.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionOpen {
    /// Continue an existing session by id.
    Resume(SessionId),
    /// Continue the most recent session in the workspace, if there is one.
    ResumeLast,
    /// Start a new session with a caller-chosen id.
    ///
    /// The id is supplied rather than generated here so a host that needs to
    /// know it up front (to label a log line, to take a lock) does not have to
    /// ask twice.
    New(SessionId),
}

/// An open session.
///
/// Holds only what a host passes back to `close`; the runtime keeps the
/// transcript, the lock and the provider state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionHandle {
    pub id: SessionId,
}

/// What a host can do with sessions.
///
/// `open` resolves the id *and* takes the lock, so a host cannot end up with a
/// session it never locked; `close` releases only the matching lock.
#[async_trait]
pub trait SessionService: Send + Sync {
    /// Resolve and open a session in `workspace`.
    async fn open(&self, workspace: &Path, request: SessionOpen) -> anyhow::Result<SessionHandle>;

    /// The sessions recorded for `workspace`, newest first.
    async fn list(&self, workspace: &Path) -> anyhow::Result<Vec<SessionSummary>>;

    /// Release the session's lock.
    ///
    /// Idempotent: closing an already-closed handle is not an error, because a
    /// host that unwinds through an error path has no way to know.
    async fn close(&self, session: SessionHandle) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A session id is opaque but comparable and displayable.
    ///
    /// Display matters: the id is printed in the exit line and in `/sessions`,
    /// and a `Debug`-only type would leak `SessionId("…")` to a user.
    #[test]
    fn a_session_id_displays_its_value() {
        let id = SessionId::new("abc123");
        assert_eq!(id.as_str(), "abc123");
        assert_eq!(id.to_string(), "abc123");
        assert_eq!(id, SessionId::new("abc123"));
        assert_ne!(id, SessionId::new("abc124"));
    }

    /// A stub service can satisfy the trait with no storage.
    ///
    /// This is the property a view test depends on: it can hand the view a
    /// `SessionService` without a database.
    #[tokio::test]
    async fn the_trait_is_implementable_without_storage() {
        struct Memory {
            sessions: std::sync::Mutex<Vec<SessionSummary>>,
        }

        #[async_trait]
        impl SessionService for Memory {
            async fn open(
                &self,
                workspace: &Path,
                request: SessionOpen,
            ) -> anyhow::Result<SessionHandle> {
                let id = match request {
                    SessionOpen::Resume(id) | SessionOpen::New(id) => id,
                    SessionOpen::ResumeLast => SessionId::new("last"),
                };
                self.sessions.lock().expect("lock").push(SessionSummary {
                    id: id.clone(),
                    root_dir: workspace.display().to_string(),
                    created_at: "now".into(),
                    updated_at: "now".into(),
                    message_count: 0,
                });
                Ok(SessionHandle { id })
            }

            async fn list(&self, workspace: &Path) -> anyhow::Result<Vec<SessionSummary>> {
                let root = workspace.display().to_string();
                Ok(self
                    .sessions
                    .lock()
                    .expect("lock")
                    .iter()
                    .filter(|summary| summary.root_dir == root)
                    .cloned()
                    .collect())
            }

            async fn close(&self, _session: SessionHandle) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let service = Memory {
            sessions: std::sync::Mutex::new(Vec::new()),
        };
        let workspace = Path::new("/repo");
        let handle = service
            .open(workspace, SessionOpen::Resume(SessionId::new("s1")))
            .await
            .expect("open");
        assert_eq!(handle.id.as_str(), "s1");
        let listed = service.list(workspace).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, handle.id);
        service.close(handle).await.expect("close");
    }

    /// Listing is scoped to the workspace it was asked about.
    ///
    /// A view that shows another workspace's sessions is worse than one that
    /// shows none: the user would resume a session from the wrong project.
    #[tokio::test]
    async fn listing_is_scoped_to_the_workspace() {
        struct Fixed;
        #[async_trait]
        impl SessionService for Fixed {
            async fn open(
                &self,
                _workspace: &Path,
                _request: SessionOpen,
            ) -> anyhow::Result<SessionHandle> {
                unreachable!("not used")
            }

            async fn list(&self, workspace: &Path) -> anyhow::Result<Vec<SessionSummary>> {
                Ok(vec![SessionSummary {
                    id: SessionId::new("s"),
                    root_dir: workspace.display().to_string(),
                    created_at: String::new(),
                    updated_at: String::new(),
                    message_count: 0,
                }])
            }

            async fn close(&self, _session: SessionHandle) -> anyhow::Result<()> {
                Ok(())
            }
        }

        let listed = Fixed.list(Path::new("/project-a")).await.expect("list");
        assert_eq!(listed[0].root_dir, "/project-a");
    }
}
