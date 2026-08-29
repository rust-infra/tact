//! Skill (custom instruction) loading.
//!
//! Skills are markdown files (`SKILL.md`) nested in subdirectories under one or
//! more skill roots. Discovery order (later wins on name clash):
//!
//! - project-local: `<workdir>/.tact/skills/`
//! - user:          `~/.tact/skills/`
//! - global:        `~/.agents/skills/`
//! - project:       `<workdir>/.claude/skills/`
//! - config:        `[agent].skill_dirs` (in listed order)
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

use crate::{
    consts::{PluginHome, TactPath},
    plugin::{PluginSkillRoot, PluginStore},
};

pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub path: PathBuf,
    /// Claude Code `argument-hint` frontmatter, if present.
    pub argument_hint: Option<String>,
    /// Claude Code `allowed-tools` frontmatter, if present (not yet enforced).
    pub allowed_tools: Option<String>,
    /// Claude Code `model` frontmatter, if present (not yet enforced).
    pub model: Option<String>,
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

/// Build a registry for `workdir` by scanning built-in skill roots, then any
/// `[agent].skill_dirs` from installed process config (when present).
pub fn get_skill_registry(workdir: impl AsRef<Path>) -> Result<SkillRegistry> {
    let workdir = workdir.as_ref();
    let mut dirs = TactPath::new(workdir).skill_search_dirs();
    if let Some(cfg) = crate::config::try_settings() {
        for raw in &cfg.agent.skill_dirs {
            let trimmed = raw.trim();
            if trimmed.is_empty() {
                continue;
            }
            let path = resolve_skill_dir(trimmed, workdir);
            if !dirs.iter().any(|d| d == &path) {
                dirs.push(path);
            }
        }
    }
    let mut registry = SkillRegistry::new(dirs);
    registry.load_skills()?;
    if let Some(plugin_home) = PluginHome::from_environment() {
        let store = PluginStore::new(plugin_home);
        let plugin_roots = store.installed_skill_roots()?;
        registry.load_plugin_skills(&plugin_roots)?;
        // Legacy `commands/*.md` slash commands load after skills so a
        // same-named command wins over the skill (Claude Code precedence).
        for root in store.installed_plugin_roots()? {
            registry.load_plugin_commands(&root.root.join("commands"), &root.plugin_id)?;
        }
    }
    Ok(registry)
}

