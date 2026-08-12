//! Cron-style scheduled tasks.
//!
//! Allows the agent to create, list, and delete recurring or one-shot
//! scheduled prompts. Tasks are persisted to `<workdir>/.tact/tact.db`
//! (the `cron_tasks` table).
//!
//! - [`CronScheduler`] is the async facade over [`CronStore`].
//! - [`SharedCronScheduler`] wraps it with `Arc<…>` for concurrent access.
//! - [`ScheduledTaskRecord`] includes the cron expression, prompt text,
//!   flags for `recurring` / `durable`, and the owning session id.

use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::store::cron_store::{CronStore, CronTaskRecord, SqliteCronStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScheduledTaskRecord {
    pub id: String,
    pub cron: String,
    pub prompt: String,
    pub recurring: bool,
    pub durable: bool,
    #[serde(default)]
    pub session_id: String,
    pub created_at: i64,
}

impl From<CronTaskRecord> for ScheduledTaskRecord {
    fn from(record: CronTaskRecord) -> Self {
        Self {
            id: record.id,
            cron: record.cron,
            prompt: record.prompt,
            recurring: record.recurring,
            durable: record.durable,
            session_id: record.session_id,
            created_at: record.created_at,
        }
    }
}

pub struct CronScheduler {
    store: Box<dyn CronStore>,
}

impl std::fmt::Debug for CronScheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CronScheduler").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct SharedCronScheduler {
    inner: Arc<CronScheduler>,
}
impl CronScheduler {
    /// Creates a scheduler backed by the given SQLite database file.
    pub async fn new(db_path: &Path) -> Result<Self> {
        Ok(Self {
            store: Box::new(SqliteCronStore::new(db_path).await?),
        })
    }

    pub async fn create(
        &self,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
        session_id: String,
    ) -> Result<ScheduledTaskRecord> {
        let record = self
            .store
            .create(cron, prompt, recurring, durable, session_id)
            .await?;
        Ok(record.into())
    }

    pub async fn delete(&self, id: &str) -> Result<String> {
        if self.store.delete(id).await? {
            Ok(format!("Deleted scheduled task {id}"))
        } else {
            anyhow::bail!("scheduled task {id} not found");
        }
    }

    pub async fn list(&self) -> Result<String> {
        let mut tasks = self.store.list().await?;
        if tasks.is_empty() {
            return Ok("No scheduled tasks.".to_string());
        }
        tasks.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(tasks
            .into_iter()
            .map(|task| {
                format!(
                    "{} {} [{}{}]: {}",
                    task.id,
                    task.cron,
                    if task.recurring {
                        "recurring"
                    } else {
                        "one-shot"
                    },
                    if task.durable { "/durable" } else { "/session" },
                    task.prompt
                )
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

impl SharedCronScheduler {
    pub fn new(scheduler: CronScheduler) -> Self {
        Self {
            inner: Arc::new(scheduler),
        }
    }

    pub async fn create(
        &self,
        cron: String,
        prompt: String,
        recurring: bool,
        durable: bool,
        session_id: String,
    ) -> Result<String> {
        let task = self
            .inner
            .create(cron, prompt, recurring, durable, session_id)
            .await?;
        serde_json::to_string_pretty(&task).context("failed to serialize scheduled task")
    }

    pub async fn delete(&self, id: &str) -> Result<String> {
        self.inner.delete(id).await
    }

    pub async fn list(&self) -> Result<String> {
        self.inner.list().await
    }
}
