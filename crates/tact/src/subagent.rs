//! Subagent run lifecycle (asynchronous subagent tracking).
//!
//! A `spawn_subagent` call that sets `run_in_background: true` returns an
//! `async_launched { id }` handle immediately while the child keeps running in
//! a detached task. This module persists that run's lifecycle to
//! `<workdir>/.tact/tact.db` (the `subagent_runs` table) so a restart can tell
//! "still running" from "finished":
//!
//! - [`SubagentManager`] owns the SQLite store and performs orphan repair.
//! - [`SharedSubagentManager`] is the thread-safe wrapper used by tools.
//! - [`SubagentRun`] captures the child session id, status, summary, and
//!   start/finish timestamps.
//!
//! The persisted record is the **source of truth for crash recovery**; the
//! in-memory `pending_subagent_results` queue in [`crate::agent::AgentRuntime`]
//! is the live fast path for same-process re-injection.

use std::{
    collections::HashMap,
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::subagent_store::{SqliteSubagentStore, SubagentStore};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

impl From<SubagentStatus> for &'static str {
    fn from(status: SubagentStatus) -> Self {
        match status {
            SubagentStatus::Running => "running",
            SubagentStatus::Completed => "completed",
            SubagentStatus::Failed => "failed",
            SubagentStatus::Cancelled => "cancelled",
        }
    }
}

