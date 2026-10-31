//! Desktop-only recent-project registry.

use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProjectId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectRecord {
    pub id: ProjectId,
    pub root: PathBuf,
    pub display_name: String,
    pub last_opened_at: DateTime<Utc>,
    #[serde(skip)]
    pub available: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct ProjectFile {
    projects: Vec<ProjectRecord>,
}

#[derive(Debug, Clone)]
pub struct ProjectRegistry {
    path: PathBuf,
    projects: Vec<ProjectRecord>,
}

impl ProjectRegistry {
    pub fn open(app_data_dir: impl AsRef<Path>) -> Result<Self> {
        let app_data_dir = app_data_dir.as_ref();
        fs::create_dir_all(app_data_dir).with_context(|| {
            format!(
                "failed to create desktop app-data directory {}",
                app_data_dir.display()
            )
        })?;
        let path = app_data_dir.join("projects.json");
        let mut file = if path.exists() {
            let raw = fs::read_to_string(&path)
                .with_context(|| format!("failed to read {}", path.display()))?;
            serde_json::from_str::<ProjectFile>(&raw)
                .with_context(|| format!("failed to parse {}", path.display()))?
        } else {
            ProjectFile::default()
        };
        for project in &mut file.projects {
            project.available = project.root.is_dir();
        }
        Ok(Self {
            path,
            projects: file.projects,
        })
    }

    pub fn records(&self) -> &[ProjectRecord] {
        &self.projects
    }

    pub fn get(&self, id: &ProjectId) -> Option<&ProjectRecord> {
        self.projects.iter().find(|project| &project.id == id)
    }

    pub fn register(&mut self, root: impl AsRef<Path>) -> Result<ProjectRecord> {
        let root = normalize_root(root.as_ref())?;
        let now = Utc::now();
        if let Some(project) = self
            .projects
            .iter_mut()
            .find(|project| project.root == root)
        {
            project.last_opened_at = now;
            project.available = project.root.is_dir();
            let record = project.clone();
            self.save()?;
            return Ok(record);
        }
        let fallback_name = root.to_string_lossy().into_owned();
        let project = ProjectRecord {
            id: ProjectId(Uuid::new_v4().to_string()),
            display_name: root
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or(&fallback_name)
                .to_string(),
            root,
            last_opened_at: now,
            available: true,
        };
        let project = ProjectRecord {
            available: project.root.is_dir(),
            ..project
        };
        self.projects.push(project.clone());
        self.projects
            .sort_by_key(|project| std::cmp::Reverse(project.last_opened_at));
        self.save()?;
        Ok(project)
    }

    pub fn remove(&mut self, id: &ProjectId) -> Result<bool> {
        let before = self.projects.len();
        self.projects.retain(|project| &project.id != id);
        let removed = self.projects.len() != before;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    fn save(&self) -> Result<()> {
        let file = ProjectFile {
            projects: self.projects.clone(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .context("failed to serialize desktop project registry")?;
        write_atomically(&self.path, &bytes)
    }
}

fn normalize_root(root: &Path) -> Result<PathBuf> {
    if root.as_os_str().is_empty() {
        anyhow::bail!("project root cannot be empty");
    }
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()
            .context("failed to resolve current directory")?
            .join(root)
    };
    Ok(fs::canonicalize(&absolute).unwrap_or(absolute))
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("registry path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let temp = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("projects"),
        std::process::id()
    ));
    fs::write(&temp, bytes).with_context(|| format!("failed to write {}", temp.display()))?;
    fs::rename(&temp, path).with_context(|| {
        let _ = fs::remove_file(&temp);
        format!("failed to replace {}", path.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_registry_deduplicates_canonical_roots() {
        let app_data = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let mut registry = ProjectRegistry::open(app_data.path()).unwrap();

        let first = registry.register(project_root.path()).unwrap();
        #[cfg(unix)]
        {
            let alias = app_data.path().join("alias");
            std::os::unix::fs::symlink(project_root.path(), &alias).unwrap();
            let second = registry.register(&alias).unwrap();
            assert_eq!(first.id, second.id);
        }
        #[cfg(not(unix))]
        {
            let second = registry.register(project_root.path()).unwrap();
            assert_eq!(first.id, second.id);
        }
        assert_eq!(registry.records().len(), 1);
    }

    #[test]
    fn missing_project_remains_visible_unavailable() {
        let app_data = tempfile::tempdir().unwrap();
        let project_parent = tempfile::tempdir().unwrap();
        let project_root = project_parent.path().join("gone");
        fs::create_dir_all(&project_root).unwrap();
        let canonical_root = fs::canonicalize(&project_root).unwrap();
        let mut registry = ProjectRegistry::open(app_data.path()).unwrap();
        registry.register(&project_root).unwrap();
        fs::remove_dir_all(&project_root).unwrap();

        let reopened = ProjectRegistry::open(app_data.path()).unwrap();
        assert_eq!(reopened.records().len(), 1);
        assert!(!reopened.records()[0].available);
        assert_eq!(reopened.records()[0].root, canonical_root);
    }

    #[test]
    fn project_registry_writes_atomically() {
        let app_data = tempfile::tempdir().unwrap();
        let project_root = tempfile::tempdir().unwrap();
        let mut registry = ProjectRegistry::open(app_data.path()).unwrap();
        registry.register(project_root.path()).unwrap();

        let path = app_data.path().join("projects.json");
        let raw = fs::read(&path).unwrap();
        let parsed: ProjectFile = serde_json::from_slice(&raw).unwrap();
        assert_eq!(parsed.projects.len(), 1);
        assert!(fs::read_dir(app_data.path()).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .ends_with(".tmp")
        }));
    }
}

/// Reads the checked-out branch (or short commit) without invoking git.
///
/// The desktop chrome shows the repository state next to the project name;
/// spawning `git` for every snapshot would be wasteful and would fail on
/// machines without a git install.
pub fn repository_branch(root: &Path) -> Option<String> {
    let git_dir = git_dir(root)?;
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let head = head.trim();
    if let Some(reference) = head.strip_prefix("ref: ") {
        Some(reference.trim_start_matches("refs/heads/").to_string())
    } else if head.chars().count() >= 7 {
        // HEAD is normally an ASCII object id, but never slice bytes: a
        // malformed HEAD should not panic the snapshot thread.
        Some(head.chars().take(7).collect())
    } else {
        None
    }
}

/// The `owner/repo` slug from the `origin` remote, when one is configured.
///
/// The title bar shows the repository identity the same way the design does;
/// reading `.git/config` avoids spawning git and works for linked worktrees.
pub fn repository_slug(root: &Path) -> Option<String> {
    let mut dir = git_dir(root)?;
    if let Ok(common) = std::fs::read_to_string(dir.join("commondir")) {
        dir = dir.join(common.trim());
    }
    let config = std::fs::read_to_string(dir.join("config")).ok()?;
    let mut in_origin = false;
    for line in config.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_origin = trimmed.starts_with("[remote \"origin\"]");
            continue;
        }
        if in_origin && let Some(url) = trimmed.strip_prefix("url =") {
            return slug_from_remote(url.trim());
        }
    }
    None
}

fn git_dir(root: &Path) -> Option<std::path::PathBuf> {
    let dot_git = root.join(".git");
    if dot_git.is_dir() {
        return Some(dot_git);
    }
    let contents = std::fs::read_to_string(&dot_git).ok()?;
    let relative = contents
        .lines()
        .find_map(|line| line.strip_prefix("gitdir:"))?
        .trim();
    let path = Path::new(relative);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    })
}

