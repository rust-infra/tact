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
        // Same validation as the `bash` tool: worktree_run executes arbitrary
        // shell inside the lane and must not become a validation bypass.
        crate::shell::validate_shell_command(command)?;
        let record = self.get(name).await?;
        let output = Command::new("sh")
            .current_dir(&record.path)
            .arg("-c")
            .arg(command)
            .output()
            .with_context(|| format!("failed to run command in {}", record.path))?;
        // Audit every invocation so `worktree_events` shows who ran what.
        self.store
            .append_event(&format!("{} worktree.run {name} {command}", Utc::now()))
            .await?;
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
    /// (which refuses a dirty tree — no `--force`), then attempts to delete
    /// the backing branch `wt/<name>` with `git branch -d` — **only** when
    /// the branch is fully merged, so unmerged commits are never destroyed.
    /// Finally deletes the tracking record and appends an audit event.
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

        // Delete the backing branch only if it is fully merged (`-d`, never
        // `-D`). A branch with unmerged commits is kept — the record removal
        // below must still succeed so the lane disappears from the index.
        let branch = record.branch.clone();
        let branch_deleted = Command::new("git")
            .current_dir(&self.repo_root)
            .args(["branch", "-d", &branch])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .context("failed to run git branch -d")?
            .success();

        self.store.remove_worktree(name).await?;
        let outcome = if branch_deleted {
            format!("removed worktree {name} (branch {branch} deleted)")
        } else {
            format!("removed worktree {name} (branch {branch} kept: unmerged)")
        };
        self.store
            .append_event(&format!(
                "{} worktree.remove {name} branch_deleted={branch_deleted}",
                Utc::now()
            ))
            .await?;
        Ok(outcome)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tool::test_support::test_context;

    /// Runs a git command in `dir`, asserting success.
    async fn git_run(dir: &Path, args: &[&str]) {
        let out = tokio::process::Command::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .await
            .expect("git command should run");
        assert!(
            out.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// Initialises a git repo with one commit so `git worktree add` has a HEAD.
    async fn init_git_repo(dir: &Path) {
        git_run(dir, &["init", "-q"]).await;
        std::fs::write(dir.join("README.md"), "hello\n").unwrap();
        git_run(dir, &["add", "."]).await;
        git_run(
            dir,
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=tact-test",
                "commit",
                "-m",
                "init",
            ],
        )
        .await;
    }

    #[tokio::test]
    async fn run_rejects_dangerous_commands() {
        let context = test_context("worktree-run-validate");
        init_git_repo(&context.work_dir).await;
        context
            .worktree_manager
            .create("lane".into(), None, "HEAD".into(), "sess".into())
            .await
            .unwrap();

        let err = context
            .worktree_manager
            .run("lane", "sudo rm -rf /")
            .await
            .expect_err("dangerous command must be blocked");
        assert!(
            err.to_string().contains("blocked"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn run_audits_invocations() {
        let context = test_context("worktree-run-audit");
        init_git_repo(&context.work_dir).await;
        context
            .worktree_manager
            .create("lane".into(), None, "HEAD".into(), "sess".into())
            .await
            .unwrap();

        context
            .worktree_manager
            .run("lane", "echo audit-me")
            .await
            .unwrap();

        let events = context.worktree_manager.events(10).await.unwrap();
        assert!(
            events.contains("worktree.run lane echo audit-me"),
            "run must be audit-logged: {events}"
        );
    }

    #[tokio::test]
    async fn remove_deletes_merged_branch_and_keeps_unmerged() {
        let context = test_context("worktree-remove-branch");
        init_git_repo(&context.work_dir).await;
        context
            .worktree_manager
            .create("merged-lane".into(), None, "HEAD".into(), "sess".into())
            .await
            .unwrap();
        let _merged = context.worktree_manager.get("merged-lane").await.unwrap();

        // Merged: the lane branch points at HEAD (the main branch has all its
        // commits), so `git branch -d` succeeds and the branch is removed.
        let out = context
            .worktree_manager
            .remove("merged-lane")
            .await
            .unwrap();
        assert!(
            out.contains("branch wt/merged-lane deleted"),
            "merged branch should be deleted: {out}"
        );
        let status = tokio::process::Command::new("git")
            .current_dir(&context.work_dir)
            .args(["branch", "--list", "wt/merged-lane"])
            .output()
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).trim().is_empty(),
            "wt/merged-lane should no longer exist"
        );

        // Unmerged: commit on the lane, then remove — the branch is kept.
        context
            .worktree_manager
            .create("unmerged-lane".into(), None, "HEAD".into(), "sess".into())
            .await
            .unwrap();
        let unmerged = context.worktree_manager.get("unmerged-lane").await.unwrap();
        git_run(
            Path::new(&unmerged.path),
            &[
                "-c",
                "user.email=test@example.com",
                "-c",
                "user.name=tact-test",
                "commit",
                "--allow-empty",
                "-m",
                "lane-only work",
            ],
        )
        .await;
        let out = context
            .worktree_manager
            .remove("unmerged-lane")
            .await
            .unwrap();
        assert!(
            out.contains("branch wt/unmerged-lane kept"),
            "unmerged branch must be kept: {out}"
        );
        let status = tokio::process::Command::new("git")
            .current_dir(&context.work_dir)
            .args(["branch", "--list", "wt/unmerged-lane"])
            .output()
            .await
            .unwrap();
        assert!(
            String::from_utf8_lossy(&status.stdout).contains("wt/unmerged-lane"),
            "unmerged branch should survive removal"
        );
    }
}
