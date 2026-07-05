//! Background (asynchronous) task execution.
//!
//! Background tasks run shell commands asynchronously via `tokio::spawn`.
//! Results are persisted to disk and can be polled at any time.
//!
//! - [`BackgroundManager`] owns the in-memory task map and the on-disk
//!   collection store.
//! - [`SharedBackgroundManager`] is the thread-safe wrapper used by tool
//!   implementations.
//! - [`BackgroundTaskRecord`] captures the command, status, start/finish
//!   timestamps, and combined stdout+stderr output.

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tokio::time::timeout;

use crate::store::{CollectionStore, StoreRoot};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundTaskStatus {
    Running,
    Completed,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackgroundTaskRecord {
    pub id: String,
    pub status: BackgroundTaskStatus,
    pub command: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
    #[serde(default)]
    pub output: String,
}

#[derive(Debug)]
pub struct BackgroundManager {
    records: CollectionStore<BackgroundTaskRecord>,
    tasks: Mutex<HashMap<String, BackgroundTaskRecord>>,
    next_id: AtomicU64,
}

#[derive(Clone, Debug)]
pub struct SharedBackgroundManager {
    inner: Arc<BackgroundManager>,
}

impl SharedBackgroundManager {
    pub fn new(root: &StoreRoot) -> Result<Self> {
        let records = root.collection::<BackgroundTaskRecord>("background/tasks")?;
        let mut tasks = HashMap::new();
        for mut record in records.list()? {
            if record.status == BackgroundTaskStatus::Running {
                record.status = BackgroundTaskStatus::Error;
                record.finished_at = Some(Utc::now());
                record.output = "Process interrupted (agent restarted)".to_string();
                records.write(&record.id, &record)?;
            }
            tasks.insert(record.id.clone(), record);
        }

        Ok(Self {
            inner: Arc::new(BackgroundManager {
                records,
                tasks: Mutex::new(tasks),
                next_id: AtomicU64::new(Utc::now().timestamp_millis().max(0) as u64),
            }),
        })
    }

    pub fn run(&self, command: String, work_dir: &Path) -> Result<String> {
        crate::shell::validate_shell_command(&command)?;

        let id = format!("{:08x}", self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let record = BackgroundTaskRecord {
            id: id.clone(),
            status: BackgroundTaskStatus::Running,
            command: command.clone(),
            started_at: Utc::now(),
            finished_at: None,
            output: String::new(),
        };
        self.save_record(record.clone())?;

        let manager = self.clone();
        let command_for_task = command.clone();
        let work_dir = work_dir.to_path_buf();
        tokio::spawn(async move {
            const MAX_OUTPUT_CHARS: usize = 50_000;
            const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

            let output = timeout(
                COMMAND_TIMEOUT,
                Command::new("sh")
                    .arg("-c")
                    .arg(&command_for_task)
                    .current_dir(&work_dir)
                    .stdout(std::process::Stdio::piped())
                    .stderr(std::process::Stdio::piped())
                    .kill_on_drop(true)
                    .output(),
            )
            .await;
            let mut record = record;
            record.finished_at = Some(Utc::now());
            match output {
                Ok(Ok(output)) => {
                    record.status = if output.status.success() {
                        BackgroundTaskStatus::Completed
                    } else {
                        BackgroundTaskStatus::Error
                    };
                    let combined = format!(
                        "{}{}",
                        String::from_utf8_lossy(&output.stdout),
                        String::from_utf8_lossy(&output.stderr)
                    );
                    record.output = combined.chars().take(MAX_OUTPUT_CHARS).collect();
                }
                Ok(Err(error)) => {
                    record.status = BackgroundTaskStatus::Error;
                    record.output = error.to_string();
                }
                Err(_) => {
                    record.status = BackgroundTaskStatus::Error;
                    record.output = format!("Error: Timeout ({COMMAND_TIMEOUT:?})");
                }
            }
            let _ = manager.save_record(record);
        });

        Ok(format!("Background task {id} started: {command}"))
    }

    pub fn check(&self, task_id: Option<&str>) -> Result<String> {
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .map_err(|_| anyhow::anyhow!("background manager lock poisoned"))?;

        if let Some(task_id) = task_id {
            let record = tasks
                .get(task_id)
                .cloned()
                .or_else(|| self.inner.records.read(task_id).ok())
                .with_context(|| format!("Unknown background task {task_id}"))?;
            return serde_json::to_string_pretty(&record).context("failed to serialize task");
        }

        if tasks.is_empty() {
            for record in self.inner.records.list()? {
                tasks.insert(record.id.clone(), record);
            }
        }

        if tasks.is_empty() {
            return Ok("No background tasks.".to_string());
        }
        let mut records = tasks.values().cloned().collect::<Vec<_>>();
        records.sort_by_key(|record| record.started_at);
        Ok(records
            .into_iter()
            .map(|record| format!("{}: {:?} {}", record.id, record.status, record.command))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    fn save_record(&self, record: BackgroundTaskRecord) -> Result<()> {
        self.inner.records.write(&record.id, &record)?;
        let mut tasks = self
            .inner
            .tasks
            .lock()
            .map_err(|_| anyhow::anyhow!("background manager lock poisoned"))?;
        tasks.insert(record.id.clone(), record);
        Ok(())
    }
}

impl std::ops::Deref for SharedBackgroundManager {
    type Target = Arc<BackgroundManager>;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::StoreRoot;
    use tempfile::TempDir;

    #[test]
    fn marks_stale_running_tasks_on_startup() {
        let tmp = TempDir::new().unwrap();
        let root = StoreRoot::new(tmp.path()).unwrap();
        let records = root
            .collection::<BackgroundTaskRecord>("background/tasks")
            .unwrap();
        records
            .write(
                "deadbeef",
                &BackgroundTaskRecord {
                    id: "deadbeef".to_string(),
                    status: BackgroundTaskStatus::Running,
                    command: "sleep 999".to_string(),
                    started_at: Utc::now(),
                    finished_at: None,
                    output: String::new(),
                },
            )
            .unwrap();

        let manager = SharedBackgroundManager::new(&root).unwrap();
        let output = manager.check(Some("deadbeef")).unwrap();

        assert!(output.contains("error"));
        assert!(output.contains("Process interrupted (agent restarted)"));
    }
}
