//! Multi-agent team coordination.
//!
//! Teammates are named agents with a role and status.  They communicate
//! via an inbox that supports point-to-point messages, broadcasts, and
//! structured protocol requests (plan approval, shutdown).
//!
//! - [`TeammateManager`] is the async facade over [`TeamStore`].
//! - [`SharedTeammateManager`] wraps it with `Arc<…>` for concurrent access.
//! - [`InboxMessage`] includes sender, recipient, body, kind, and timestamp.
//! - [`TeammateRecord`] is the roster entry, persisted in `tact.db`.

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::team_store::{SqliteTeamStore, TeamStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeammateRecord {
    pub name: String,
    pub role: String,
    pub status: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxMessage {
    pub from: String,
    pub to: String,
    pub body: String,
    pub kind: String,
    pub created_at: DateTime<Utc>,
}

pub struct TeammateManager {
    store: Box<dyn TeamStore>,
}

impl std::fmt::Debug for TeammateManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TeammateManager").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct SharedTeammateManager {
    inner: Arc<TeammateManager>,
}

impl TeammateManager {
    /// Creates a manager backed by the given SQLite database file.
    pub async fn new(db_path: &Path) -> Result<Self> {
        Ok(Self {
            store: Box::new(SqliteTeamStore::new(db_path).await?),
        })
    }

    pub async fn spawn_teammate(&self, name: String, role: String) -> Result<String> {
        self.store
            .create_teammate(name.clone(), role.clone())
            .await?;
        let record = TeammateRecord {
            name,
            role,
            status: "idle".to_string(),
        };
        serde_json::to_string_pretty(&record).context("failed to serialize teammate")
    }

    pub async fn list_teammates(&self) -> Result<String> {
        let mut teammates = self.store.list_teammates().await?;
        if teammates.is_empty() {
            return Ok("No teammates.".to_string());
        }
        teammates.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(teammates
            .into_iter()
            .map(|teammate| format!("{} [{}] {}", teammate.name, teammate.role, teammate.status))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub async fn send_message(&self, from: String, to: String, body: String) -> Result<String> {
        self.deliver(from, &to, "message", body).await?;
        Ok(format!("sent message to {to}"))
    }

    pub async fn broadcast(&self, from: String, body: String) -> Result<String> {
        let teammates = self.store.list_teammates().await?;
        for teammate in &teammates {
            self.deliver(from.clone(), &teammate.name, "message", body.clone())
                .await?;
        }
        Ok(format!("broadcast to {} teammates", teammates.len()))
    }

    pub async fn read_inbox(&self, owner: &str) -> Result<String> {
        let messages = self.store.read_inbox(owner).await?;
        if messages.is_empty() {
            return Ok("Inbox is empty.".to_string());
        }
        serde_json::to_string_pretty(&messages).context("failed to serialize inbox")
    }

    pub async fn protocol_request(
        &self,
        from: String,
        to: String,
        kind: String,
        body: String,
    ) -> Result<String> {
        self.deliver(from, &to, &kind, body).await?;
        Ok(format!("sent protocol request to {to}"))
    }

    /// Builds an inbox message and appends it to the recipient's inbox.
    async fn deliver(&self, from: String, to: &str, kind: &str, body: String) -> Result<()> {
        let message = InboxMessage {
            from,
            to: to.to_string(),
            body,
            kind: kind.to_string(),
            created_at: Utc::now(),
        };
        self.store.append_message(to, &message).await
    }
}

impl SharedTeammateManager {
    pub fn new(manager: TeammateManager) -> Self {
        Self {
            inner: Arc::new(manager),
        }
    }

    pub async fn spawn_teammate(&self, name: String, role: String) -> Result<String> {
        self.inner.spawn_teammate(name, role).await
    }

    pub async fn list_teammates(&self) -> Result<String> {
        self.inner.list_teammates().await
    }

    pub async fn send_message(&self, from: String, to: String, body: String) -> Result<String> {
        self.inner.send_message(from, to, body).await
    }

    pub async fn broadcast(&self, from: String, body: String) -> Result<String> {
        self.inner.broadcast(from, body).await
    }

    pub async fn read_inbox(&self, owner: &str) -> Result<String> {
        self.inner.read_inbox(owner).await
    }

    pub async fn protocol_request(
        &self,
        from: String,
        to: String,
        kind: String,
        body: String,
    ) -> Result<String> {
        self.inner.protocol_request(from, to, kind, body).await
    }
}

/// Teammates, contributed as a plugin.
///
/// The manager already exists and is shared behind an `Arc`; this is the lifecycle
/// seam. What it buys is replacement: a host that coordinates teammates some other
/// way installs its own plugin instead of replacing what every tool writes against.
pub struct TeamPlugin {
    manager: SharedTeammateManager,
}

impl TeamPlugin {
    /// A plugin over an already-opened manager.
    pub fn new(manager: SharedTeammateManager) -> Self {
        Self { manager }
    }

    /// Open the plugin's own store.
    pub async fn open(db: &std::path::Path) -> anyhow::Result<Self> {
        Ok(Self::new(SharedTeammateManager::new(
            TeammateManager::new(db).await?,
        )))
    }
}

#[async_trait::async_trait]
impl tact_kernel::plugin::Plugin for TeamPlugin {
    fn manifest(&self) -> tact_kernel::plugin::PluginManifest {
        tact_kernel::plugin::PluginManifest::new(
            tact_kernel::plugin::PluginId::new("builtin.team").expect("a literal id"),
            "Team",
            env!("CARGO_PKG_VERSION"),
            tact_kernel::plugin::PluginKind::Capability,
            "Teammates: membership, direct messages and broadcast",
        )
    }

    fn register(&self, registrar: &mut tact_kernel::plugin::Registrar<'_>) -> anyhow::Result<()> {
        registrar.service(std::sync::Arc::new(TeamHandle(self.manager.clone())))
    }
}

/// The manager a [`TeamPlugin`] provided.
#[derive(Clone)]
pub struct TeamHandle(pub SharedTeammateManager);

impl std::ops::Deref for TeamHandle {
    type Target = SharedTeammateManager;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// The teammate manager a host resolved from its registry.
///
/// # Errors
///
/// Returns an error when no `TeamPlugin` was installed. The team tools report that
/// they cannot run rather than silently addressing nobody.
pub fn team_manager(
    services: &tact_kernel::service::ServiceRegistry,
) -> anyhow::Result<SharedTeammateManager> {
    services
        .get::<TeamHandle>()
        .map(|handle| handle.0.clone())
        .ok_or_else(|| anyhow::anyhow!("no team manager installed; install TeamPlugin"))
}

#[cfg(test)]
mod plugin_tests {
    /// Unique per test, so parallel tests in one process do not share a database.
    static TEAM_PLUGIN_DIR_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    use super::*;
    use tact_kernel::capability::CapabilityFilter;
    use tact_kernel::plugin::PluginHost;

    async fn manager() -> SharedTeammateManager {
        let dir = std::env::temp_dir().join(format!(
            "tact-team-plugin-{}-{}",
            std::process::id(),
            TEAM_PLUGIN_DIR_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        SharedTeammateManager::new(TeammateManager::new(&dir.join("team.db")).await.unwrap())
    }

    /// The plugin registers a manager the host can resolve.
    #[tokio::test]
    async fn the_plugin_provides_a_resolvable_manager() {
        let mut host = PluginHost::new(CapabilityFilter::default());
        host.install(TeamPlugin::new(manager().await))
            .await
            .unwrap();
        let runtime = host.seal().await.unwrap();
        assert!(team_manager(runtime.services()).is_ok());
    }

    /// A host with no plugin is told so by name.
    #[test]
    fn a_missing_plugin_is_an_error_that_names_it() {
        let registry = tact_kernel::service::ServiceRegistry::default();
        let error = match team_manager(&registry) {
            Ok(_) => panic!("no manager was installed, so this must fail"),
            Err(error) => error,
        };
        assert!(format!("{error}").contains("TeamPlugin"), "{error}");
    }

    /// The manifest says what it contributes.
    #[tokio::test]
    async fn the_manifest_says_what_it_is() {
        use tact_kernel::plugin::Plugin;
        let manifest = TeamPlugin::new(manager().await).manifest();
        assert_eq!(manifest.id.as_str(), "builtin.team");
        assert_eq!(manifest.kind, tact_kernel::plugin::PluginKind::Capability);
    }
}
