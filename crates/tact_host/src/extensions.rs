//! Process-level extension operations shared by desktop and other frontends.

use std::path::{Path, PathBuf};

use tact::{
    consts::PluginHome,
    plugin::{PluginInstaller, PluginRequest, PluginStore, execute_request},
    skill::{SharedSkillRegistry, get_skill_registry, lock_skills},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionSnapshot {
    pub skills: Vec<SkillSnapshot>,
    pub plugins: Vec<PluginSnapshot>,
    pub marketplaces: Vec<MarketplaceSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillSnapshot {
    pub name: String,
    pub description: String,
    pub enabled: bool,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSnapshot {
    pub name: String,
    pub marketplace: String,
    pub version: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceSnapshot {
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginAction {
    Reload,
    Install { spec: String },
    Uninstall { name: String },
    Update { name: String },
    MarketplaceAdd { source: String },
    MarketplaceUpdate { name: String },
    MarketplaceRemove { name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostExtensionError {
    Unavailable(String),
    OperationFailed(String),
}

#[derive(Clone)]
pub struct HostExtensionService {
    work_dir: PathBuf,
    skill_registry: SharedSkillRegistry,
    plugin_home: Option<PluginHome>,
}

impl HostExtensionService {
    pub fn new(
        work_dir: PathBuf,
        skill_registry: SharedSkillRegistry,
        plugin_home: Option<PluginHome>,
    ) -> Self {
        Self {
            work_dir,
            skill_registry,
            plugin_home,
        }
    }

    #[must_use]
    pub fn skill_registry(&self) -> SharedSkillRegistry {
        self.skill_registry.clone()
    }

    pub async fn snapshot(&self) -> Result<ExtensionSnapshot, HostExtensionError> {
        let skills = {
            let registry = lock_skills(&self.skill_registry);
            registry
                .skills()
                .values()
                .map(|document| SkillSnapshot {
                    name: document.manifest.name.clone(),
                    description: document.manifest.description.clone(),
                    enabled: !registry.is_disabled(&document.manifest.name),
                    path: document.dir.display().to_string(),
                })
                .collect()
        };
        let Some(home) = self.plugin_home.clone() else {
            return Ok(ExtensionSnapshot {
                skills,
                plugins: Vec::new(),
                marketplaces: Vec::new(),
            });
        };
        let (plugins, marketplaces) = tokio::task::spawn_blocking(move || {
            let plugins = PluginInstaller::new(home.clone())
                .list()
                .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))?
                .into_iter()
                .map(|plugin| PluginSnapshot {
                    name: plugin.id,
                    marketplace: plugin.marketplace,
                    version: (!plugin.revision.is_empty()).then_some(plugin.revision),
                    enabled: true,
                })
                .collect();
            let marketplaces = PluginStore::new(home)
                .load_marketplaces()
                .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))?
                .iter()
                .map(|(_, record)| MarketplaceSnapshot {
                    name: record.name.clone(),
                    source: record.source.display_source(),
                })
                .collect();
            Ok::<_, HostExtensionError>((plugins, marketplaces))
        })
        .await
        .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))??;
        Ok(ExtensionSnapshot {
            skills,
            plugins,
            marketplaces,
        })
    }

    pub async fn reload_skills(&self) -> Result<ExtensionSnapshot, HostExtensionError> {
        let fresh = get_skill_registry(&self.work_dir)
            .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))?;
        *lock_skills(&self.skill_registry) = fresh;
        self.snapshot().await
    }

    pub async fn set_skill_enabled(
        &self,
        name: &str,
        enabled: bool,
    ) -> Result<ExtensionSnapshot, HostExtensionError> {
        let next_disabled = {
            let mut registry = lock_skills(&self.skill_registry);
            if !registry.skills().contains_key(name) {
                return Err(HostExtensionError::OperationFailed(format!(
                    "unknown skill: {name}"
                )));
            }
            let mut disabled: Vec<String> = registry.disabled().iter().cloned().collect();
            if enabled {
                disabled.retain(|candidate| candidate != name);
            } else if !disabled.iter().any(|candidate| candidate == name) {
                disabled.push(name.to_string());
            }
            disabled.sort();
            registry.set_disabled(disabled.clone());
            disabled
        };
        tact::config::persist_skill_disable(&next_disabled)
            .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))?;
        self.snapshot().await
    }

    pub async fn plugin_action(
        &self,
        action: PluginAction,
    ) -> Result<ExtensionSnapshot, HostExtensionError> {
        let Some(home) = self.plugin_home.clone() else {
            return Err(HostExtensionError::Unavailable(
                "plugin home is unavailable".to_string(),
            ));
        };
        let refresh_skills = matches!(
            action,
            PluginAction::Reload
                | PluginAction::Install { .. }
                | PluginAction::Uninstall { .. }
                | PluginAction::Update { .. }
        );
        let request = match action {
            PluginAction::Reload => PluginRequest::Reload,
            PluginAction::Install { spec } => {
                let (plugin, marketplace) = spec
                    .split_once('@')
                    .map(|(plugin, marketplace)| (plugin.to_string(), marketplace.to_string()))
                    .unwrap_or((spec, String::new()));
                PluginRequest::Install {
                    plugin,
                    marketplace,
                }
            }
            PluginAction::Uninstall { name } => PluginRequest::Uninstall { plugin: name },
            PluginAction::Update { name } => PluginRequest::Update { plugin: name },
            PluginAction::MarketplaceAdd { source } => PluginRequest::MarketplaceAdd { source },
            PluginAction::MarketplaceUpdate { name } => PluginRequest::MarketplaceUpdate { name },
            PluginAction::MarketplaceRemove { name } => PluginRequest::MarketplaceRemove { name },
        };
        tokio::task::spawn_blocking(move || execute_request(home, request))
            .await
            .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))?
            .map_err(|error| HostExtensionError::OperationFailed(error.to_string()))?;
        if refresh_skills {
            self.reload_skills().await
        } else {
            self.snapshot().await
        }
    }
}

pub fn default_plugin_home() -> Option<PluginHome> {
    PluginHome::from_environment()
}

pub fn plugin_home_for(root: impl AsRef<Path>) -> PluginHome {
    PluginHome::from_home(root.as_ref())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_skill(root: &Path, name: &str, description: &str) {
        let dir = root.join(".tact/skills").join(name);
        std::fs::create_dir_all(&dir).expect("skill dir");
        std::fs::write(
            dir.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: {description}\n---\n\nbody"),
        )
        .expect("skill file");
    }

    async fn service(root: &Path) -> HostExtensionService {
        write_skill(root, "demo", "Demo skill");
        let registry = tact::skill::shared_skill_registry(root).expect("skill registry");
        HostExtensionService::new(root.to_path_buf(), registry, Some(plugin_home_for(root)))
    }

    #[tokio::test]
    async fn extension_snapshot_reports_skills_plugins_and_marketplaces() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let snapshot = service(tmp.path())
            .await
            .snapshot()
            .await
            .expect("snapshot");

        assert!(
            snapshot
                .skills
                .iter()
                .any(|skill| skill.name == "demo" && skill.enabled)
        );
        assert!(snapshot.plugins.is_empty());
        assert!(
            snapshot.marketplaces.len() >= 2,
            "built-in marketplaces must be reported"
        );
    }

    #[tokio::test]
    async fn plugin_action_returns_the_new_extension_snapshot() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let service = service(tmp.path()).await;
        let snapshot = service
            .plugin_action(PluginAction::Reload)
            .await
            .expect("reload");
        assert!(snapshot.skills.iter().any(|skill| skill.name == "demo"));
    }
}
