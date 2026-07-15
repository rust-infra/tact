//! Skill (custom instruction) loading.
//!
//! Skills are markdown files (`SKILL.md`) nested in subdirectories under one or
//! more skill roots. Discovery order (later wins on name clash):
//!
//! - legacy: `<workdir>/skills/`
//! - user:   `~/.tact/skills/`
//! - project: `<workdir>/.claude/skills/`
//!
//! Each file has optional YAML frontmatter for `name` and `description`
//! (Agent Skills–compatible). Bodies are unrestricted; TUI slash invoke may
//! additionally substitute Claude Code–style bare `$ARGUMENTS`.
//!
//! - [`SkillRegistry`] scans skill directories, parses frontmatter, and
//!   provides lookup by name.
//! - [`get_skill_registry`] / [`shared_skill_registry`] construct registries;
//!   interactive mode shares [`SharedSkillRegistry`] between agent tools and the TUI
//!   so `/skill-reload` updates both without restart.
//! - [`SkillRegistry::describe_available`] supplies name/description lines for
//!   the system prompt (not full bodies).
//! - Full bodies are wrapped in `<skill>` XML for `load_skill` tool results
//!   and for TUI slash invocation (user task).

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use anyhow::Result;
use serde::Deserialize;
use tracing::warn;
use walkdir::WalkDir;

use crate::consts::TactPath;

pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
}

pub struct SkillDocument {
    pub manifest: SkillManifest,
    pub body: String,
}

impl std::fmt::Display for SkillDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            r#"<skill name="{}">
{}
</skill>"#,
            self.manifest.name, self.body
        )
    }
}

/// Shared registry used by the agent tools and (in interactive mode) the TUI.
pub type SharedSkillRegistry = Arc<Mutex<SkillRegistry>>;

/// Build a registry for `workdir` by scanning Claude-compatible skill roots
/// (plus legacy `<workdir>/skills` for backward compatibility).
pub fn get_skill_registry(workdir: impl AsRef<Path>) -> Result<SkillRegistry> {
    let dirs = TactPath::new(workdir.as_ref()).skill_search_dirs();
    let mut registry = SkillRegistry::new(dirs);
    registry.load_skills()?;
    Ok(registry)
}

/// Load skills into a mutex-backed registry shared across agent + TUI.
pub fn shared_skill_registry(workdir: impl AsRef<Path>) -> Result<SharedSkillRegistry> {
    Ok(Arc::new(Mutex::new(get_skill_registry(workdir)?)))
}

/// Lock the shared skill registry (recovers from poison).
pub fn lock_skills(reg: &SharedSkillRegistry) -> MutexGuard<'_, SkillRegistry> {
    reg.lock().unwrap_or_else(|e| e.into_inner())
}

pub struct SkillRegistry {
    skill_dirs: Vec<PathBuf>,
    skills: HashMap<String, SkillDocument>,
}

