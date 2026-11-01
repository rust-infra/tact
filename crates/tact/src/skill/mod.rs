//! Skill (custom instruction) loading.
//!
//! Skills are markdown files (`SKILL.md`) nested in subdirectories under one or
//! more skill roots. Roots are scanned in ascending precedence — a later root
//! wins on a name clash:
//!
//! - global:  `~/.agents/skills/`   (Codex compatibility root)
//! - user:    `~/.tact/skills/`
//! - project: `<workdir>/.tact/skills/`
//! - config:  `[agent].skill_dirs`  (appended last, in listed order)
//!
//! Installed plugin roots load after all of these, into their own
//! `plugin:skill` namespace.
//!
//! Each file has optional YAML frontmatter for `name` and `description`
//! (Agent Skills–compatible). Bodies are unrestricted; TUI slash invoke may
//! additionally substitute Claude Code–style bare `$ARGUMENTS`.
//!
//! - [`SkillRegistry`] scans skill directories, parses frontmatter, and
//!   provides lookup by name.
//! - [`get_skill_registry`] / [`shared_skill_registry`] construct registries;
//!   interactive mode shares [`SharedSkillRegistry`] between agent tools and the TUI
//!   so `/skill reload` updates both without restart.
//! - [`SkillRegistry::describe_available_bounded`] supplies the system prompt's
//!   catalog — name/description lines, each description capped, and the list
//!   dropped to a context-window budget. [`SkillRegistry::describe_available`]
//!   is the uncapped form the TUI snapshots.
//! - Full bodies are wrapped in `<skill>` XML for `load_skill` tool results
//!   and for TUI slash invocation (user task). The wrapper carries the skill
//!   directory, so a skill that ships `references/` or `scripts/` can say so.
//! - `[agent].skill_disable` names skills to keep out of the catalog. They stay
//!   loadable by name: the switch hides from automatic discovery, it does not
//!   disable the capability.
//!
//! See `docs/superpowers/specs/2026-10-11-skill-catalog-budget-design.md` for
//! why the catalog is bounded and why the path rides the body rather than the
//! catalog.

use std::{
    collections::{HashMap, HashSet},
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

/// Metadata for one loaded skill.
///
/// Deliberately narrow. Claude Code's `argument-hint` / `allowed-tools` /
/// `model` frontmatter is **not** parsed: a field that is stored but never
/// enforced invites authors to rely on it, and nothing in Tact reads those
/// keys, so they are ignored rather than carried for a consumer that does not
/// exist. Re-adding one means adding its consumer in the same change.
pub struct SkillManifest {
    pub name: String,
    pub description: String,
}

pub struct SkillDocument {
    pub manifest: SkillManifest,
    pub body: String,
    /// Directory holding the `SKILL.md` this body came from.
    ///
    /// Carried on the document rather than the manifest on purpose: it is
    /// delivered inside the `<skill>` wrapper when the body is loaded, never in
    /// the catalog. A path in the catalog costs every request, and the model
    /// only needs it once it is acting on the skill.
    pub dir: PathBuf,
}

fn is_bare_arguments_placeholder(after: &str) -> bool {
    match after.chars().next() {
        None => true,
        Some('[') => false,
        Some(c) if c.is_ascii_alphanumeric() || c == '_' => false,
        Some(_) => true,
    }
}

/// Render a skill body for an explicit invocation.
///
/// Bare `$ARGUMENTS` is substituted once; indexed forms are preserved. When
/// the body has no placeholder, arguments are appended so the model still
/// receives them.
#[must_use]
pub fn render_skill_body(body: &str, args: &str) -> String {
    let body = body.trim();
    let mut out = String::with_capacity(body.len() + args.len());
    let mut rest = body;
    let mut substituted = false;
    while let Some(index) = rest.find("$ARGUMENTS") {
        out.push_str(&rest[..index]);
        let after = &rest[index + "$ARGUMENTS".len()..];
        if is_bare_arguments_placeholder(after) {
            out.push_str(args);
            substituted = true;
            rest = after;
        } else {
            out.push_str("$ARGUMENTS");
            rest = after;
        }
    }
    out.push_str(rest);
    if !substituted && !args.is_empty() {
        out.push_str("\n\nARGUMENTS: ");
        out.push_str(args);
    }
    out
}

fn escape_xml_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Build the agent-facing task text from a loaded skill's parts.
#[must_use]
pub fn format_skill_task(name: &str, dir: &std::path::Path, body: &str, args: &str) -> String {
    format!(
        "<skill name=\"{}\" dir=\"{}\">\n{}\n</skill>",
        escape_xml_attribute(name),
        escape_xml_attribute(&dir.display().to_string()),
        render_skill_body(body, args)
    )
}

/// Build the agent-facing task text for an explicitly invoked skill.
#[must_use]
pub fn format_skill_agent_task(skill: &SkillDocument, args: &str) -> String {
    format_skill_task(&skill.manifest.name, &skill.dir, &skill.body, args)
}

/// Default cap on a catalog description, in characters.
///
/// A catalog line is a routing hint; the whole description is what `load_skill`
/// returns. 200 holds a "what it does and when to use it" sentence pair, and
/// halves the catalog on a machine carrying 79 skills.
pub const DEFAULT_SKILL_DESCRIPTION_CAP: usize = 200;

/// Catalog ceiling used when `model_context_window` is unknown (`0`).
pub const CATALOG_BUDGET_FALLBACK_CHARS: usize = 8_000;

/// How many dropped skill names the over-budget trailer will spell out.
pub const TRAILER_NAME_CAP: usize = 40;

/// Catalog ceiling for a model context window, in characters.
///
/// Two percent of the window at four bytes per token — the same estimate the
/// `mcp get` cost line uses. `0` (window unknown, or auto-compaction off) falls
/// back to [`CATALOG_BUDGET_FALLBACK_CHARS`].
pub fn catalog_budget_chars(model_context_window: usize) -> usize {
    if model_context_window == 0 {
        return CATALOG_BUDGET_FALLBACK_CHARS;
    }
    // window / 100 * 8 == 2% of the window, in bytes at 4 bytes/token.
    model_context_window.saturating_div(100).saturating_mul(8)
}

impl std::fmt::Display for SkillDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            r#"<skill name="{}" dir="{}">
{}
</skill>"#,
            escape_xml_attr(&self.manifest.name),
            escape_xml_attr(&self.dir.display().to_string()),
            self.body
        )
    }
}

