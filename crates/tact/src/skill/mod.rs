//! Skill (custom instruction) loading.
//!
//! Skills are markdown files (`SKILL.md`) nested in subdirectories under one or
//! more skill roots. Discovery order (later wins on name clash):
//!
//! - legacy: `<workdir>/skills/`
//! - user:   `~/.tact/skills/`
//! - project: `<workdir>/.claude/skills/`
//!
//! Each file has optional YAML frontmatter for `name` and `description`.
//!
//! - [`SkillRegistry`] scans skill directories, parses frontmatter, and
//!   provides lookup by name.
//! - [`get_skill_registry`] is the convenience constructor for a workdir.
//! - The rendered skill body is wrapped in `<skill>` XML tags for injection
//!   into tool results.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
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

/// Build a registry for `workdir` by scanning Claude-compatible skill roots
/// (plus legacy `<workdir>/skills` for backward compatibility).
pub fn get_skill_registry(workdir: impl AsRef<Path>) -> Result<SkillRegistry> {
    let dirs = TactPath::new(workdir.as_ref()).skill_search_dirs();
    let mut registry = SkillRegistry::new(dirs);
    registry.load_skills()?;
    Ok(registry)
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
}
