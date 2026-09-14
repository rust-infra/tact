//! Off-event-loop background tasks.
//!
//! Two operations used to run synchronously on the UI thread:
//!
//! 1. `git branch --show-current` for the bottom bar (`git_branch_task`),
//! 2. the skills filesystem walk while holding the shared skill-registry mutex
//!    (`skills_task`).
//!
//! Both now run in `tokio::task::spawn_blocking`, keep their `JoinHandle` so
//! shutdown can abort them, and deliver their result through a `oneshot` the
//! event loop polls each tick. The git refresh keeps its 5-second throttle and
//! both are in-flight-gated (a task is never spawned while one is already
//! running), so no new task is spawned every frame.
//!
//! When no tokio runtime is present (synchronous unit tests) the work is run
//! inline so callers observe the result immediately.

use tokio::sync::oneshot::{self, error::TryRecvError};

use crate::widgets::state::{App, SkillEntry};

/// Why a skills reload was started — selects the completion message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SkillsReloadSource {
    /// `/skill-reload` palette command: reports both success and failure.
    Command,
    /// Plugin install/uninstall follow-up: only failures are reported.
    Plugin,
}

/// Snapshot produced by an off-loop skills reload.
pub(crate) struct SkillsSnapshot {
    pub description: String,
    pub data: Vec<SkillEntry>,
}

/// In-flight git-branch refresh: join handle (for abort) + result receiver.
pub(crate) struct GitBranchTask {
    handle: tokio::task::JoinHandle<()>,
    rx: oneshot::Receiver<String>,
}

/// In-flight skills reload: join handle (for abort) + result receiver.
pub(crate) struct SkillsTask {
    handle: tokio::task::JoinHandle<()>,
    rx: oneshot::Receiver<Result<SkillsSnapshot, String>>,
    source: SkillsReloadSource,
}

impl App {
    /// Refresh the git branch name shown in the bottom bar.
    ///
    /// Throttled to at most once every 5 seconds and in-flight-gated. The `git`
    /// process is forked on a blocking worker, not the event-loop thread; the
    /// result is applied by [`App::poll_background_tasks`].
    pub(crate) fn maybe_refresh_git_branch(&mut self) {
        // In-flight gate: never spawn a second refresh while one is running.
        if self.git_branch_task.is_some() {
            return;
        }
        let now = std::time::Instant::now();
        if self
            .last_git_refresh
            .is_some_and(|t| now.duration_since(t).as_secs() < 5)
        {
            return;
        }
        self.last_git_refresh = Some(now);

        // `git branch --show-current` only reads `.git/HEAD` and is near-instant.
        let branch = || {
            std::process::Command::new("git")
                .args(["branch", "--show-current"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .unwrap_or_else(|| "unknown".to_string())
        };

        match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                let (tx, rx) = oneshot::channel();
                let handle = tokio::task::spawn_blocking(move || {
                    let _ = tx.send(branch());
                });
                self.git_branch_task = Some(GitBranchTask { handle, rx });
            }
            // No runtime (sync unit tests): run inline and apply immediately.
            Err(_) => self.apply_git_branch(branch()),
        }
    }

    fn apply_git_branch(&mut self, branch: String) {
        if branch != self.status_bar_mut().git_branch {
            self.status_bar_mut().git_branch = branch;
            self.dirty = true;
        }
    }

    /// Start an off-loop skills reload. In-flight-gated: a reload already in
    /// progress wins and this call is a no-op.
    pub(crate) fn start_skills_reload(&mut self, source: SkillsReloadSource) {
        if self.skills_task.is_some() {
            return;
        }
        let registry = self.skill_registry.clone();
        let work_dir = self.work_dir.clone();
        match tokio::runtime::Handle::try_current() {
            Ok(_) => {
                let (tx, rx) = oneshot::channel();
                let handle = tokio::task::spawn_blocking(move || {
                    let _ = tx.send(crate::handlers::reload_skills(&registry, &work_dir));
                });
                self.skills_task = Some(SkillsTask { handle, rx, source });
            }
            // No runtime (sync unit tests): run inline and apply immediately.
            Err(_) => {
                let result = crate::handlers::reload_skills(&registry, &work_dir);
                self.apply_skills_reload(source, result);
            }
        }
    }

