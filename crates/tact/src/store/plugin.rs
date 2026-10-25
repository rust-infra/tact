//! Storage and session wiring as plugins.
//!
//! Phase 3's `StoragePlugin`/`SessionPlugin` exist for one reason: the host must
//! be able to assemble an agent without reaching into a global opener. The store
//! is an `Arc<dyn SessionStore>` already, so providing it is a typed registration
//! — no new trait, no adapter.
//!
//! The session store is the *extension-level* contract the spec describes: it
//! keeps model-specific persistence (messages, provider conversation state,
//! request bodies, tool schedules) out of the kernel while still being available
//! to the Agent and Compaction through constructor injection.

use std::sync::Arc;

use anyhow::Result;
use tact_kernel::plugin::{Plugin, PluginId, PluginKind, PluginManifest, Registrar};

use super::session_store::DynSessionStore;

/// Records where the session store lives.
///
/// Constructed after the database is open, because opening is async and a
/// plugin's `register` is not: the host opens the store, then installs this and
/// [`SessionStorePlugin`].
pub struct StoragePlugin {
    path: std::path::PathBuf,
}

impl StoragePlugin {
    /// Name the file the store lives in.
    ///
    /// The store handle itself is provided by [`SessionStorePlugin`]: this
    /// plugin exists so a diagnostic can name the database even in a run that
    /// never builds an agent (`session list`, `stats`).
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

#[async_trait::async_trait]
impl Plugin for StoragePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.storage").expect("a literal id"),
            "Session storage",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Infrastructure,
            "The SQLite session store: messages, provider state, tool schedules",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        // The store itself is provided by `SessionStorePlugin`, under the newtype
        // a `TypeId` can key on. This plugin records *which file* it was opened
        // from, so a diagnostic can name it without the host threading the path
        // alongside the handle.
        registrar.service(Arc::new(StorageLocation(self.path.clone())))
    }
}

impl StoragePlugin {
    /// The file this store lives in, for diagnostics.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }
}

/// The store a [`StoragePlugin`] provided.
///
/// `Arc<dyn SessionStore>` is not `Sized`, so it is registered under a newtype:
/// a `TypeId` needs a concrete type, and registering the bare trait object would
/// make every store in the process the same key.
#[derive(Clone)]
pub struct SessionStoreHandle(pub DynSessionStore);

impl std::ops::Deref for SessionStoreHandle {
    type Target = DynSessionStore;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Provides the session store under a concrete newtype.
pub struct SessionStorePlugin {
    store: DynSessionStore,
}

impl SessionStorePlugin {
    pub fn new(store: DynSessionStore) -> Self {
        Self { store }
    }
}

#[async_trait::async_trait]
impl Plugin for SessionStorePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest::new(
            PluginId::new("builtin.session_store").expect("a literal id"),
            "Session store",
            env!("CARGO_PKG_VERSION"),
            PluginKind::Infrastructure,
            "The extension-level persistence contract the Agent and Compaction share",
        )
    }

    fn register(&self, registrar: &mut Registrar<'_>) -> Result<()> {
        registrar.service(Arc::new(SessionStoreHandle(self.store.clone())))
    }
}

/// The database file the store was opened from.
#[derive(Clone)]
pub struct StorageLocation(pub std::path::PathBuf);

impl StorageLocation {
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

/// Where the store lives, if a [`StoragePlugin`] was installed.
pub fn storage_location(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<std::path::PathBuf> {
    services
        .get::<StorageLocation>()
        .map(|location| location.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no storage installed: is builtin.storage installed?"))
}

/// Pull the session store a plugin provided.
pub fn session_store(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<DynSessionStore> {
    services
        .get::<SessionStoreHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| {
            anyhow::anyhow!("no session store installed: is builtin.session_store installed?")
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-store-plugin-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(".tact").join("tact.db")
    }

    async fn store(name: &str) -> (DynSessionStore, std::path::PathBuf) {
        let path = temp_path(name);
        let store = super::super::session_store::open_sqlite_session_store(&path)
            .await
            .unwrap();
        (store, path)
    }

    #[tokio::test]
    async fn the_store_is_retrievable_after_sealing() {
        let (store, path) = store("retrievable").await;
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(SessionStorePlugin::new(store.clone()))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();
        let resolved = session_store(runtime.services()).unwrap();
        // The same store, not a copy: a session written through one handle is
        // visible through the other.
        resolved
            .create_session("s1", path.to_str().unwrap(), "")
            .await
            .unwrap();
        let sessions = store.list_sessions(None).await.unwrap();
        assert!(
            sessions.iter().any(|summary| summary.id == "s1"),
            "the resolved handle reaches the same database"
        );
    }

    #[tokio::test]
    async fn a_host_without_the_plugin_is_told_which_one_is_missing() {
        let runtime = PluginHost::new(CapabilityFilter::default())
            .seal()
            .await
            .unwrap();
        let Err(error) = session_store(runtime.services()) else {
            panic!("no storage plugin means no store");
        };
        assert!(
            error.to_string().contains("builtin.session_store"),
            "{error}"
        );
    }

    #[tokio::test]
    async fn the_two_plugins_can_both_be_installed() {
        // They are separate on purpose: the path is a diagnostic the host wants
        // even when it never hands the store to an agent (a `session list` run).
        let (store, path) = store("both").await;
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(StoragePlugin::new(&path)).await.unwrap();
        host.install(SessionStorePlugin::new(store)).await.unwrap();
        let runtime = host.seal().await.unwrap();
        assert_eq!(
            storage_location(runtime.services()).unwrap(),
            path,
            "the location names the file the store was opened from"
        );
        assert!(session_store(runtime.services()).is_ok());
    }

    #[tokio::test]
    async fn the_manifest_names_infrastructure() {
        let (store, path) = store("manifest").await;
        let plugin = StoragePlugin::new(&path);
        assert_eq!(plugin.manifest().kind, PluginKind::Infrastructure);
        assert_eq!(plugin.manifest().id.as_str(), "builtin.storage");
        assert_eq!(plugin.path(), path);
        let session = SessionStorePlugin::new(store);
        assert_eq!(session.manifest().kind, PluginKind::Infrastructure);
        assert_eq!(session.manifest().id.as_str(), "builtin.session_store");
    }
}
