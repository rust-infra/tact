//! The session service a host drives.
//!
//! `SessionService` is the kernel's port; this is the extension crate's
//! implementation over the durable store and its lock. It exists so a host can
//! resolve, list and close sessions without knowing that the transcript lives in
//! SQLite, that a lock is a row with a pid and an epoch, or that "resume last"
//! means "the most recently touched row for this workspace".
//!
//! The lock is the part that has to be right: `open` takes it and `close`
//! releases the *matching* one, because two sessions claiming the same row would
//! interleave their writes.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};
use tact_kernel::runtime::{
    SessionHandle, SessionId, SessionOpen, SessionService, SessionSummary as NeutralSummary,
};
use tokio::sync::Mutex;

use super::session_store::{DynSessionStore, SessionLock};

/// The session service, backed by the durable store.
pub struct RuntimeSessionService {
    store: DynSessionStore,
    /// The locks this service took, by session id.
    ///
    /// Held because `close` receives an opaque handle and must release exactly
    /// the lock that handle's `open` took. A map (not a single slot) because a
    /// host may hold more than one session at a time.
    locks: Mutex<HashMap<String, SessionLock>>,
}

impl RuntimeSessionService {
    pub fn new(store: DynSessionStore) -> Arc<Self> {
        Arc::new(Self {
            store,
            locks: Mutex::new(HashMap::new()),
        })
    }
}

#[async_trait]
impl SessionService for RuntimeSessionService {
    async fn open(&self, workspace: &Path, request: SessionOpen) -> Result<SessionHandle> {
        let root_dir = workspace.display().to_string();
        let id = match request {
            SessionOpen::Resume(id) | SessionOpen::New(id) => id,
            SessionOpen::ResumeLast => {
                // The store already orders by recency; the first row is "last".
                match self.store.list_sessions(Some(&root_dir)).await?.first() {
                    Some(summary) => SessionId::new(summary.id.clone()),
                    // Nothing to resume: a fresh id rather than an error, which
                    // is what a first run in a new workspace is.
                    None => SessionId::new(uuid::Uuid::new_v4().to_string()),
                }
            }
        };

        // The row has to exist before the lock can name it, and the touch is what
        // makes a later `ResumeLast` find this session.
        self.store
            .ensure_session_row(id.as_str(), &root_dir, "")
            .await?;
        let lock = SessionLock::acquire(self.store.clone(), id.as_str()).await?;
        self.locks
            .lock()
            .await
            .insert(id.as_str().to_string(), lock);
        self.store.touch_session(id.as_str(), &root_dir).await?;
        Ok(SessionHandle { id })
    }

    async fn list(&self, workspace: &Path) -> Result<Vec<NeutralSummary>> {
        let root_dir = workspace.display().to_string();
        Ok(self
            .store
            .list_sessions(Some(&root_dir))
            .await?
            .into_iter()
            .map(|summary| NeutralSummary {
                id: SessionId::new(summary.id),
                root_dir: summary.root_dir,
                created_at: summary.created_at.to_rfc3339(),
                updated_at: summary.updated_at.to_rfc3339(),
                message_count: summary.message_count,
            })
            .collect())
    }

    async fn close(&self, session: SessionHandle) -> Result<()> {
        // Idempotent: a host unwinding through an error path has no way to know
        // whether the lock was already released.
        if let Some(lock) = self.locks.lock().await.remove(session.id.as_str()) {
            lock.release().await?;
        }
        Ok(())
    }
}

/// Provides the session service.
pub struct SessionPlugin {
    service: Arc<RuntimeSessionService>,
}

impl SessionPlugin {
    /// A plugin over an already-opened store.
    ///
    /// Opening is async and happens before assembly, like every other store
    /// plugin: `register` cannot await.
    pub fn new(store: DynSessionStore) -> Self {
        Self {
            service: RuntimeSessionService::new(store),
        }
    }
}

#[async_trait]
impl Plugin for SessionPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.session_service").expect("a literal id"),
            "Session service",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Runtime,
            "Session identity, listing and lock release over the durable store",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        // Registered as a concrete newtype, not as `Arc<RuntimeSessionService>`:
        // a `TypeId` keys the registry, and `Arc<T>` is a different type from
        // `T`, so providing the `Arc` would make the handle unretrievable as the
        // service it implements.
        registrar.service(std::sync::Arc::new(SessionServiceHandle(
            self.service.clone(),
        )))
    }
}

/// The session service a [`SessionPlugin`] provided.
#[derive(Clone)]
pub struct SessionServiceHandle(pub Arc<RuntimeSessionService>);