    fn apply_skills_reload(
        &mut self,
        source: SkillsReloadSource,
        result: Result<SkillsSnapshot, String>,
    ) {
        match result {
            Ok(snapshot) => {
                let count = snapshot.data.len();
                self.skills_description = snapshot.description;
                self.skills_data = snapshot.data;
                // Skill list affects log highlighting; force visual-cache rebuild.
                self.log_scroll.visual_cache_ver = 0;
                if source == SkillsReloadSource::Command {
                    let msgs = self.msgs();
                    let msg = msgs.skill_reloaded_tmpl.replace("{}", &count.to_string());
                    self.add_system_message(msg);
                }
            }
            Err(err) => {
                let msgs = self.msgs();
                let tmpl = match source {
                    SkillsReloadSource::Command => msgs.skill_reload_failed_tmpl,
                    SkillsReloadSource::Plugin => msgs.plugin_reload_failed_tmpl,
                };
                self.add_system_message(tmpl.replace("{}", &err));
            }
        }
        self.dirty = true;
    }

    /// Poll completed background tasks and apply their results. Called once per
    /// event-loop iteration (and on every idle tick).
    pub(crate) fn poll_background_tasks(&mut self) {
        self.poll_git_branch();
        self.poll_skills_reload();
    }

    fn poll_git_branch(&mut self) {
        let outcome = match self.git_branch_task.as_mut() {
            Some(task) => task.rx.try_recv(),
            None => return,
        };
        match outcome {
            Ok(branch) => {
                self.git_branch_task = None;
                self.apply_git_branch(branch);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Closed) => {
                self.git_branch_task = None;
            }
        }
    }

    fn poll_skills_reload(&mut self) {
        let (source, outcome) = match self.skills_task.as_mut() {
            Some(task) => (task.source, task.rx.try_recv()),
            None => return,
        };
        match outcome {
            Ok(result) => {
                self.skills_task = None;
                self.apply_skills_reload(source, result);
            }
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Closed) => {
                self.skills_task = None;
            }
        }
    }

    /// Abort any in-flight background tasks. Called on shutdown so a slow
    /// `git`/filesystem walk cannot outlive the TUI.
    pub(crate) fn abort_background_tasks(&mut self) {
        if let Some(task) = self.git_branch_task.take() {
            task.handle.abort();
        }
        if let Some(task) = self.skills_task.take() {
            task.handle.abort();
        }
    }
}

#[cfg(test)]
mod background_tests {
    use super::{App, SkillsReloadSource};
    use crate::render::test_harness::make_app;

    #[test]
    fn git_branch_refresh_is_throttled() {
        let mut app = make_app();
        app.status_bar_mut().git_branch = "initial".into();
        app.dirty = false;

        // First call: should run git and update last_git_refresh.
        app.maybe_refresh_git_branch();
        let first_refresh = app.last_git_refresh;
        assert!(
            first_refresh.is_some(),
            "first call should set last_git_refresh"
        );
        // The branch is now whatever git reports (in this repo, a real branch name).
        assert_ne!(
            app.status_bar_mut().git_branch,
            "initial",
            "first refresh should replace the placeholder branch"
        );

        // Second call immediately: throttle should prevent re-running git.
        app.maybe_refresh_git_branch();
        assert_eq!(
            app.last_git_refresh, first_refresh,
            "second call within throttle window should not update last_git_refresh"
        );
    }

    #[test]
    fn git_branch_refresh_sets_dirty_on_change() {
        let mut app = make_app();
        // Set the branch to a value that cannot match any real git branch.
        app.status_bar_mut().git_branch = "__nonexistent_branch__".into();
        app.dirty = false;

        app.maybe_refresh_git_branch();
        assert!(
            app.dirty,
            "dirty should be set when git branch differs from stale value"
        );
        assert_ne!(
            app.status_bar_mut().git_branch,
            "__nonexistent_branch__",
            "branch should be updated to real git output"
        );
    }

