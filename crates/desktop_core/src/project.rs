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