impl TryFrom<&str> for SubagentStatus {
    type Error = anyhow::Error;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "running" => Ok(Self::Running),
            "completed" => Ok(Self::Completed),
            "failed" => Ok(Self::Failed),
            "cancelled" => Ok(Self::Cancelled),
            other => anyhow::bail!("invalid subagent status in database: {other}"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentRun {
    pub child_id: String,
    pub status: SubagentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

/// A finished background subagent's summary, queued for re-injection into the
/// parent's conversation on its next turn.
#[derive(Debug, Clone)]
pub struct SubagentResult {
    pub child_id: String,
    pub summary: String,
    pub success: bool,
}

pub struct SubagentManager {
    records: Arc<dyn SubagentStore>,
    /// Live cancel handles for running children, keyed by child session id.
    /// Registered by `spawn_subagent` so a `cancel_subagent` tool / slash
    /// command / TUI button can flip the child's cooperative cancel flag.
    cancel_handles: std::sync::Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl std::fmt::Debug for SubagentManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubagentManager").finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct SharedSubagentManager {
    inner: Arc<SubagentManager>,
}

impl SubagentManager {
    /// Creates a manager backed by the given SQLite database file.
    ///
    /// Repairs orphans on startup: any record still marked `running` belongs
    /// to a process that no longer exists, so it is rewritten as `failed`
    /// (`"Process interrupted (agent restarted)"`). Finished results are not
    /// re-delivered automatically — the model re-spawns or resumes as it sees
    /// fit.
    pub async fn new(db_path: &Path) -> Result<Self> {
        let records: Arc<dyn SubagentStore> = Arc::new(SqliteSubagentStore::new(db_path).await?);
        for mut record in records.list_running().await? {
            record.status = SubagentStatus::Failed;
            record.finished_at = Some(Utc::now());
            record.summary = Some("Process interrupted (agent restarted)".to_string());
            records.upsert(&record).await?;
        }

        Ok(Self {
            records,
            cancel_handles: std::sync::Mutex::new(HashMap::new()),
        })
    }

    /// Records the start of a child run (`status = running`).
    pub async fn start(&self, child_id: String) -> Result<()> {
        let record = SubagentRun {
            child_id,
            status: SubagentStatus::Running,
            summary: None,
            started_at: Utc::now(),
            finished_at: None,
        };
        self.records.upsert(&record).await
    }

    /// Records the terminal outcome of a child run, preserving `started_at`.
    pub async fn finish(&self, child_id: &str, success: bool, summary: String) -> Result<()> {
        let mut record = self
            .records
            .get(child_id)
            .await?
            .with_context(|| format!("unknown subagent run {child_id}"))?;
        record.status = if success {
            SubagentStatus::Completed
        } else {
            SubagentStatus::Failed
        };
        record.summary = Some(summary);
        record.finished_at = Some(Utc::now());
        self.records.upsert(&record).await
    }

    /// Records a cooperative cancellation.
    pub async fn cancel(&self, child_id: &str) -> Result<()> {
        let mut record = self
            .records
            .get(child_id)
            .await?
            .with_context(|| format!("unknown subagent run {child_id}"))?;
        record.status = SubagentStatus::Cancelled;
        record.finished_at = Some(Utc::now());
        self.records.upsert(&record).await
    }

    /// Registers the cooperative cancel flag for a running child so external
    /// entry points (tool / slash command / TUI button) can request its
    /// cancellation. Replaces any previous handle for the same id.
    pub fn register_cancel_handle(&self, child_id: &str, flag: Arc<AtomicBool>) {
        self.cancel_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(child_id.to_string(), flag);
    }

    /// Removes the cancel handle for a finished child.
    pub fn unregister_cancel_handle(&self, child_id: &str) {
        self.cancel_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(child_id);
    }

    /// Requests cancellation of a running child by flipping its cooperative
    /// flag. Returns `false` when no live handle exists (already finished,
    /// unknown id, or a synchronous child that never registered a handle).
    pub fn request_cancel(&self, child_id: &str) -> bool {
        let flag = self
            .cancel_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(child_id)
            .cloned();
        match flag {
            Some(flag) => {
                flag.store(true, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    /// Requests cancellation of **every** currently running child (used when
    /// the parent agent exits, so background subagents stop instead of
    /// becoming orphans). Returns the number of live handles flipped.
    pub fn cancel_all(&self) -> usize {
        let handles: Vec<Arc<AtomicBool>> = self
            .cancel_handles
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .cloned()
            .collect();
        for flag in &handles {
            flag.store(true, Ordering::Relaxed);
        }
        handles.len()
    }

    /// Requests cancellation of every live child and persists `cancelled`
    /// immediately. Shutdown uses this instead of waiting for detached child
    /// tasks, which may be dropped when the Tokio runtime exits.
    pub async fn cancel_all_and_persist(&self) -> usize {
        let child_ids: Vec<String> = {
            let handles = self
                .cancel_handles
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            for flag in handles.values() {
                flag.store(true, Ordering::Relaxed);
            }
            handles.keys().cloned().collect()
        };

        for child_id in &child_ids {
            // Synchronous children have cancel handles but no subagent_runs
            // row; their errors are intentionally ignored here.
            let _ = self.cancel(child_id).await;
        }
        child_ids.len()
    }

    /// Reads a single run record by child session id (`None` when unknown).
    pub async fn get(&self, child_id: &str) -> Result<Option<SubagentRun>> {
        self.records.get(child_id).await
    }

    /// Blocks until the child run reaches a terminal status
    /// (`Completed` / `Failed` / `Cancelled`), or `timeout_ms` elapses.
    ///
    /// This is the Codex `wait_agent` analog: the parent can spawn N
    /// background subagents, then `wait_subagent` on each instead of polling
    /// `check_subagent` across multiple turns. Returns a human-readable
    /// summary line on completion, or a "still running" note on timeout.
    pub async fn wait(&self, child_id: &str, timeout_ms: u64) -> Result<String> {
        const POLL_INTERVAL_MS: u64 = 250;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let record = self
                .records
                .get(child_id)
                .await?
                .with_context(|| format!("Unknown subagent {child_id}"))?;
            match record.status {
                SubagentStatus::Running => {}
                _ => {
                    let summary = record.summary.as_deref().unwrap_or("(no summary)");
                    return Ok(format!(
                        "{}: {:?} {summary}",
                        record.child_id, record.status
                    ));
                }
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(format!(
                    "{child_id}: still running (timed out after {timeout_ms} ms)"
                ));
            }
            tokio::time::sleep(Duration::from_millis(POLL_INTERVAL_MS)).await;
        }
    }

    /// Human/LLM-facing status query, mirroring `check_background`.
    ///
    /// `Some(id)` returns a single run as pretty JSON; `None` lists all runs
    /// one line per run, sorted by start time.
    pub async fn check(&self, child_id: Option<&str>) -> Result<String> {
        if let Some(child_id) = child_id {
            let record = self
                .records
                .get(child_id)
                .await?
                .with_context(|| format!("Unknown subagent {child_id}"))?;
            return serde_json::to_string_pretty(&record)
                .context("failed to serialize subagent run");
        }

        let mut records = self.records.list().await?;
        if records.is_empty() {
            return Ok("No subagent runs.".to_string());
        }
        records.sort_by_key(|record| record.started_at);
        Ok(records
            .into_iter()
            .map(|record| {
                let summary = record.summary.as_deref().unwrap_or("(no summary)");
                format!("{}: {:?} {summary}", record.child_id, record.status)
            })
            .collect::<Vec<_>>()
            .join("\n"))
    }
}

impl SharedSubagentManager {
    pub fn new(manager: SubagentManager) -> Self {
        Self {
            inner: Arc::new(manager),
        }
    }

    pub async fn start(&self, child_id: String) -> Result<()> {
        self.inner.start(child_id).await
    }

    pub async fn finish(&self, child_id: &str, success: bool, summary: String) -> Result<()> {
        self.inner.finish(child_id, success, summary).await
    }

    pub async fn cancel(&self, child_id: &str) -> Result<()> {
        self.inner.cancel(child_id).await
    }

    pub fn register_cancel_handle(&self, child_id: &str, flag: Arc<AtomicBool>) {
        self.inner.register_cancel_handle(child_id, flag);
    }

    pub fn unregister_cancel_handle(&self, child_id: &str) {
        self.inner.unregister_cancel_handle(child_id);
    }

    pub fn request_cancel(&self, child_id: &str) -> bool {
        self.inner.request_cancel(child_id)
    }

    pub fn cancel_all(&self) -> usize {
        self.inner.cancel_all()
    }

    pub async fn cancel_all_and_persist(&self) -> usize {
        self.inner.cancel_all_and_persist().await
    }

    pub async fn check(&self, child_id: Option<&str>) -> Result<String> {
        self.inner.check(child_id).await
    }

    pub async fn get(&self, child_id: &str) -> Result<Option<SubagentRun>> {
        self.inner.get(child_id).await
    }

    pub async fn wait(&self, child_id: &str, timeout_ms: u64) -> Result<String> {
        self.inner.wait(child_id, timeout_ms).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::subagent_store::SqliteSubagentStore;

    fn temp_db(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-subagent-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tact.db")
    }

    #[tokio::test]
    async fn marks_stale_running_subagents_on_startup() {
        let db = temp_db("orphan_repair");
        // Seed a Running row directly through the store (simulating a crash).
        {
            let store = SqliteSubagentStore::new(&db).await.unwrap();
            store
                .upsert(&SubagentRun {
                    child_id: "deadbeef".to_string(),
                    status: SubagentStatus::Running,
                    summary: None,
                    started_at: Utc::now(),
                    finished_at: None,
                })
                .await
                .unwrap();
        }
        // Rebuild the manager — orphan repair rewrites Running → Failed.
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        let output = manager.check(Some("deadbeef")).await.unwrap();
        assert!(output.contains("failed"), "output: {output}");
        assert!(
            output.contains("Process interrupted (agent restarted)"),
            "output: {output}"
        );
    }

    #[tokio::test]
    async fn start_finish_check_round_trips() {
        let db = temp_db("lifecycle");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        manager.start("child-1".to_string()).await.unwrap();
        manager
            .finish("child-1", true, "all done".to_string())
            .await
            .unwrap();
        let output = manager.check(Some("child-1")).await.unwrap();
        assert!(output.contains("completed"), "output: {output}");
        assert!(output.contains("all done"), "output: {output}");
    }

    #[tokio::test]
    async fn check_lists_empty_when_no_runs() {
        let db = temp_db("empty");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        assert_eq!(manager.check(None).await.unwrap(), "No subagent runs.");
    }

    #[tokio::test]
    async fn cancel_transitions_to_cancelled() {
        let db = temp_db("cancel");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        manager.start("child-1".to_string()).await.unwrap();
        manager.cancel("child-1").await.unwrap();
        let output = manager.check(Some("child-1")).await.unwrap();
        assert!(output.contains("cancelled"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_returns_summary_on_completion() {
        let db = temp_db("wait_done");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        manager.start("child-1".to_string()).await.unwrap();

        // Complete the run from a background task after a short delay.
        let mgr = manager.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            mgr.finish("child-1", true, "done summary".to_string())
                .await
                .unwrap();
        });

        let output = manager.wait("child-1", 2_000).await.unwrap();
        assert!(output.contains("Completed"), "output: {output}");
        assert!(output.contains("done summary"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_times_out_while_still_running() {
        let db = temp_db("wait_timeout");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        manager.start("child-1".to_string()).await.unwrap();

        let output = manager.wait("child-1", 100).await.unwrap();
        assert!(output.contains("still running"), "output: {output}");
    }

    #[tokio::test]
    async fn wait_errors_for_unknown_child() {
        let db = temp_db("wait_unknown");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        assert!(manager.wait("ghost", 100).await.is_err());
    }

    #[test]
    fn cancel_handle_register_request_unregister() {
        let db = temp_db("cancel_handle");
        let manager = SharedSubagentManager::new(
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(SubagentManager::new(&db))
                .unwrap(),
        );

        // Unknown id has no handle.
        assert!(!manager.request_cancel("ghost"));

        // Registered handle is flipped by request_cancel.
        let flag = Arc::new(AtomicBool::new(false));
        manager.register_cancel_handle("child-1", flag.clone());
        assert!(manager.request_cancel("child-1"));
        assert!(flag.load(Ordering::Relaxed), "flag must be set");

        // Unregister removes the handle; a second request fails.
        manager.unregister_cancel_handle("child-1");
        assert!(!manager.request_cancel("child-1"));
    }

    #[tokio::test]
    async fn cancel_all_and_persist_marks_running_children_cancelled() {
        let db = temp_db("cancel_all_persisted");
        let manager = SharedSubagentManager::new(SubagentManager::new(&db).await.unwrap());
        manager.start("child-1".to_string()).await.unwrap();
        manager.start("child-2".to_string()).await.unwrap();

        let f1 = Arc::new(AtomicBool::new(false));
        let f2 = Arc::new(AtomicBool::new(false));
        manager.register_cancel_handle("child-1", f1.clone());
        manager.register_cancel_handle("child-2", f2.clone());

        let count = manager.cancel_all_and_persist().await;

        assert_eq!(count, 2);
        assert!(f1.load(Ordering::Relaxed));
        assert!(f2.load(Ordering::Relaxed));
        assert!(
            manager
                .check(Some("child-1"))
                .await
                .unwrap()
                .contains("cancelled")
        );
        assert!(
            manager
                .check(Some("child-2"))
                .await
                .unwrap()
                .contains("cancelled")
        );
    }

    #[test]
    fn cancel_all_flips_every_live_handle() {
        let db = temp_db("cancel_all");
        let manager = SharedSubagentManager::new(
            tokio::runtime::Runtime::new()
                .unwrap()
                .block_on(SubagentManager::new(&db))
                .unwrap(),
        );
        let f1 = Arc::new(AtomicBool::new(false));
        let f2 = Arc::new(AtomicBool::new(false));
        manager.register_cancel_handle("child-1", f1.clone());
        manager.register_cancel_handle("child-2", f2.clone());

        let count = manager.cancel_all();

        assert_eq!(count, 2, "both live handles must be flipped");
        assert!(f1.load(Ordering::Relaxed));
        assert!(f2.load(Ordering::Relaxed));

        // A second cancel_all has nothing left (handles still registered but
        // already true — cancel_all re-flips; count stays 2). Unregister then
        // verify it drops to 0.
        manager.unregister_cancel_handle("child-1");
        manager.unregister_cancel_handle("child-2");
        assert_eq!(manager.cancel_all(), 0);
    }
}