impl SkillRegistry {
    pub fn new(skill_dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            skill_dirs: skill_dirs.into_iter().collect(),
            skills: HashMap::new(),
        }
    }

    pub fn load_skills(&mut self) -> Result<()> {
        self.skills.clear();

        // Later directories win on name clash: legacy → user → project.
        let dirs = self.skill_dirs.clone();
        for dir in dirs {
            self.load_skills_from_dir(&dir)?;
        }

        Ok(())
    }

    fn load_skills_from_dir(&mut self, skills_dir: &Path) -> Result<()> {
        if !skills_dir.exists() {
            return Ok(());
        }

        for entry in WalkDir::new(skills_dir)
            .into_iter()
            .filter_map(|r| match r {
                Ok(e) => Some(e),
                Err(e) => {
                    warn!("skipping skill dir entry: {e}");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_file())
            .filter(|entry| entry.file_name().to_str() == Some("SKILL.md"))
        {
            let path = entry.path();

            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    warn!("can't read skill file {}: {e}", path.display());
                    continue;
                }
            };

            let (meta, body) = parse_frontmatter(&content);
            let fallback_name = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let name = meta.name.unwrap_or(fallback_name);
            let description = meta
                .description
                .unwrap_or_else(|| "No description".to_string());

            let document = SkillDocument {
                manifest: SkillManifest {
                    name: name.clone(),
                    description,
                    path: path.to_path_buf(),
                },
                body,
            };

            self.skills.insert(name, document);
        }

        Ok(())
    }

    /// List available skills with name + description (metadata only).
    pub fn describe_available(&self) -> String {
        if self.skills.is_empty() {
            return "(no skills available)".to_string();
        }

        let mut names = self.skills.keys().cloned().collect::<Vec<_>>();
        names.sort();

        names
            .into_iter()
            .filter_map(|name| {
                self.skills.get(&name).map(|skill| {
                    format!("- {}: {}", skill.manifest.name, skill.manifest.description)
                })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// List available skills with full body injected (Claude Code style).
    pub fn describe_available_with_body(&self) -> String {
        if self.skills.is_empty() {
            return "(no skills available)".to_string();
        }

        let mut names = self.skills.keys().cloned().collect::<Vec<_>>();
        names.sort();

        names
            .into_iter()
            .filter_map(|name| self.skills.get(&name).map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn load_full_text(&self, name: &str) -> String {
        match self.skills.get(name) {
            Some(skill) => skill.to_string(),
            None => {
                let mut names = self.skills.keys().cloned().collect::<Vec<_>>();
                names.sort();
                format!(
                    "Error: Unknown skill '{}'. Available: {}",
                    name,
                    names.join(", ")
                )
            }
        }
    }

    pub fn skills(&self) -> &HashMap<String, SkillDocument> {
        &self.skills
    }

    pub fn skill_dirs(&self) -> &[PathBuf] {
        &self.skill_dirs
    }
}

#[derive(Debug, Default, Deserialize)]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

fn parse_frontmatter(text: &str) -> (SkillFrontmatter, String) {
    let text = text.replace("\r\n", "\n");

    let Some(rest) = text.strip_prefix("---\n") else {
        return (SkillFrontmatter::default(), text.trim().to_string());
    };

    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return (SkillFrontmatter::default(), text.trim().to_string());
    };

    let meta = serde_yaml::from_str::<SkillFrontmatter>(frontmatter).unwrap_or_default();

    (meta, body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_frontmatter_with_lf_line_endings() {
        let input = "---\nname: test\ndescription: hello\n---\n\nbody";
        let (meta, body) = parse_frontmatter(input);

        assert_eq!(meta.name.as_deref(), Some("test"));
        assert_eq!(meta.description.as_deref(), Some("hello"));
        assert_eq!(body, "body");
    }

    #[test]
    fn parses_frontmatter_with_crlf_line_endings() {
        let input = "---\r\nname: test\r\ndescription: hello\r\n---\r\n\r\nbody";
        let (meta, body) = parse_frontmatter(input);

        assert_eq!(meta.name.as_deref(), Some("test"));
        assert_eq!(meta.description.as_deref(), Some("hello"));
        assert_eq!(body, "body");
    }

    fn write_skill(root: &Path, name: &str, description: &str, body: &str) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\n{body}"),
        )
        .unwrap();
    }

    #[test]
    fn loads_from_project_claude_skills_dir() {
        let dir = tempdir().unwrap();
        let project_skills = dir.path().join(".claude/skills");
        write_skill(&project_skills, "deploy", "Deploy playbook", "step 1");

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(registry.skills().contains_key("deploy"));
        assert!(registry.load_full_text("deploy").contains("step 1"));
    }

    #[test]
    fn loads_legacy_workdir_skills_dir() {
        let dir = tempdir().unwrap();
        let legacy = dir.path().join("skills");
        write_skill(&legacy, "old", "Legacy skill", "legacy body");

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(registry.skills().contains_key("old"));
    }

    #[test]
    fn project_skill_overrides_legacy_same_name() {
        let dir = tempdir().unwrap();
        write_skill(&dir.path().join("skills"), "style", "legacy", "LEGACY");
        write_skill(
            &dir.path().join(".claude/skills"),
            "style",
            "project",
            "PROJECT",
        );

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(registry.load_full_text("style").contains("PROJECT"));
        assert!(!registry.load_full_text("style").contains("LEGACY"));
    }

    #[test]
    fn shared_registry_reload_updates_in_place() {
        let dir = tempdir().unwrap();
        let unique = format!("reload-demo-{}", std::process::id());
        let shared = shared_skill_registry(dir.path()).unwrap();
        assert!(
            !lock_skills(&shared).skills().contains_key(&unique),
            "fresh temp workdir should not already contain {unique}"
        );

        write_skill(
            &dir.path().join(".claude/skills"),
            &unique,
            "Deploy",
            "v1 body",
        );
        {
            let mut reg = lock_skills(&shared);
            *reg = get_skill_registry(dir.path()).unwrap();
            assert!(reg.load_full_text(&unique).contains("v1 body"));
        }
        // Same Arc still visible to other holders.
        assert!(
            lock_skills(&shared)
                .load_full_text(&unique)
                .contains("v1 body")
        );
    }
}
