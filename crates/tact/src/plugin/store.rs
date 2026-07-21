use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use super::{InstalledState, MarketplaceState};
use crate::consts::PluginHome;

const MARKETPLACES_FILE: &str = "marketplaces.json";
const INSTALLED_FILE: &str = "installed.json";

/// The skill directory supplied by one installed plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginSkillRoot {
    pub plugin_id: String,
    pub skills_dir: PathBuf,
}

/// Persistent storage for plugin marketplace and installation state.
#[derive(Clone, Debug)]
pub struct PluginStore {
    home: PluginHome,
}

impl PluginStore {
    /// Creates a store rooted at an already-resolved plugin home.
    #[must_use]
    pub fn new(home: PluginHome) -> Self {
        Self { home }
    }

    /// Creates a store rooted in the given user's home directory.
    #[must_use]
    pub fn from_home(home: &Path) -> Self {
        Self::new(PluginHome::from_home(home))
    }

    /// Loads marketplace state and restores the built-in official marketplace.
    pub fn load_marketplaces(&self) -> Result<MarketplaceState> {
        let path = self.home.root.join(MARKETPLACES_FILE);
        let mut state = if path.exists() { read_json(&path)? } else { MarketplaceState::with_builtin() };
        state.ensure_builtin();
        Ok(state)
    }

    /// Atomically persists marketplace state.
    pub fn save_marketplaces(&self, state: &MarketplaceState) -> Result<()> {
        let mut state = state.clone();
        state.ensure_builtin();
        state.validate()?;
        write_json_atomically(&self.home.root.join(MARKETPLACES_FILE), &state)
    }

    /// Loads the installed-plugin state, returning an empty state when absent.
    pub fn load_installed(&self) -> Result<InstalledState> {
        let path = self.home.root.join(INSTALLED_FILE);
        if !path.exists() {
            return Ok(InstalledState::default());
        }
        read_json(&path)
    }

    /// Returns skill roots for installed plugins whose cached content remains valid.
    pub fn installed_skill_roots(&self) -> Result<Vec<PluginSkillRoot>> {
        let cache = match self.home.cache.canonicalize() {
            Ok(cache) => cache,
            Err(_) => return Ok(Vec::new()),
        };

        Ok(self
            .load_installed()?
            .plugins
            .into_values()
            .filter_map(|plugin| {
                let plugin_root = plugin.cache_path.canonicalize().ok()?;
                if !plugin_root.starts_with(&cache) {
                    return None;
                }

                let skills_dir = plugin_root.join("skills");
                skills_dir.is_dir().then_some(PluginSkillRoot { plugin_id: plugin.id, skills_dir })
            })
            .collect())
    }

    /// Commits installed state only after the staged candidate directory is valid.
    pub fn commit_install(&self, state: &InstalledState, candidate: &Path) -> Result<()> {
        if !candidate.is_dir() {
            bail!("plugin install candidate is not a directory: {}", candidate.display());
        }
        write_json_atomically(&self.home.root.join(INSTALLED_FILE), state)
    }
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let content =
        fs::read_to_string(path).with_context(|| format!("failed to read plugin state {}", path.display()))?;
    serde_json::from_str(&content).with_context(|| format!("failed to parse plugin state {}", path.display()))
}

fn write_json_atomically<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path.parent().context("plugin state path has no parent")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create plugin state directory {}", parent.display()))?;

    let temporary = temporary_sibling(path)?;
    let write_result = (|| -> Result<()> {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .with_context(|| format!("failed to create temporary plugin state {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, value)?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to finish temporary plugin state {}", temporary.display()))?;
        file.sync_all().with_context(|| format!("failed to sync temporary plugin state {}", temporary.display()))?;
        fs::rename(&temporary, path).with_context(|| format!("failed to replace plugin state {}", path.display()))?;
        Ok(())
    })();

    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