impl std::ops::Deref for SessionServiceHandle {
    type Target = RuntimeSessionService;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The session service a [`SessionPlugin`] provided.
pub fn session_service(
    services: &tact_kernel::service::ServiceRegistry,
) -> Result<Arc<RuntimeSessionService>> {
    services
        .get::<SessionServiceHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| {
            anyhow::anyhow!("no session service installed: is builtin.session_service installed?")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    async fn store(name: &str) -> (DynSessionStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = super::super::session_store::open_sqlite_session_store(
            &dir.path().join(format!("{name}.db")),
        )
        .await
        .expect("store");
        (store, dir)
    }

    /// The plugin provides the service, and the handle reaches real storage.
    #[tokio::test]
    async fn the_plugin_provides_a_working_service() {
        let (store, dir) = store("provides").await;
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(SessionPlugin::new(store))
            .await
            .expect("install");
        let runtime = host.seal().await.expect("seal");
        let service = session_service(runtime.services()).expect("service");

        let handle = service
            .open(dir.path(), SessionOpen::New(SessionId::new("s1")))
            .await
            .expect("open");
        assert_eq!(handle.id.as_str(), "s1");

        let listed = service.list(dir.path()).await.expect("list");
        assert!(
            listed.iter().any(|summary| summary.id.as_str() == "s1"),
            "an opened session is listed: {listed:?}"
        );
        assert_eq!(listed[0].root_dir, dir.path().display().to_string());

        service.close(handle).await.expect("close");
    }

    /// `ResumeLast` finds the session this workspace opened last.
    ///
    /// This is the one resolution rule a host relies on for `--resume-last`, and
    /// getting it wrong is silent: a fresh id looks like a resumed one until the
    /// user notices their history is gone.
    #[tokio::test]
    async fn resume_last_finds_the_most_recent_session() {
        let (store, dir) = store("resume_last").await;
        let service = RuntimeSessionService::new(store);

        let first = service
            .open(dir.path(), SessionOpen::New(SessionId::new("first")))
            .await
            .expect("open first");
        service.close(first).await.expect("close first");

        let second = service
            .open(dir.path(), SessionOpen::New(SessionId::new("second")))
            .await
            .expect("open second");
        service.close(second).await.expect("close second");

        let resumed = service
            .open(dir.path(), SessionOpen::ResumeLast)
            .await
            .expect("resume last");
        assert_eq!(
            resumed.id.as_str(),
            "second",
            "the most recently opened session is the one resumed"
        );
    }

    /// A workspace with no history starts a fresh session instead of failing.
    ///
    /// A first run in a new project must not have to special-case `--resume-last`.
    #[tokio::test]
    async fn resume_last_with_no_history_starts_a_new_session() {
        let (store, dir) = store("resume_empty").await;
        let service = RuntimeSessionService::new(store);

        let opened = service
            .open(dir.path(), SessionOpen::ResumeLast)
            .await
            .expect("open");
        assert!(
            !opened.id.as_str().is_empty(),
            "a fresh id was minted rather than an error"
        );
        // ...and it became the recorded session, so the next resume finds it.
        let listed = service.list(dir.path()).await.expect("list");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, opened.id);
    }

    /// Closing twice is not an error.
    ///
    /// A host unwinding through an error path cannot know whether the lock was
    /// already released, so it must be able to call `close` again.
    #[tokio::test]
    async fn closing_twice_is_not_an_error() {
        let (store, dir) = store("closes_twice").await;
        let service = RuntimeSessionService::new(store);
        let handle = service
            .open(dir.path(), SessionOpen::New(SessionId::new("s")))
            .await
            .expect("open");
        service.close(handle.clone()).await.expect("first close");
        service.close(handle).await.expect("second close");
    }

    /// Listing is scoped: another workspace's sessions are not offered.
    #[tokio::test]
    async fn listing_does_not_leak_across_workspaces() {
        let (store, dir) = store("scoped").await;
        let service = RuntimeSessionService::new(store);
        let other = tempfile::tempdir().expect("tempdir");

        let handle = service
            .open(dir.path(), SessionOpen::New(SessionId::new("mine")))
            .await
            .expect("open");
        service.close(handle).await.expect("close");

        let mine = service.list(dir.path()).await.expect("list mine");
        assert_eq!(mine.len(), 1);
        let theirs = service.list(other.path()).await.expect("list other");
        assert!(
            theirs.is_empty(),
            "a different workspace sees nothing: {theirs:?}"
        );
    }

    /// The plugin names itself and its kind.
    #[tokio::test]
    async fn the_manifest_names_a_runtime_plugin() {
        let (store, _dir) = store("manifest").await;
        let plugin = SessionPlugin::new(store);
        assert_eq!(plugin.manifest().kind, PluginKind::Runtime);
        assert_eq!(plugin.manifest().id.as_str(), "builtin.session_service");
    }

    /// A host without the plugin is told which one is missing.
    #[tokio::test]
    async fn a_host_without_the_plugin_names_it() {
        let runtime = PluginHost::new(CapabilityFilter::default())
            .seal()
            .await
            .expect("seal");
        let Err(error) = session_service(runtime.services()) else {
            panic!("no plugin means no service");
        };
        assert!(
            error.to_string().contains("builtin.session_service"),
            "{error}"
        );
    }
}