/// Resolve a configured skill root: `~` / `~/…` via `$HOME`, else relative to `workdir`.
fn resolve_skill_dir(raw: &str, workdir: &Path) -> PathBuf {
    if raw == "~" || raw.starts_with("~/") {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return PathBuf::from(raw);
        };
        return if raw == "~" {
            home
        } else {
            home.join(&raw[2..])
        };
    }
    let path = PathBuf::from(raw);
    if path.is_absolute() {
        path
    } else {
        workdir.join(path)
    }
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

        // Later directories in `skill_dirs` win on name clash.
        let dirs = self.skill_dirs.clone();
        for dir in dirs {
            self.load_skills_from_dir(&dir)?;
        }

        Ok(())
    }

    fn load_plugin_skills(&mut self, plugin_roots: &[PluginSkillRoot]) -> Result<()> {
        for root in plugin_roots {
            self.load_direct_plugin_skills(&root.skills_dir, &root.plugin_id)?;
        }

        Ok(())
    }

    fn load_direct_plugin_skills(&mut self, skills_dir: &Path, plugin_id: &str) -> Result<()> {
        if !skills_dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(skills_dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn!("skipping plugin skill directory entry: {error}");
                    continue;
                }
            };
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let skill = entry.path().join("SKILL.md");
            if skill.is_file() {
                self.load_skill_file(&skill, Some(plugin_id));
            }
        }
        Ok(())
    }

    fn load_skills_from_dir(&mut self, skills_dir: &Path) -> Result<()> {
        self.load_skills_from_dir_with_namespace(skills_dir, None)
    }

    fn load_skills_from_dir_with_namespace(
        &mut self,
        skills_dir: &Path,
        namespace: Option<&str>,
    ) -> Result<()> {
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
            self.load_skill_file(entry.path(), namespace);
        }

        Ok(())
    }

    fn load_skill_file(&mut self, path: &Path, namespace: Option<&str>) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!("can't read skill file {}: {e}", path.display());
                return;
            }
        };

        let (meta, body) = parse_frontmatter(&content);
        let fallback_name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        let local_name = meta.name.unwrap_or(fallback_name);
        let name = namespace
            .map(|plugin_id| format!("{plugin_id}:{local_name}"))
            .unwrap_or(local_name);
        let description = meta
            .description
            .unwrap_or_else(|| "No description".to_string());

        let document = SkillDocument {
            manifest: SkillManifest {
                name: name.clone(),
                description,
                path: path.to_path_buf(),
                argument_hint: meta.argument_hint,
                allowed_tools: meta.allowed_tools,
                model: meta.model,
            },
            body,
        };

        self.skills.insert(name, document);
    }

    /// Loads legacy Claude Code `commands/*.md` slash commands from one
    /// installed plugin as `plugin:<filename-stem>` skills.
    ///
    /// Claude Code treats `commands/*.md` and `skills/<name>/SKILL.md`
    /// identically (only the file layout differs), so both land in the same
    /// registry. Loading commands after skills makes a same-named command win
    /// over the skill, matching Claude's command-over-skill precedence.
    fn load_plugin_commands(&mut self, commands_dir: &Path, plugin_id: &str) -> Result<()> {
        if !commands_dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(commands_dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn!("skipping plugin command dir entry: {error}");
                    continue;
                }
            };
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("md") {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let content = match std::fs::read_to_string(&path) {
                Ok(content) => content,
                Err(error) => {
                    warn!("can't read plugin command {}: {error}", path.display());
                    continue;
                }
            };
            let (meta, body) = parse_frontmatter(&content);
            let name = format!("{plugin_id}:{stem}");
            let document = SkillDocument {
                manifest: SkillManifest {
                    name: name.clone(),
                    description: meta
                        .description
                        .unwrap_or_else(|| "No description".to_string()),
                    path,
                    argument_hint: meta.argument_hint,
                    allowed_tools: meta.allowed_tools,
                    model: meta.model,
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
    /// Claude Code `argument-hint` (shows in /help); parsed for display.
    #[serde(default, rename = "argument-hint")]
    argument_hint: Option<String>,
    /// Claude Code `allowed-tools` (comma list); parsed but not enforced yet.
    #[serde(default, rename = "allowed-tools")]
    allowed_tools: Option<String>,
    /// Claude Code `model` override; parsed but not enforced yet.
    #[serde(default)]
    model: Option<String>,
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
    use std::fs;

    use tempfile::tempdir;

    use super::*;
    use crate::consts::TactPath;
    use crate::plugin::PluginSkillRoot;

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

    fn registry_with_plugins(plugins: &[(&str, &str)]) -> SkillRegistry {
        let dir = tempdir().unwrap();
        let mut roots = Vec::with_capacity(plugins.len());

        for (plugin_id, skill_name) in plugins {
            let skills_dir = dir.path().join(plugin_id).join("skills");
            write_skill(&skills_dir, skill_name, "Plugin skill", "plugin body");
            roots.push(PluginSkillRoot {
                plugin_id: (*plugin_id).to_owned(),
                skills_dir,
            });
        }

        let mut registry = SkillRegistry::new([]);
        registry.load_plugin_skills(&roots).unwrap();
        registry
    }

    #[test]
    fn plugin_skills_are_namespaced_and_do_not_collide() {
        let registry = registry_with_plugins(&[("alpha", "review"), ("beta", "review")]);

        assert!(registry.skills().contains_key("alpha:review"));
        assert!(registry.skills().contains_key("beta:review"));
    }

    #[test]
    fn plugin_skills_only_load_direct_skill_children() {
        let dir = tempdir().unwrap();
        let skills_dir = dir.path().join("plugin/skills");
        write_skill(&skills_dir, "direct", "Direct plugin skill", "direct body");
        write_skill(
            &skills_dir.join("nested"),
            "hidden",
            "Nested plugin skill",
            "nested body",
        );
        let mut registry = SkillRegistry::new([]);

        registry
            .load_plugin_skills(&[PluginSkillRoot {
                plugin_id: "plugin".into(),
                skills_dir,
            }])
            .unwrap();

        assert!(registry.skills().contains_key("plugin:direct"));
        assert!(!registry.skills().contains_key("plugin:hidden"));
    }

    #[test]
    fn standalone_skill_keeps_its_unqualified_name() {
        let dir = tempdir().unwrap();
        let standalone_dir = dir.path().join("standalone");
        write_skill(
            &standalone_dir,
            "review",
            "Standalone skill",
            "standalone body",
        );

        let mut registry = SkillRegistry::new([standalone_dir]);
        registry.load_skills().unwrap();
        let plugin_skills_dir = dir.path().join("plugin/skills");
        write_skill(&plugin_skills_dir, "review", "Plugin skill", "plugin body");
        registry
            .load_plugin_skills(&[PluginSkillRoot {
                plugin_id: "alpha".to_owned(),
                skills_dir: plugin_skills_dir,
            }])
            .unwrap();

        assert!(registry.skills().contains_key("review"));
        assert!(registry.skills().contains_key("alpha:review"));
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
    fn loads_from_workdir_tact_skills_dir() {
        let dir = tempdir().unwrap();
        let tact_skills = dir.path().join(".tact/skills");
        write_skill(&tact_skills, "local", "Local skill", "local body");

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(registry.skills().contains_key("local"));
        assert!(registry.load_full_text("local").contains("local body"));
    }

    #[test]
    fn bare_workdir_skills_dir_is_not_scanned() {
        let dir = tempdir().unwrap();
        write_skill(
            &dir.path().join("skills"),
            "old",
            "Legacy skill",
            "legacy body",
        );

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(!registry.skills().contains_key("old"));
    }

    #[test]
    fn project_skill_overrides_tact_skills_same_name() {
        let dir = tempdir().unwrap();
        write_skill(&dir.path().join(".tact/skills"), "style", "tact", "TACT");
        write_skill(
            &dir.path().join(".claude/skills"),
            "style",
            "project",
            "PROJECT",
        );

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(registry.load_full_text("style").contains("PROJECT"));
        assert!(!registry.load_full_text("style").contains("TACT"));
    }

    #[test]
    fn resolve_skill_dir_joins_relative_to_workdir() {
        let workdir = PathBuf::from("/proj");
        assert_eq!(
            resolve_skill_dir("./vendor/skills", &workdir),
            PathBuf::from("/proj/vendor/skills")
        );
        assert_eq!(
            resolve_skill_dir("/abs/skills", &workdir),
            PathBuf::from("/abs/skills")
        );
    }

    #[test]
    fn resolve_skill_dir_expands_home_prefix() {
        let workdir = PathBuf::from("/proj");
        let home = std::env::var_os("HOME").expect("HOME");
        assert_eq!(
            resolve_skill_dir("~/shared-skills", &workdir),
            PathBuf::from(home).join("shared-skills")
        );
    }

    #[test]
    fn configured_extra_dir_overrides_earlier_roots() {
        let dir = tempdir().unwrap();
        write_skill(&dir.path().join(".tact/skills"), "shared", "base", "BASE");
        let extra = dir.path().join("extra-skills");
        write_skill(&extra, "shared", "extra", "EXTRA");

        let mut dirs = TactPath::new(dir.path()).skill_search_dirs();
        dirs.push(extra);
        let mut registry = SkillRegistry::new(dirs);
        registry.load_skills().unwrap();
        assert!(registry.load_full_text("shared").contains("EXTRA"));
        assert!(!registry.load_full_text("shared").contains("BASE"));
    }

    #[test]
    fn plugin_commands_load_as_namespaced_slash_commands() {
        let dir = tempdir().unwrap();
        let plugin_root = dir.path().join("plugin");
        fs::create_dir_all(plugin_root.join("commands")).unwrap();
        fs::write(
            plugin_root.join("commands/commit.md"),
            "---\ndescription: Commit helper\n---\n\nRun git commit.",
        )
        .unwrap();
        let mut registry = SkillRegistry::new([]);

        registry
            .load_plugin_commands(&plugin_root.join("commands"), "plugin")
            .unwrap();

        let doc = registry.skills().get("plugin:commit").expect("command loaded");
        assert_eq!(doc.manifest.description, "Commit helper");
        assert!(doc.body.contains("git commit"));
    }

    #[test]
    fn plugin_command_overrides_same_name_skill() {
        let dir = tempdir().unwrap();
        let plugin_root = dir.path().join("plugin");
        fs::create_dir_all(plugin_root.join("commands")).unwrap();
        fs::create_dir_all(plugin_root.join("skills/review")).unwrap();
        fs::write(
            plugin_root.join("commands/review.md"),
            "---\ndescription: Command review\n---\n\nCOMMAND BODY",
        )
        .unwrap();
        fs::write(
            plugin_root.join("skills/review/SKILL.md"),
            "---\nname: review\ndescription: Skill review\n---\n\nSKILL BODY",
        )
        .unwrap();
        let mut registry = SkillRegistry::new([]);

        // Load skills first, then commands — the command must win.
        registry
            .load_plugin_skills(&[PluginSkillRoot {
                plugin_id: "plugin".into(),
                skills_dir: plugin_root.join("skills"),
            }])
            .unwrap();
        registry
            .load_plugin_commands(&plugin_root.join("commands"), "plugin")
            .unwrap();

        let doc = registry.skills().get("plugin:review").expect("review present");
        assert!(
            doc.body.contains("COMMAND BODY"),
            "command must override the same-named skill"
        );
    }

    #[test]
    fn command_frontmatter_extends_manifest() {
        let dir = tempdir().unwrap();
        let commands_dir = dir.path().join("commands");
        fs::create_dir_all(&commands_dir).unwrap();
        fs::write(
            commands_dir.join("build.md"),
            "---\ndescription: Build\nargument-hint: <target>\nallowed-tools: Bash, Read\nmodel: sonnet\n---\n\nbody",
        )
        .unwrap();
        let mut registry = SkillRegistry::new([]);

        registry
            .load_plugin_commands(&commands_dir, "plugin")
            .unwrap();

        let doc = registry.skills().get("plugin:build").expect("loaded");
        assert_eq!(doc.manifest.argument_hint.as_deref(), Some("<target>"));
        assert_eq!(
            doc.manifest.allowed_tools.as_deref(),
            Some("Bash, Read")
        );
        assert_eq!(doc.manifest.model.as_deref(), Some("sonnet"));
    }

    #[test]
    fn plugin_commands_ignore_non_markdown_files() {
        let dir = tempdir().unwrap();
        let commands_dir = dir.path().join("commands");
        fs::create_dir_all(&commands_dir).unwrap();
        fs::write(commands_dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::write(commands_dir.join("note.md"), "---\ndescription: Note\n---\n").unwrap();
        let mut registry = SkillRegistry::new([]);

        registry
            .load_plugin_commands(&commands_dir, "plugin")
            .unwrap();

        assert!(registry.skills().contains_key("plugin:note"));
        assert!(!registry.skills().contains_key("plugin:run.sh"));
        assert!(!registry.skills().contains_key("plugin:run"));
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
