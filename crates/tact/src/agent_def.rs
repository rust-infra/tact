//! Declarative subagent definitions.
//!
//! Claude Code plugins ship `agents/<name>.md` files that define reusable
//! subagents (system prompt, tool restrictions, model, permission mode).
//! This module loads those definitions — plus project-local
//! `<workdir>/.tact/agents/*.md` — into a shared registry that
//! [`spawn_subagent`](crate::tool::subagent::spawn_subagent) can reference by
//! name through the `agent` input field.
//!
//! Discovery order (later wins on name clash):
//!
//! - project-local: `<workdir>/.tact/agents/` (plain names)
//! - installed plugins: `<plugin-cache>/agents/` (`plugin:<name>`)

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::Result;
use serde::Deserialize;
use tracing::warn;

use crate::{
    consts::{PluginHome, TactPath},
    permission::PermissionMode,
    plugin::PluginStore,
};

/// A declarative subagent definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubagentDefinition {
    /// Full registry name (`plugin:<name>` for plugin agents, plain otherwise).
    pub name: String,
    pub description: String,
    /// The definition body — used as the subagent's system prompt.
    pub body: String,
    /// Optional allowed tool names (Claude Code naming); restricts the
    /// subagent toolset when set.
    pub tools: Option<Vec<String>>,
    /// Optional model override for the subagent.
    pub model: Option<String>,
    /// Optional permission-mode override for the subagent.
    pub permission_mode: Option<PermissionMode>,
    /// Source file path.
    pub path: PathBuf,
}

/// Shared registry used by `spawn_subagent` (and tests).
pub type SharedAgentDefinitionRegistry = Arc<Mutex<AgentDefinitionRegistry>>;

/// Build a registry for `workdir`: project-local `.tact/agents` first, then
/// installed plugin `agents/` directories (later roots win on name clash).
pub fn get_agent_definition_registry(workdir: impl AsRef<Path>) -> Result<AgentDefinitionRegistry> {
    let workdir = workdir.as_ref();
    let mut registry = AgentDefinitionRegistry::new();
    registry.load_dir(&TactPath::new(workdir).agents_dir(), None)?;
    if let Some(plugin_home) = PluginHome::from_environment() {
        let store = PluginStore::new(plugin_home);
        for root in store.installed_plugin_roots()? {
            registry.load_dir(&root.root.join("agents"), Some(&root.plugin_id))?;
        }
    }
    Ok(registry)
}

/// Build a mutex-backed registry shared across agent + tools.
pub fn shared_agent_definition_registry(
    workdir: impl AsRef<Path>,
) -> Result<SharedAgentDefinitionRegistry> {
    Ok(Arc::new(Mutex::new(get_agent_definition_registry(
        workdir,
    )?)))
}

/// Lock the shared agent-definition registry (recovers from poison).
pub fn lock_agent_definitions(
    reg: &SharedAgentDefinitionRegistry,
) -> std::sync::MutexGuard<'_, AgentDefinitionRegistry> {
    reg.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Default)]
pub struct AgentDefinitionRegistry {
    definitions: HashMap<String, SubagentDefinition>,
}

impl AgentDefinitionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Loads `*.md` files from one `agents` directory.
    ///
    /// `namespace` is `Some(plugin_id)` for plugin agents, making each entry
    /// `plugin:<name>`; `None` keeps the plain name (project-local agents).
    fn load_dir(&mut self, agents_dir: &Path, namespace: Option<&str>) -> Result<()> {
        if !agents_dir.is_dir() {
            return Ok(());
        }

        for entry in std::fs::read_dir(agents_dir)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    warn!("skipping agent definition dir entry: {error}");
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
            self.load_file(&path, stem, namespace);
        }

        Ok(())
    }

    fn load_file(&mut self, path: &Path, stem: &str, namespace: Option<&str>) {
        let content = match std::fs::read_to_string(path) {
            Ok(content) => content,
            Err(error) => {
                warn!("can't read agent definition {}: {error}", path.display());
                return;
            }
        };
        let (meta, body) = parse_frontmatter(&content);
        let local_name = meta.name.unwrap_or_else(|| stem.to_string());
        let name = namespace
            .map(|plugin_id| format!("{plugin_id}:{local_name}"))
            .unwrap_or(local_name);
        let tools = meta.tools.as_ref().map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(ToString::to_string)
                .collect()
        });
        let permission_mode = match meta.permission_mode_raw.as_deref() {
            Some("default") => Some(PermissionMode::Default),
            Some("plan") => Some(PermissionMode::Plan),
            Some("auto") => Some(PermissionMode::Auto),
            _ => None,
        };
        let definition = SubagentDefinition {
            name: name.clone(),
            description: meta
                .description
                .unwrap_or_else(|| "No description".to_string()),
            body,
            tools,
            permission_mode,
            model: meta.model,
            path: path.to_path_buf(),
        };
        self.definitions.insert(name, definition);
    }

    /// Looks up a definition by full name (already namespaced, e.g.
    /// `plugin:reviewer`) or by the local name when unambiguous.
    pub fn get(&self, name: &str) -> Option<&SubagentDefinition> {
        if let Some(definition) = self.definitions.get(name) {
            return Some(definition);
        }
        // Fall back to a unique local-name match so callers can pass
        // `reviewer` for a single `plugin:reviewer` without the prefix.
        let mut matches = self
            .definitions
            .values()
            .filter(|d| d.name.ends_with(&format!(":{name}")) || d.name == name);
        let first = matches.next()?;
        matches.next().is_none().then_some(first)
    }

    pub fn definitions(&self) -> &HashMap<String, SubagentDefinition> {
        &self.definitions
    }

    /// Lists `name: description` lines for error messages and help.
    pub fn describe_available(&self) -> String {
        if self.definitions.is_empty() {
            return "(no declarative agents available)".to_string();
        }
        let mut names = self.definitions.keys().cloned().collect::<Vec<_>>();
        names.sort();
        names
            .into_iter()
            .filter_map(|name| {
                self.definitions
                    .get(&name)
                    .map(|d| format!("- {}: {}", d.name, d.description))
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[derive(Debug, Default, Deserialize)]
struct AgentFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[serde(default)]
    tools: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "permissionMode")]
    permission_mode_raw: Option<String>,
}