    #[test]
    fn git_branch_refresh_does_not_set_dirty_when_unchanged() {
        let mut app = make_app();
        // First refresh to get the real branch into status_bar.
        app.maybe_refresh_git_branch();
        let real_branch = app.status_bar_mut().git_branch.clone();
        app.dirty = false;
        // Manually back-date last_git_refresh to bypass the throttle.
        app.last_git_refresh = Some(
            std::time::Instant::now()
                .checked_sub(std::time::Duration::from_secs(10))
                .unwrap_or_else(std::time::Instant::now),
        );

        app.maybe_refresh_git_branch();
        assert!(
            !app.dirty,
            "dirty should stay false when git branch hasn't changed"
        );
        assert_eq!(
            app.status_bar_mut().git_branch,
            real_branch,
            "branch should stay unchanged"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn git_branch_refresh_task_completes_and_applies() {
        let mut app = make_app();
        app.status_bar_mut().git_branch = "stale".into();
        app.dirty = false;

        app.maybe_refresh_git_branch();
        assert!(
            app.git_branch_task.is_some(),
            "under a runtime the git refresh must run off-thread"
        );
        // Never spawn a second refresh while one is in flight.
        let before = app.last_git_refresh;
        app.maybe_refresh_git_branch();
        assert_eq!(app.last_git_refresh, before);

        // Poll until the blocking task reports back.
        for _ in 0..200 {
            app.poll_background_tasks();
            if app.git_branch_task.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            app.git_branch_task.is_none(),
            "git refresh task should complete"
        );
        assert_ne!(app.status_bar_mut().git_branch, "stale");
    }

    fn app_with_skill(work_dir: &std::path::Path, name: &str) -> App {
        let dir = work_dir.join(".tact/skills").join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: test skill\n---\n\n{name} body"),
        )
        .unwrap();
        let mut app = make_app();
        app.work_dir = work_dir.to_path_buf();
        app.skill_registry = std::sync::Arc::new(std::sync::Mutex::new(
            tact::skill::get_skill_registry(work_dir).unwrap(),
        ));
        app
    }

    #[test]
    fn skills_reload_runs_inline_without_a_runtime() {
        // Sync call sites (tests, no reactor) must still observe the reload.
        let temp_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_skill(temp_dir.path(), "existing");
        std::fs::create_dir_all(temp_dir.path().join(".tact/skills/new")).unwrap();
        std::fs::write(
            temp_dir.path().join(".tact/skills/new/SKILL.md"),
            "---\nname: new\ndescription: d\n---\nbody",
        )
        .unwrap();

        app.start_skills_reload(SkillsReloadSource::Command);

        assert!(
            app.skills_task.is_none(),
            "inline path completes immediately"
        );
        assert!(app.skills_data.iter().any(|s| s.name == "new"));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Reloaded")),
            "command reload must report the count: {:?}",
            app.log.items
        );
        // Registry itself is updated (shared with the agent).
        let registry = tact::skill::lock_skills(&app.skill_registry);
        assert!(registry.skills().contains_key("new"));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn skills_reload_runs_off_thread_and_is_in_flight_gated() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_skill(temp_dir.path(), "existing");

        app.start_skills_reload(SkillsReloadSource::Command);
        assert!(
            app.skills_task.is_some(),
            "under a runtime the reload must run off-thread"
        );
        // Second call while in flight must be ignored (no duplicate spawn).
        app.start_skills_reload(SkillsReloadSource::Plugin);
        assert!(app.skills_task.is_some());

        for _ in 0..200 {
            app.poll_background_tasks();
            if app.skills_task.is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(app.skills_task.is_none(), "reload task should complete");
        assert!(app.skills_data.iter().any(|s| s.name == "existing"));
        assert!(
            app.log
                .items
                .iter()
                .any(|item| item.raw.contains("Reloaded")),
            "first (Command) source drives the completion message"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn abort_background_tasks_clears_pending_tasks() {
        let temp_dir = tempfile::tempdir().unwrap();
        let mut app = app_with_skill(temp_dir.path(), "existing");
        app.start_skills_reload(SkillsReloadSource::Plugin);
        app.maybe_refresh_git_branch();
        assert!(app.skills_task.is_some());
        assert!(app.git_branch_task.is_some());
        app.abort_background_tasks();
        assert!(app.git_branch_task.is_none());
        assert!(app.skills_task.is_none());
    }
}