/// Escape a value for use inside a double-quoted XML attribute.
fn escape_xml_attr(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

/// Shared registry used by the agent tools and (in interactive mode) the TUI.
pub type SharedSkillRegistry = Arc<Mutex<SkillRegistry>>;

/// Build a registry for `workdir` by scanning built-in skill roots, then any
/// `[agent].skill_dirs` from installed process config (when present).
pub fn get_skill_registry(workdir: impl AsRef<Path>) -> Result<SkillRegistry> {
    let workdir = workdir.as_ref();
    let mut dirs = TactPath::new(workdir).skill_search_dirs();
    let mut disabled: Vec<String> = Vec::new();
    if let Some(cfg) = crate::config::try_settings() {
        disabled = cfg.agent.skill_disable.clone();
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
    registry.set_disabled(disabled);
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
    /// Registry names kept out of the catalog by `[agent].skill_disable`.
    ///
    /// Filtered at render time, never at load: `/skill list` still shows a
    /// disabled skill, and `load_skill` still resolves it.
    disabled: HashSet<String>,
}

impl SkillRegistry {
    pub fn new(skill_dirs: impl IntoIterator<Item = PathBuf>) -> Self {
        Self {
            skill_dirs: skill_dirs.into_iter().collect(),
            skills: HashMap::new(),
            disabled: HashSet::new(),
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
            // `Path::is_dir()` (stat) rather than `DirEntry::file_type()` (lstat),
            // so a symlinked skill directory is a skill like any other — the same
            // rule the standalone walk follows. Still one level deep: the flat,
            // direct-children-only contract is unchanged.
            if !entry.path().is_dir() {
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
            // Skill roots are assembled by symlinking directories in place:
            // `~/.agents/skills/omarchy -> /usr/share/omarchy/default/agents/skills/omarchy`.
            // walkdir does not follow links by default, and a symlinked *directory*
            // is not `is_file()`, so without this the skill is silently missing.
            .follow_links(true)
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
            },
            body,
            dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
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
                },
                body,
                dir: path.parent().map(Path::to_path_buf).unwrap_or_default(),
            };
            self.skills.insert(name, document);
        }

        Ok(())
    }

    /// List available skills with name + description (metadata only, uncapped).
    ///
    /// The TUI snapshots this into `skills_description`. The system prompt uses
    /// [`Self::describe_available_bounded`] instead, because this form has no
    /// ceiling: on a machine with 79 skills it measures 22.6 KB ≈ 5.6k tokens.
    pub fn describe_available(&self) -> String {
        let entries = self.catalog_entries();
        if entries.is_empty() {
            return "(no skills available)".to_string();
        }

        entries
            .into_iter()
            .map(|doc| format!("- {}: {}", doc.manifest.name, doc.manifest.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// The system prompt's catalog: descriptions capped, list dropped to fit.
    ///
    /// `description_cap` of `0` leaves descriptions whole. When the capped
    /// catalog still exceeds `budget_chars`, entries are dropped from the tail
    /// of the name-sorted list and replaced by one trailer line naming them —
    /// without the names, `load_skill` would be unusable for the dropped set.
    ///
    /// The tail is alphabetical because ranking by usefulness would need the
    /// root each skill came from, which [`SkillManifest`] does not carry. A
    /// deterministic, stated order beats a hidden heuristic.
    pub fn describe_available_bounded(
        &self,
        budget_chars: usize,
        description_cap: usize,
    ) -> String {
        let entries = self.catalog_entries();
        if entries.is_empty() {
            return "(no skills available)".to_string();
        }

        // Keep-loop: shrink the kept prefix until it plus the trailer fits.
        // Terminates at `kept == 0` even if the trailer alone overflows.
        let mut kept = entries.len();
        loop {
            let body = render_catalog(&entries[..kept], &entries[kept..], description_cap);
            if kept == 0 || body.chars().count() <= budget_chars {
                return body;
            }
            kept -= 1;
        }
    }

    /// List available skills with full body injected (Claude Code style).
    ///
    /// Not capped or budgeted: `skill_body_auto_inject` is an explicit opt-in
    /// to "everything, always", so trimming it would break its own promise.
    pub fn describe_available_with_body(&self) -> String {
        let entries = self.catalog_entries();
        if entries.is_empty() {
            return "(no skills available)".to_string();
        }

        entries
            .into_iter()
            .map(std::string::ToString::to_string)
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

    /// Replace the set of names kept out of the catalog.
    pub fn set_disabled(&mut self, names: impl IntoIterator<Item = String>) {
        self.disabled = names.into_iter().collect();
    }

    pub fn disabled(&self) -> &HashSet<String> {
        &self.disabled
    }

    /// Whether `name` is kept out of the catalog. Still loadable by name.
    pub fn is_disabled(&self, name: &str) -> bool {
        self.disabled.contains(name)
    }

    /// Name-sorted catalog entries, disabled skills removed.
    fn catalog_entries(&self) -> Vec<&SkillDocument> {
        let mut entries: Vec<&SkillDocument> = self
            .skills
            .iter()
            .filter(|(name, _)| !self.disabled.contains(*name))
            .map(|(_, doc)| doc)
            .collect();
        entries.sort_by(|a, b| a.manifest.name.cmp(&b.manifest.name));
        entries
    }
}

/// Render the kept entries plus the over-budget trailer for the dropped ones.
fn render_catalog(
    kept: &[&SkillDocument],
    dropped: &[&SkillDocument],
    description_cap: usize,
) -> String {
    let mut out = kept
        .iter()
        .map(|doc| {
            format!(
                "- {}: {}",
                doc.manifest.name,
                cap_description(&doc.manifest.description, description_cap)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    if let Some(trailer) = catalog_trailer(dropped) {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&trailer);
    }

    out
}

/// The one line that stands in for skills the budget could not afford.
///
/// Names are spelled out, up to [`TRAILER_NAME_CAP`], because a bare count
/// would leave `load_skill` unusable for the dropped set.
fn catalog_trailer(dropped: &[&SkillDocument]) -> Option<String> {
    if dropped.is_empty() {
        return None;
    }

    let total = dropped.len();
    let named = dropped
        .iter()
        .take(TRAILER_NAME_CAP)
        .map(|doc| doc.manifest.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");

    Some(if total > TRAILER_NAME_CAP {
        format!("- (+{total} omitted for the catalog budget: {named}, …)")
    } else {
        format!("- (+{total} omitted for the catalog budget: {named})")
    })
}

/// Truncate a description on a word boundary, appending `…`.
///
/// `cap` of `0` means no cap. The cut falls back to the last space unless the
/// tail after it is more than 40% of the cap, in which case the cap wins — a
/// description with one very long token must still be bounded.
fn cap_description(description: &str, cap: usize) -> String {
    if cap == 0 || description.chars().count() <= cap {
        return description.to_string();
    }

    let head: String = description.chars().take(cap).collect();
    let trimmed = head.trim_end();
    let cut = match trimmed.rfind(' ') {
        Some(idx) if trimmed[idx..].chars().count().saturating_mul(5) <= cap * 2 => {
            trimmed[..idx].trim_end()
        }
        _ => trimmed,
    };

    format!("{cut}…")
}

/// The only frontmatter keys Tact reads. Anything else in the block — Claude
/// Code's `argument-hint` / `allowed-tools` / `model` included — is ignored by
/// serde, which is the point: see [`SkillManifest`].
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

    /// A plugin skill whose directory is a symlink is still a direct child, so
    /// it loads — the plugin scan must not be stricter than the standalone walk.
    #[cfg(unix)]
    #[test]
    fn symlinked_plugin_skill_dir_is_loaded() {
        let dir = tempdir().unwrap();
        let real_root = dir.path().join("real-skills");
        write_skill(&real_root, "linked", "Linked plugin skill", "linked body");

        let skills_dir = dir.path().join("plugin/skills");
        fs::create_dir_all(&skills_dir).unwrap();
        std::os::unix::fs::symlink(real_root.join("linked"), skills_dir.join("linked")).unwrap();

        let mut registry = SkillRegistry::new([]);
        registry
            .load_plugin_skills(&[PluginSkillRoot {
                plugin_id: "plugin".into(),
                skills_dir,
            }])
            .unwrap();

        assert!(registry.skills().contains_key("plugin:linked"));
        assert!(
            registry
                .load_full_text("plugin:linked")
                .contains("linked body")
        );
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
    fn claude_project_skills_dir_is_not_scanned() {
        // Claude-directory compatibility was removed; `<workdir>/.claude/skills`
        // must not be picked up as a project skill root.
        let dir = tempdir().unwrap();
        write_skill(
            &dir.path().join(".claude/skills"),
            "deploy",
            "Deploy playbook",
            "step 1",
        );

        let registry = get_skill_registry(dir.path()).unwrap();
        assert!(
            !registry.skills().contains_key("deploy"),
            "legacy .claude/skills must not be scanned after claude-dir removal"
        );
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

    /// Skill roots are commonly populated with symlinks (`omarchy` ships its
    /// skills as `~/.agents/skills/omarchy -> /usr/share/omarchy/...`); a
    /// symlinked directory must be followed, not skipped.
    #[cfg(unix)]
    #[test]
    fn symlinked_skill_dir_is_loaded() {
        let dir = tempdir().unwrap();
        let real_root = dir.path().join("real-skills");
        write_skill(&real_root, "linked", "Linked skill", "linked body");

        let search_root = dir.path().join("skills-root");
        fs::create_dir_all(&search_root).unwrap();
        std::os::unix::fs::symlink(real_root.join("linked"), search_root.join("linked")).unwrap();

        let mut registry = SkillRegistry::new([search_root]);
        registry.load_skills().unwrap();

        assert!(registry.skills().contains_key("linked"));
        assert!(registry.load_full_text("linked").contains("linked body"));
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

        let doc = registry
            .skills()
            .get("plugin:commit")
            .expect("command loaded");
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

        let doc = registry
            .skills()
            .get("plugin:review")
            .expect("review present");
        assert!(
            doc.body.contains("COMMAND BODY"),
            "command must override the same-named skill"
        );
    }

    /// Claude Code's `argument-hint` / `allowed-tools` / `model` are ignored,
    /// not parsed: a key nobody enforces must not look supported. The keys it
    /// does read still parse from the same block.
    #[test]
    fn claude_only_frontmatter_keys_are_ignored() {
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
        assert_eq!(doc.manifest.name, "plugin:build");
        assert_eq!(doc.manifest.description, "Build");
    }

    #[test]
    fn plugin_commands_ignore_non_markdown_files() {
        let dir = tempdir().unwrap();
        let commands_dir = dir.path().join("commands");
        fs::create_dir_all(&commands_dir).unwrap();
        fs::write(commands_dir.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::write(
            commands_dir.join("note.md"),
            "---\ndescription: Note\n---\n",
        )
        .unwrap();
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
            &dir.path().join(".tact/skills"),
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

    // ---- catalog budget ----

    fn registry_with_skills(entries: &[(&str, &str)]) -> (tempfile::TempDir, SkillRegistry) {
        let dir = tempdir().unwrap();
        for (name, description) in entries {
            write_skill(dir.path(), name, description, "body");
        }
        let mut registry = SkillRegistry::new([dir.path().to_path_buf()]);
        registry.load_skills().unwrap();
        (dir, registry)
    }

    #[test]
    fn a_long_description_is_capped_on_a_word_boundary() {
        let (_dir, registry) =
            registry_with_skills(&[("long", "alpha beta gamma delta epsilon zeta eta theta")]);

        let catalog = registry.describe_available_bounded(10_000, 20);

        assert!(catalog.contains("- long: alpha beta gamma…"), "{catalog}");
    }

    #[test]
    fn a_cap_of_zero_leaves_descriptions_whole() {
        let (_dir, registry) =
            registry_with_skills(&[("long", "alpha beta gamma delta epsilon zeta eta theta")]);

        let catalog = registry.describe_available_bounded(10_000, 0);

        assert!(
            catalog.contains("- long: alpha beta gamma delta epsilon zeta eta theta"),
            "{catalog}"
        );
    }

    #[test]
    fn the_catalog_drops_the_alphabetical_tail_when_over_budget() {
        let description = "x".repeat(100);
        let (_dir, registry) = registry_with_skills(&[
            ("alpha", &description),
            ("bravo", &description),
            ("charlie", &description),
        ]);

        // Room for one entry plus the trailer, not two.
        let catalog = registry.describe_available_bounded(200, 0);

        assert!(catalog.contains("- alpha: "), "{catalog}");
        assert!(!catalog.contains("- bravo: "), "{catalog}");
        assert!(
            catalog.contains("- (+2 omitted for the catalog budget: bravo, charlie)"),
            "{catalog}"
        );
    }

    #[test]
    fn the_trailer_names_the_dropped_skills_and_stops_after_forty() {
        let entries: Vec<(String, &str)> = (0..45).map(|i| (format!("s{i:02}"), "d")).collect();
        let borrowed: Vec<(&str, &str)> = entries
            .iter()
            .map(|(name, description)| (name.as_str(), *description))
            .collect();
        let (_dir, registry) = registry_with_skills(&borrowed);

        // Far below any render that keeps an entry, so only the trailer survives.
        let catalog = registry.describe_available_bounded(100, 0);
        let trailer = catalog.lines().next().unwrap_or_default();

        assert!(
            trailer.starts_with("- (+45 omitted for the catalog budget: "),
            "{trailer}"
        );
        assert!(trailer.ends_with(", …)"), "{trailer}");
        let names = trailer
            .trim_start_matches("- (+45 omitted for the catalog budget: ")
            .trim_end_matches(", …)");
        assert_eq!(names.split(", ").count(), TRAILER_NAME_CAP, "{trailer}");
    }

    #[test]
    fn the_budget_falls_back_to_eight_thousand_chars_without_a_window() {
        assert_eq!(catalog_budget_chars(0), CATALOG_BUDGET_FALLBACK_CHARS);
        assert_eq!(catalog_budget_chars(1_000_000), 80_000);
        assert_eq!(catalog_budget_chars(200_000), 16_000);
    }

    /// The budget must not fire on a wide window: with 1M tokens the ceiling is
    /// 80,000 chars and a real 79-skill catalog is nowhere near it.
    #[test]
    fn a_catalog_that_fits_is_not_touched() {
        let description = "x".repeat(300);
        let (_dir, registry) =
            registry_with_skills(&[("alpha", &description), ("bravo", &description)]);

        assert_eq!(
            registry.describe_available_bounded(catalog_budget_chars(1_000_000), 0),
            registry.describe_available()
        );
    }

    /// `skill_body_auto_inject` is an explicit opt-in to "everything, always":
    /// neither the cap nor the budget may reach it.
    #[test]
    fn describe_available_with_body_is_not_capped() {
        let dir = tempdir().unwrap();
        let long_body = "y".repeat(400);
        write_skill(dir.path(), "alpha", "a description", &long_body);
        let mut registry = SkillRegistry::new([dir.path().to_path_buf()]);
        registry.load_skills().unwrap();

        let catalog = registry.describe_available_with_body();

        assert!(catalog.contains(&long_body), "the body must arrive whole");
        assert!(
            catalog.contains("<skill name=\"alpha\" dir=\""),
            "{catalog}"
        );
    }

    #[test]
    fn a_disabled_skill_leaves_the_catalog_but_still_loads() {
        let (_dir, mut registry) = registry_with_skills(&[("demo", "a description")]);

        registry.set_disabled(["demo".to_string()]);

        assert!(registry.is_disabled("demo"));
        assert_eq!(registry.describe_available(), "(no skills available)");
        assert_eq!(
            registry.describe_available_bounded(10_000, 0),
            "(no skills available)"
        );
        assert_eq!(
            registry.describe_available_with_body(),
            "(no skills available)"
        );
        assert!(
            registry.load_full_text("demo").contains("body"),
            "a disabled skill must stay loadable by name"
        );
    }

    #[test]
    fn the_skill_block_carries_the_skill_directory() {
        let (dir, registry) = registry_with_skills(&[("demo", "a description")]);

        let block = registry.load_full_text("demo");
        let expected = dir.path().join("demo");

        assert!(
            block.contains(&format!(r#"dir="{}""#, expected.display())),
            "{block}"
        );
        assert!(block.contains("<skill name=\"demo\""), "{block}");
    }
}