fn parse_frontmatter(text: &str) -> (AgentFrontmatter, String) {
    let text = text.replace("\r\n", "\n");

    let Some(rest) = text.strip_prefix("---\n") else {
        return (AgentFrontmatter::default(), text.trim().to_string());
    };
    let Some((frontmatter, body)) = rest.split_once("\n---\n") else {
        return (AgentFrontmatter::default(), text.trim().to_string());
    };

    let meta = serde_yaml::from_str::<AgentFrontmatter>(frontmatter).unwrap_or_default();
    (meta, body.trim().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    fn write_agent(root: &Path, name: &str, body: &str) {
        fs::create_dir_all(root).unwrap();
        fs::write(root.join(format!("{name}.md")), body).unwrap();
    }

    #[test]
    fn loads_plugin_agents_namespaced() {
        let dir = tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        write_agent(
            &agents_dir,
            "reviewer",
            "---\nname: reviewer\ndescription: Reviews code\ntools: Read, Bash\n---\n\nReview body",
        );

        let mut registry = AgentDefinitionRegistry::new();
        registry.load_dir(&agents_dir, Some("plugin")).unwrap();

        let def = registry.get("plugin:reviewer").expect("loaded");
        assert_eq!(def.description, "Reviews code");
        assert!(def.body.contains("Review body"));
        assert_eq!(
            def.tools,
            Some(vec!["Read".to_string(), "Bash".to_string()])
        );
        assert_eq!(def.permission_mode, None);
    }

    #[test]
    fn project_agents_stay_plain_names() {
        let dir = tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        write_agent(
            &agents_dir,
            "architect",
            "---\nname: architect\ndescription: Architecture reviewer\npermissionMode: plan\n---\n\nbody",
        );

        let mut registry = AgentDefinitionRegistry::new();
        registry.load_dir(&agents_dir, None).unwrap();

        let def = registry.get("architect").expect("loaded");
        assert_eq!(def.permission_mode, Some(PermissionMode::Plan));
    }

    #[test]
    fn get_falls_back_to_unique_local_name() {
        let dir = tempdir().unwrap();
        let agents_dir = dir.path().join("agents");
        write_agent(&agents_dir, "reviewer", "---\ndescription: R\n---\n\nbody");
        let mut registry = AgentDefinitionRegistry::new();
        registry.load_dir(&agents_dir, Some("alpha")).unwrap();

        assert!(registry.get("alpha:reviewer").is_some());
        assert!(registry.get("reviewer").is_some());
    }

    #[test]
    fn get_ambiguous_local_name_returns_none() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a/agents");
        let b = dir.path().join("b/agents");
        write_agent(&a, "reviewer", "---\ndescription: R\n---\n\nbody");
        write_agent(&b, "reviewer", "---\ndescription: R\n---\n\nbody");
        let mut registry = AgentDefinitionRegistry::new();
        registry.load_dir(&a, Some("alpha")).unwrap();
        registry.load_dir(&b, Some("beta")).unwrap();

        assert!(registry.get("reviewer").is_none(), "ambiguous");
        assert!(registry.get("alpha:reviewer").is_some());
        assert!(registry.get("beta:reviewer").is_some());
    }

    #[test]
    fn later_root_overwrites_earlier() {
        let dir = tempdir().unwrap();
        let a = dir.path().join("a/agents");
        let b = dir.path().join("b/agents");
        write_agent(&a, "reviewer", "---\ndescription: A\n---\n\nA BODY");
        write_agent(&b, "reviewer", "---\ndescription: B\n---\n\nB BODY");
        let mut registry = AgentDefinitionRegistry::new();
        registry.load_dir(&a, Some("plugin")).unwrap();
        registry.load_dir(&b, Some("plugin")).unwrap();

        let def = registry.get("plugin:reviewer").unwrap();
        assert!(def.body.contains("B BODY"));
    }
}