fn slug_from_remote(url: &str) -> Option<String> {
    let cleaned = url.trim().trim_end_matches('/').trim_end_matches(".git");
    // A local-path remote (`/srv/git/tact`, `../tact`) has no owner, and
    // reporting its parent directory as an owner would be a false identity.
    if !(cleaned.starts_with("git@")
        || cleaned.starts_with("ssh://")
        || cleaned.starts_with("http://")
        || cleaned.starts_with("https://")
        || cleaned.starts_with("git://"))
    {
        return None;
    }
    // `git@github.com:owner/repo` uses a colon before the path; normalize it
    // so the last two path segments are always owner and repository.
    let normalized = cleaned.replace(':', "/");
    let segments = normalized
        .rsplit('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if segments.len() < 2 {
        return None;
    }
    Some(format!("{}/{}", segments[1], segments[0]))
}

#[cfg(test)]
mod branch_tests {
    use super::repository_branch;

    #[test]
    fn reads_the_checked_out_branch() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "ref: refs/heads/feat/desktop\n",
        )
        .unwrap();
        assert_eq!(
            repository_branch(dir.path()).as_deref(),
            Some("feat/desktop")
        );
    }

    #[test]
    fn reads_a_detached_head_as_a_short_commit() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/HEAD"),
            "63e6476a9849641da9530e344beae555a69807ec\n",
        )
        .unwrap();
        assert_eq!(repository_branch(dir.path()).as_deref(), Some("63e6476"));
    }

    #[test]
    fn reads_the_origin_slug_from_https() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/config"),
            "[core]\n\trepositoryformatversion = 0\n[remote \"origin\"]\n\turl = https://github.com/rust-infra/tact.git\n",
        )
        .unwrap();
        assert_eq!(
            super::repository_slug(dir.path()).as_deref(),
            Some("rust-infra/tact")
        );
    }

    #[test]
    fn reads_the_origin_slug_from_ssh() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(
            dir.path().join(".git/config"),
            "[remote \"origin\"]\n\turl = git@github.com:rust-infra/tact.git\n",
        )
        .unwrap();
        assert_eq!(
            super::repository_slug(dir.path()).as_deref(),
            Some("rust-infra/tact")
        );
    }

    #[test]
    fn local_path_remotes_have_no_slug() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        for url in [
            "/srv/git/tact.git",
            "../repos/tact",
            "file:///srv/git/tact.git",
        ] {
            std::fs::write(
                dir.path().join(".git/config"),
                format!("[remote \"origin\"]\n\turl = {url}\n"),
            )
            .unwrap();
            assert_eq!(
                super::repository_slug(dir.path()),
                None,
                "{url} must not be reported as an owner/repo identity"
            );
        }
    }

    #[test]
    fn a_non_ascii_head_is_truncated_by_characters() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/HEAD"), "république\n").unwrap();
        assert_eq!(
            repository_branch(dir.path()).as_deref(),
            Some("républi"),
            "byte slicing would panic on this input"
        );
    }

    #[test]
    fn missing_origin_has_no_slug() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::write(dir.path().join(".git/config"), "[core]\n\tbare = false\n").unwrap();
        assert_eq!(super::repository_slug(dir.path()), None);
    }

    #[test]
    fn follows_a_worktree_gitdir_file() {
        let repo = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(repo.path().join("worktrees/feat")).unwrap();
        std::fs::write(
            repo.path().join("worktrees/feat/HEAD"),
            "ref: refs/heads/wt/desktop\n",
        )
        .unwrap();
        let checkout = tempfile::tempdir().unwrap();
        std::fs::write(
            checkout.path().join(".git"),
            format!("gitdir: {}\n", repo.path().join("worktrees/feat").display()),
        )
        .unwrap();
        assert_eq!(
            repository_branch(checkout.path()).as_deref(),
            Some("wt/desktop")
        );
    }
}