fn temporary_sibling(path: &Path) -> Result<PathBuf> {
    let file_name = path.file_name().context("plugin state path has no file name")?;
    Ok(path.with_file_name(format!(".{}.tmp", file_name.to_string_lossy())))
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use tempfile::tempdir;

    use super::{PluginSkillRoot, PluginStore};
    use crate::plugin::{InstalledPlugin, InstalledState, MarketplaceSource, OFFICIAL_MARKETPLACE};

    const OFFICIAL_MARKETPLACE_URL: &str = "https://github.com/anthropics/claude-plugins-official.git";

    #[test]
    fn load_marketplaces_restores_the_builtin_marketplace() {
        let home = tempdir().unwrap();
        let store = PluginStore::from_home(home.path());
        fs::create_dir_all(home.path().join(".tact/plugins")).unwrap();
        fs::write(home.path().join(".tact/plugins/marketplaces.json"), r#"{ "marketplaces": {} }"#).unwrap();

        assert!(store.load_marketplaces().unwrap().get(OFFICIAL_MARKETPLACE).is_some());
    }

    #[test]
    fn official_marketplace_source_is_canonicalized_when_loading_and_saving() {
        let home = tempdir().unwrap();
        let store = PluginStore::from_home(home.path());
        let attacker_source = MarketplaceSource::GitUrl("https://attacker.invalid/plugins.git".into());
        fs::create_dir_all(home.path().join(".tact/plugins")).unwrap();
        fs::write(
            home.path().join(".tact/plugins/marketplaces.json"),
            r#"{
                "marketplaces": {
                    "claude-plugins-official": {
                        "name": "claude-plugins-official",
                        "source": { "GitUrl": "https://attacker.invalid/plugins.git" }
                    }
                }
            }"#,
        )
        .unwrap();
        let loaded = store.load_marketplaces().unwrap();

        assert_eq!(loaded.get(OFFICIAL_MARKETPLACE).unwrap().source.git_url(), OFFICIAL_MARKETPLACE_URL);
        assert_ne!(loaded.get(OFFICIAL_MARKETPLACE).unwrap().source, attacker_source);
    }

    #[test]
    fn commit_install_does_not_persist_when_candidate_is_invalid() {
        let home = tempdir().unwrap();
        let store = PluginStore::from_home(home.path());
        let candidate = home.path().join("missing-candidate");

        assert!(store.commit_install(&InstalledState::default(), &candidate).is_err());
        assert!(!home.path().join(".tact/plugins/installed.json").exists());
    }

    #[test]
    fn commit_install_persists_state_after_validating_candidate() {
        let home = tempdir().unwrap();
        let candidate = home.path().join("candidate");
        fs::create_dir(&candidate).unwrap();
        let store = PluginStore::from_home(home.path());
        let state = InstalledState {
            plugins: BTreeMap::from([(
                "example".to_owned(),
                InstalledPlugin {
                    id: "example".to_owned(),
                    marketplace: "acme".to_owned(),
                    revision: "0123456789abcdef0123456789abcdef01234567".to_owned(),
                    cache_path: candidate.clone(),
                    skill_count: 1,
                },
            )]),
        };

        store.commit_install(&state, &candidate).unwrap();

        assert_eq!(store.load_installed().unwrap(), state);
        assert!(!home.path().join(".tact/plugins/.installed.json.tmp").exists());
    }

    #[test]
    fn load_installed_migrates_task_one_records() {
        let home = tempdir().unwrap();
        let store = PluginStore::from_home(home.path());
        fs::create_dir_all(home.path().join(".tact/plugins")).unwrap();
        fs::write(
            home.path().join(".tact/plugins/installed.json"),
            r#"{
                "plugins": {
                    "acme/example": {
                        "id": "example",
                        "marketplace": "acme"
                    }
                }
            }"#,
        )
        .unwrap();

        let installed = &store.load_installed().unwrap().plugins["acme/example"];

        assert_eq!(installed.id, "example");
        assert_eq!(installed.marketplace, "acme");
        assert!(installed.revision.is_empty());
        assert!(installed.cache_path.as_os_str().is_empty());
        assert_eq!(installed.skill_count, 0);
    }

    #[test]
    fn installed_skill_roots_return_each_valid_plugin_skills_directory() {
        let home = tempdir().unwrap();
        let plugin_root = home.path().join(".tact/plugins/cache/acme/review/abc123");
        fs::create_dir_all(plugin_root.join("skills/review")).unwrap();
        fs::write(plugin_root.join("skills/review/SKILL.md"), "review").unwrap();
        let store = PluginStore::from_home(home.path());
        let state = InstalledState {
            plugins: BTreeMap::from([(
                "acme/review".to_owned(),
                InstalledPlugin {
                    id: "review".to_owned(),
                    marketplace: "acme".to_owned(),
                    revision: "abc123".to_owned(),
                    cache_path: plugin_root.clone(),
                    skill_count: 1,
                },
            )]),
        };
        store.commit_install(&state, &plugin_root).unwrap();

        assert_eq!(
            store.installed_skill_roots().unwrap(),
            vec![PluginSkillRoot {
                plugin_id: "review".to_owned(),
                skills_dir: plugin_root.canonicalize().unwrap().join("skills"),
            }]
        );
    }
}
