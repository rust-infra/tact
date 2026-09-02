//! Git worktree management.
//!
//! Worktrees are isolated working directories backed by `git worktree`
//! commands.  Each worktree has a name, path, branch, optional task ID,
//! and status.
//!
//! - [`WorktreeManager`] wraps `git worktree add` and stores metadata in
//!   `<workdir>/.tact/tact.db` (the `worktrees` / `worktree_events` tables).
//! - [`SharedWorktreeManager`] is the thread-safe wrapper.
//! - Supports `create`, `list`, `status`, `run` (execute in-tree), and
//!   `events` (audit log).

use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
};

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::store::worktree_store::{SqliteWorktreeStore, WorktreeStore};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorktreeRecord {
    pub name: String,
    pub path: String,
    pub branch: String,
    pub task_id: Option<u64>,
    pub status: String,
}

pub struct WorktreeManager {
    repo_root: PathBuf,
    store: Box<dyn WorktreeStore>,
}

impl std::fmt::Debug for WorktreeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorktreeManager")
            .field("repo_root", &self.repo_root)
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Debug)]
pub struct SharedWorktreeManager {
    inner: Arc<WorktreeManager>,
}

impl WorktreeManager {
    /// Creates a manager backed by the given SQLite database file.
    pub async fn new(db_path: &Path, repo_root: PathBuf) -> Result<Self> {
        Ok(Self {
            repo_root,
            store: Box::new(SqliteWorktreeStore::new(db_path).await?),
        })
    }

    pub async fn create(
        &self,
        name: String,
        task_id: Option<u64>,
        base_ref: String,
        session_id: String,
    ) -> Result<String> {
        if self.store.find_worktree(&name).await?.is_some() {
            anyhow::bail!("worktree {name} already exists");
        }
        let dir = self.repo_root.join(".worktrees").join(&name);
        let branch = format!("wt/{name}");
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args([
                "worktree",
                "add",
                "-b",
                &branch,
                &dir.display().to_string(),
                &base_ref,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("failed to run git worktree add")?;
        if !output.status.success() {
            let stderr_lossy = String::from_utf8_lossy(&output.stderr);
            let stderr = stderr_lossy.trim();
            if stderr.is_empty() {
                anyhow::bail!("git worktree add failed with {}", output.status);
            }
            anyhow::bail!("git worktree add failed: {stderr}");
        }
        let record = WorktreeRecord {
            name: name.clone(),
            path: dir.display().to_string(),
            branch,
            task_id,
            status: "active".to_string(),
        };
        // Concurrent duplicate: the UNIQUE name constraint rejects the row.
        if !self.store.create_worktree(&record, &session_id).await? {
            anyhow::bail!("worktree {name} already exists");
        }
        self.store
            .append_event(&format!("{} worktree.create {}", Utc::now(), name))
            .await?;
        serde_json::to_string_pretty(&record).context("failed to serialize worktree")
    }

    pub async fn list(&self) -> Result<String> {
        let worktrees = self.store.list_worktrees().await?;
        if worktrees.is_empty() {
            return Ok("No worktrees.".to_string());
        }
        Ok(worktrees
            .into_iter()
            .map(|worktree| format!("{} {} {}", worktree.name, worktree.branch, worktree.path))
            .collect::<Vec<_>>()
            .join("\n"))
    }

    pub async fn status(&self, name: &str) -> Result<String> {
        let record = self.get(name).await?;
        let output = Command::new("git")
            .current_dir(&record.path)
            .arg("status")
            .output()
            .with_context(|| format!("failed to run git status in {}", record.path))?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub async fn run(&self, name: &str, command: &str) -> Result<String> {
        let record = self.get(name).await?;
        let output = Command::new("sh")
            .current_dir(&record.path)
            .arg("-c")
            .arg(command)
            .output()
            .with_context(|| format!("failed to run command in {}", record.path))?;
        Ok(format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    pub async fn events(&self, limit: usize) -> Result<String> {
        Ok(self.store.recent_events(limit).await?.join("\n"))
    }

    /// Removes a tracked worktree: runs `git worktree remove` on its path
    /// (which refuses a dirty tree — no `--force`), deletes the tracking
    /// record, and appends an audit event. The backing branch `wt/<name>` is
    /// left in place so unmerged commits stay recoverable.
    pub async fn remove(&self, name: &str) -> Result<String> {
        let record = self.get(name).await?;
        let output = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["worktree", "remove", &record.path])
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .context("failed to run git worktree remove")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            anyhow::bail!(if stderr.is_empty() {
                format!("git worktree remove failed with {}", output.status)
            } else {
                format!("git worktree remove failed: {stderr}")
            });
        }
        self.store.remove_worktree(name).await?;
        self.store
            .append_event(&format!("{} worktree.remove {}", Utc::now(), name))
            .await?;
        Ok(format!("removed worktree {name}"))
    }

    /// Looks up a tracked worktree by name.
    pub async fn get(&self, name: &str) -> Result<WorktreeRecord> {
        self.store
            .find_worktree(name)
            .await?
            .with_context(|| format!("worktree {name} not found"))
    }
}

impl SharedWorktreeManager {
    pub fn new(manager: WorktreeManager) -> Self {
        Self {
            inner: Arc::new(manager),
        }
    }

    pub async fn create(
        &self,
        name: String,
        task_id: Option<u64>,
        base_ref: String,
        session_id: String,
    ) -> Result<String> {
        self.inner.create(name, task_id, base_ref, session_id).await
    }

    pub async fn list(&self) -> Result<String> {
        self.inner.list().await
    }

    pub async fn status(&self, name: &str) -> Result<String> {
        self.inner.status(name).await
    }

    pub async fn run(&self, name: &str, command: &str) -> Result<String> {
        self.inner.run(name, command).await
    }

    pub async fn events(&self, limit: usize) -> Result<String> {
        self.inner.events(limit).await
    }

    pub async fn remove(&self, name: &str) -> Result<String> {
        self.inner.remove(name).await
    }

    pub async fn get(&self, name: &str) -> Result<WorktreeRecord> {
        self.inner.get(name).await
    }
}
