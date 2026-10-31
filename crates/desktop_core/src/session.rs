//! Project-scoped session listing for the desktop shell.

use std::{path::PathBuf, sync::Arc};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use tact::store::{
    ArchiveFilter, DynSessionStore, SessionListQuery, SessionStore, open_sqlite_session_store,
};
use tokio::sync::OnceCell;

use crate::{ProjectId, ProjectRecord};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSummaryView {
    pub id: String,
    pub project_id: ProjectId,
    pub root_dir: PathBuf,
    pub title: String,
    pub updated_at: DateTime<Utc>,
    pub archived_at: Option<DateTime<Utc>>,
    pub locked: bool,
    pub message_count: i64,
    pub search_score: i32,
}

/// One project's session catalog. The SQLite pool is opened lazily and then
/// retained for the project pane's lifetime.
pub struct SessionCatalog {
    project: ProjectRecord,
    store: OnceCell<DynSessionStore>,
}

impl SessionCatalog {
    pub fn new(project: ProjectRecord) -> Self {
        Self {
            project,
            store: OnceCell::new(),
        }
    }

    pub fn project(&self) -> &ProjectRecord {
        &self.project
    }

    async fn store(&self) -> Result<&DynSessionStore> {
        self.store
            .get_or_try_init(|| async {
                if !self.project.root.is_dir() {
                    anyhow::bail!("project is unavailable: {}", self.project.root.display());
                }
                open_sqlite_session_store(&self.project.root.join(".tact/tact.db"))
                    .await
                    .with_context(|| {
                        format!(
                            "failed to open session store for {}",
                            self.project.root.display()
                        )
                    })
            })
            .await
    }

    pub async fn list(
        &self,
        archive: ArchiveFilter,
        search: &str,
    ) -> Result<Vec<SessionSummaryView>> {
        let store = self.store().await?;
        let summaries = store
            .list_sessions_filtered(SessionListQuery {
                root_dir: Some(self.project.root.display().to_string()),
                archive,
                search: None,
            })
            .await?;
        let mut views: Vec<_> = summaries
            .into_iter()
            .filter_map(|summary| {
                let title = summary
                    .title
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or_else(|| {
                        if summary.message_count == 0 {
                            "New session".to_string()
                        } else {
                            summary.id.clone()
                        }
                    });
                let score = if search.trim().is_empty() {
                    0
                } else {
                    tact_catalog::fuzzy_score(search, &title)
                        .or_else(|| tact_catalog::fuzzy_score(search, &summary.id))?
                };
                Some(SessionSummaryView {
                    id: summary.id,
                    project_id: self.project.id.clone(),
                    root_dir: summary.root_dir.into(),
                    title,
                    updated_at: summary.updated_at,
                    archived_at: summary.archived_at,
                    locked: summary.locked,
                    message_count: summary.message_count,
                    search_score: score,
                })
            })
            .collect();
        views.sort_by(|left, right| {
            right
                .search_score
                .cmp(&left.search_score)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
        });
        Ok(views)
    }

    pub async fn rename(&self, session_id: &str, title: &str) -> Result<()> {
        self.store()
            .await?
            .set_session_title(session_id, title)
            .await
    }

    pub async fn set_archived(&self, session_id: &str, archived: bool) -> Result<()> {
        self.store()
            .await?
            .set_session_archived(session_id, archived)
            .await
    }

    pub fn store_handle(&self) -> Option<Arc<dyn SessionStore>> {
        self.store.get().cloned()
    }
}

#[cfg(test)]
mod tests {
    use tact_agent_api::{AgentMessage, AgentRole, MessageOrigin};

    use super::*;
    use crate::ProjectRegistry;

    #[tokio::test]
    async fn session_catalog_scopes_and_fuzzy_searches() {
        let app_data = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(project.path().join(".tact")).unwrap();
        let store = open_sqlite_session_store(&project.path().join(".tact/tact.db"))
            .await
            .unwrap();
        let canonical_root = std::fs::canonicalize(project.path()).unwrap();
        for (id, title) in [
            ("session-a", "Fix authentication retry"),
            ("session-b", "Update dashboard"),
        ] {
            store
                .ensure_session_row(id, &canonical_root.display().to_string(), "")
                .await
                .unwrap();
            store
                .append_agent_message(
                    id,
                    &AgentMessage::text(AgentRole::User, MessageOrigin::User, title),
                    1,
                )
                .await
                .unwrap();
        }
        let mut registry = ProjectRegistry::open(app_data.path()).unwrap();
        let project = registry.register(project.path()).unwrap();
        let catalog = SessionCatalog::new(project);
        let found = catalog.list(ArchiveFilter::Active, "auth").await.unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "session-a");
    }
}
