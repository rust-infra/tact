use std::{
    collections::BTreeMap,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The official Claude marketplace that is available in every marketplace state.
pub const OFFICIAL_MARKETPLACE: &str = "claude-plugins-official";
/// The official OpenAI Codex marketplace, seeded into every state.
///
/// The name matches the catalog declared by `github.com/openai/plugins`
/// (`.agents/plugins/marketplace.json`), so a later
/// `plugin marketplace add openai/plugins` resolves to this same entry instead
/// of creating a duplicate.
pub const OPENAI_MARKETPLACE: &str = "openai-curated";
pub(crate) const MARKETPLACE_BACKUPS_DIRECTORY: &str = ".backups";
const OFFICIAL_MARKETPLACE_URL: &str = "https://github.com/anthropics/claude-plugins-official.git";
const OPENAI_MARKETPLACE_URL: &str = "https://github.com/openai/plugins.git";

/// The marketplace records seeded into every registry and restored on load.
///
/// These are protected: user commands cannot replace or remove them.
const BUILTIN_MARKETPLACES: &[(&str, &str)] = &[
    (OFFICIAL_MARKETPLACE, OFFICIAL_MARKETPLACE_URL),
    (OPENAI_MARKETPLACE, OPENAI_MARKETPLACE_URL),
];

fn is_builtin_marketplace(name: &str) -> bool {
    BUILTIN_MARKETPLACES
        .iter()
        .any(|(builtin, _)| *builtin == name)
}

fn builtin_record(name: &str, url: &str) -> MarketplaceRecord {
    MarketplaceRecord {
        name: name.to_owned(),
        source: MarketplaceSource::GitUrl(url.to_owned()),
    }
}

/// A marketplace location, either a Git repository or a catalog document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MarketplaceSource {
    GitUrl(String),
    CatalogUrl(String),
    /// A Codex-style local marketplace root.
    ///
    /// The catalog lives at `<root>/.agents/plugins/marketplace.json`; local
    /// plugin source paths in that catalog are resolved relative to `<root>`.
    LocalPath(std::path::PathBuf),
}

impl MarketplaceSource {
    /// Parses a GitHub `owner/repository` shorthand or a complete source URL.
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.is_empty() {
            bail!("marketplace source cannot be empty");
        }

        if value.contains("://") {
            let url = reqwest::Url::parse(value)
                .with_context(|| format!("invalid marketplace source: {value}"))?;
            if !matches!(url.scheme(), "git" | "http" | "https" | "ssh") || url.host().is_none() {
                bail!("unsupported marketplace source: {value}");
            }
            if matches!(url.scheme(), "http" | "https") && url.path().ends_with(".json") {
                return Ok(Self::CatalogUrl(value.to_owned()));
            }
            return Ok(Self::GitUrl(value.to_owned()));
        }

        if let Some(ssh_target) = value.strip_prefix("git@") {
            let Some((host, path)) = ssh_target.split_once(':') else {
                bail!("invalid marketplace source: {value}");
            };
            if host.is_empty() || path.is_empty() || path.contains('?') || path.contains('#') {
                bail!("invalid marketplace source: {value}");
            }
            return Ok(Self::GitUrl(value.to_owned()));
        }

        let mut components = value.split('/');
        let Some(owner) = components.next() else {
            bail!("invalid marketplace source: {value}");
        };
        let Some(repository) = components.next() else {
            bail!("invalid marketplace source: {value}");
        };
        if owner.is_empty() || repository.is_empty() || components.next().is_some() {
            bail!("invalid marketplace source: {value}");
        }

        Ok(Self::GitUrl(format!("https://github.com/{value}.git")))
    }

    /// Returns the source URL/path shown to users.
    ///
    /// A Codex-style local marketplace has no git URL. Its source path is the
    /// resolution root, not the catalog location, so pointing at it alone reads
    /// as "the wrong directory"; the catalog is appended to disambiguate.
    #[must_use]
    pub fn display_source(&self) -> String {
        match self {
            Self::GitUrl(url) | Self::CatalogUrl(url) => url.clone(),
            Self::LocalPath(path) => format!(
                "{} (catalog: {})",
                path.display(),
                marketplace_catalog_path(path).display()
            ),
        }
    }

    /// Backward-compatible name for display source; local paths are included.
    #[must_use]
    pub fn git_url(&self) -> String {
        self.display_source()
    }
}

/// A named marketplace source persisted in the marketplace registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarketplaceRecord {
    pub name: String,
    pub source: MarketplaceSource,
}

/// The catalog document of a Codex-style local marketplace root.
///
/// Mirrors the candidate order used when reading a local marketplace: the
/// personal `.agents/plugins/marketplace.json` first, then a repo-local
/// `.codex-plugin/marketplace.json`, then a bare `marketplace.json`.
#[must_use]
pub fn marketplace_catalog_path(root: &Path) -> PathBuf {
    const FILE: &str = "marketplace.json";
    let agents = root.join(".agents").join("plugins").join(FILE);
    if agents.exists() {
        return agents;
    }
    let manifest = root.join(".codex-plugin").join(FILE);
    if manifest.exists() {
        manifest
    } else {
        root.join(FILE)
    }
}

/// The persisted marketplace registry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MarketplaceState {
    marketplaces: BTreeMap<String, MarketplaceRecord>,
    #[serde(skip)]
    discovered: BTreeMap<String, MarketplaceRecord>,
}

impl MarketplaceState {
    /// Creates the marketplace registry with the built-in marketplaces.
    #[must_use]
    pub fn with_builtin() -> Self {
        let mut marketplaces = BTreeMap::new();
        for (name, url) in BUILTIN_MARKETPLACES {
            marketplaces.insert((*name).to_owned(), builtin_record(name, url));
        }
        Self {
            marketplaces,
            discovered: BTreeMap::new(),
        }
    }

    /// Adds a user marketplace unless it would replace a built-in source.
    pub fn add(&mut self, name: &str, source: MarketplaceSource) -> Result<()> {
        validate_marketplace_name(name)?;
        if is_builtin_marketplace(name) {
            bail!("the built-in marketplace cannot be replaced");
        }

        if let Some(existing) = self.marketplaces.get(name) {
            if existing.source == source {
                return Ok(());
            }
            bail!("marketplace {name} already exists with a different source");
        }

        self.marketplaces.insert(
            name.to_owned(),
            MarketplaceRecord {
                name: name.to_owned(),
                source,
            },
        );
        Ok(())
    }

    /// Adds a discovered (non-persisted) Codex marketplace unless the name is
    /// already present. Discovered marketplaces are re-scanned on every load,
    /// so deleting `~/.agents/plugins/marketplace.json` removes them again.
    pub(crate) fn merge_discovered<I>(&mut self, records: I)
    where
        I: IntoIterator<Item = MarketplaceRecord>,
    {
        for record in records {
            if self.marketplaces.contains_key(&record.name)
                || self.discovered.contains_key(&record.name)
                || validate_marketplace_name(&record.name).is_err()
            {
                continue;
            }
            self.discovered.insert(record.name.clone(), record);
        }
    }

    /// Removes a user-added marketplace from the registry.
    ///
    /// Discovered Codex marketplaces are not persisted, so they cannot be
    /// removed here; a plain "unknown marketplace" would be misleading.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        validate_marketplace_name(name)?;
        if is_builtin_marketplace(name) {
            bail!("the built-in marketplace cannot be removed");
        }
        if self.marketplaces.remove(name).is_none() {
            if self.discovered.contains_key(name) {
                bail!(
                    "marketplace {name} is discovered from a Codex marketplace file and cannot be removed; \
                     delete its marketplace.json instead"
                );
            }
            bail!("unknown marketplace {name}");
        }
        Ok(())
    }

    /// Returns the marketplace record for a given name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&MarketplaceRecord> {
        self.marketplaces
            .get(name)
            .or_else(|| self.discovered.get(name))
    }

    /// Iterates over marketplace names and their records.
    ///
    /// Discovered Codex marketplaces come first so a bare `plugin install`
    /// prefers the user's local Codex catalog over the legacy built-in source.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &MarketplaceRecord)> {
        self.discovered
            .iter()
            .map(|(name, record)| (name.as_str(), record))
            .chain(
                self.marketplaces
                    .iter()
                    .map(|(name, record)| (name.as_str(), record)),
            )
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, record) in &self.marketplaces {
            validate_marketplace_name(name)?;
            if record.name != *name {
                bail!("marketplace record name does not match its registry key: {name}");
            }
        }
        Ok(())
    }

    /// Restores the canonical built-in marketplace entries.
    pub(crate) fn ensure_builtin(&mut self) {
        for (name, url) in BUILTIN_MARKETPLACES {
            self.marketplaces
                .insert((*name).to_owned(), builtin_record(name, url));
        }
    }
}

pub(crate) fn validate_marketplace_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name == MARKETPLACE_BACKUPS_DIRECTORY
        || name.contains(['/', '\\'])
        || Path::new(name)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        bail!("invalid marketplace name: {name}");
    }
    Ok(())
}

impl<'de> Deserialize<'de> for MarketplaceState {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RawMarketplaceState {
            marketplaces: BTreeMap<String, MarketplaceRecord>,
        }

        let raw = RawMarketplaceState::deserialize(deserializer)?;
        let mut state = Self {
            marketplaces: raw.marketplaces,
            discovered: BTreeMap::new(),
        };
        state.validate().map_err(serde::de::Error::custom)?;
        state.ensure_builtin();
        Ok(state)
    }
}

/// A plugin installed from a marketplace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub id: String,
    pub marketplace: String,
    #[serde(default)]
    pub revision: String,
    #[serde(default, alias = "path")]
    pub cache_path: std::path::PathBuf,
    #[serde(default)]
    pub skill_count: usize,
    /// Number of `commands/*.md` slash commands shipped by the plugin.
    #[serde(default)]
    pub command_count: usize,
    /// Whether the plugin declares lifecycle hooks (plugin.json `hooks`).
    #[serde(default)]
    pub has_hooks: bool,
    /// Whether the plugin ships MCP servers (`.mcp.json` or `mcpServers`).
    #[serde(default)]
    pub has_mcp: bool,
}

/// The feature surface of one installed plugin, computed at install time.
///
/// At least one field must be non-empty for a plugin to be installable; this
/// replaces the old hard requirement for a `skills/` directory so command-only,
/// hook-only, and MCP-only marketplace plugins can be installed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginFeatures {
    pub skill_count: usize,
    pub command_count: usize,
    pub has_hooks: bool,
    pub has_mcp: bool,
}

impl PluginFeatures {
    /// True when the plugin contributes no supported feature at all.
    #[must_use]
    pub fn is_empty(self) -> bool {
        self.skill_count == 0 && self.command_count == 0 && !self.has_hooks && !self.has_mcp
    }
}

impl From<&InstalledPlugin> for PluginFeatures {
    fn from(plugin: &InstalledPlugin) -> Self {
        Self {
            skill_count: plugin.skill_count,
            command_count: plugin.command_count,
            has_hooks: plugin.has_hooks,
            has_mcp: plugin.has_mcp,
        }
    }
}

/// The persisted installed-plugin registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledState {
    pub plugins: BTreeMap<String, InstalledPlugin>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{MarketplaceSource, MarketplaceState, OFFICIAL_MARKETPLACE, OPENAI_MARKETPLACE};
    use crate::consts::PluginHome;

    #[test]
    fn plugin_home_uses_the_tact_plugin_layout() {
        let home = PluginHome::from_home(Path::new("/users/example"));

        assert_eq!(home.root, Path::new("/users/example/.tact/plugins"));
        assert_eq!(
            home.marketplaces,
            Path::new("/users/example/.tact/plugins/marketplaces")
        );
        assert_eq!(home.cache, Path::new("/users/example/.tact/plugins/cache"));
    }

    #[test]
    fn github_shorthand_normalizes_to_git_url() {
        assert_eq!(
            MarketplaceSource::parse("acme/plugins").unwrap().git_url(),
            "https://github.com/acme/plugins.git"
        );
    }

    #[test]
    fn local_marketplace_display_names_the_catalog_next_to_its_root() {
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join(".agents/plugins")).unwrap();
        std::fs::write(
            home.path().join(".agents/plugins/marketplace.json"),
            r#"{"name":"codex-test-market","plugins":[]}"#,
        )
        .unwrap();

        assert_eq!(
            MarketplaceSource::LocalPath(home.path().to_path_buf()).display_source(),
            format!(
                "{} (catalog: {})",
                home.path().display(),
                home.path()
                    .join(".agents/plugins/marketplace.json")
                    .display()
            )
        );
    }

    #[test]
    fn marketplace_catalog_path_prefers_the_personal_agents_catalog() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".agents/plugins")).unwrap();
        std::fs::write(root.path().join(".agents/plugins/marketplace.json"), "{}").unwrap();
        std::fs::write(root.path().join("marketplace.json"), "{}").unwrap();

        assert_eq!(
            super::marketplace_catalog_path(root.path()),
            root.path().join(".agents/plugins/marketplace.json")
        );
    }

    #[test]
    fn marketplace_catalog_path_falls_back_to_a_bare_catalog() {
        let root = tempfile::tempdir().unwrap();

        assert_eq!(
            super::marketplace_catalog_path(root.path()),
            root.path().join("marketplace.json")
        );
    }

    #[test]
    fn discovered_marketplace_removal_explains_that_it_is_not_persisted() {
        let mut state = MarketplaceState::with_builtin();
        state.merge_discovered(vec![super::MarketplaceRecord {
            name: "codex-marketplace-global".to_owned(),
            source: MarketplaceSource::LocalPath(Path::new("/home/example").to_path_buf()),
        }]);

        let error = state.remove("codex-marketplace-global").unwrap_err();

        assert!(
            error.to_string().contains("delete its marketplace.json"),
            "{error}"
        );
    }

    #[test]
    fn catalog_url_with_query_string_is_classified_by_url_path() {
        assert_eq!(
            MarketplaceSource::parse("https://example.invalid/catalog.json?version=2").unwrap(),
            MarketplaceSource::CatalogUrl("https://example.invalid/catalog.json?version=2".into())
        );
    }

    #[test]
    fn git_uri_is_classified_as_a_git_source() {
        assert_eq!(
            MarketplaceSource::parse("git://example.invalid/marketplace.git").unwrap(),
            MarketplaceSource::GitUrl("git://example.invalid/marketplace.git".into())
        );
    }

    #[test]
    fn rejects_unsupported_marketplace_uris() {
        for source in [
            "file:///tmp/marketplace.json",
            "mailto:marketplace@example.invalid",
            "git:///marketplace.git",
        ] {
            assert!(MarketplaceSource::parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_marketplace_names_with_path_components_before_persistence() {
        let mut state = MarketplaceState::with_builtin();
        for name in ["../outside", "nested/name", "nested\\name", ".", ""] {
            assert!(
                state
                    .add(
                        name,
                        MarketplaceSource::GitUrl("https://example.invalid/a.git".into())
                    )
                    .is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn public_api_cannot_replace_a_builtin_marketplace() {
        let mut state = MarketplaceState::with_builtin();
        for name in [OFFICIAL_MARKETPLACE, OPENAI_MARKETPLACE] {
            assert!(
                state
                    .add(name, MarketplaceSource::GitUrl("https://x/y.git".into()))
                    .is_err(),
                "{name}"
            );
        }
        assert_eq!(
            state.get(OFFICIAL_MARKETPLACE).unwrap().source.git_url(),
            "https://github.com/anthropics/claude-plugins-official.git"
        );
        assert_eq!(
            state.get(OPENAI_MARKETPLACE).unwrap().source.git_url(),
            "https://github.com/openai/plugins.git"
        );
    }

    #[test]
    fn duplicate_user_marketplace_name_with_a_different_source_is_rejected() {
        let mut state = MarketplaceState::with_builtin();
        state
            .add(
                "fixture",
                MarketplaceSource::GitUrl("https://example.invalid/one.git".into()),
            )
            .unwrap();

        assert!(
            state
                .add(
                    "fixture",
                    MarketplaceSource::GitUrl("https://example.invalid/two.git".into()),
                )
                .is_err()
        );
    }

    #[test]
    fn iter_exposes_marketplaces_without_mutation() {
        let state = MarketplaceState::with_builtin();
        let marketplaces: Vec<_> = state.iter().collect();

        assert_eq!(marketplaces.len(), 2);
        assert_eq!(marketplaces[0].0, OFFICIAL_MARKETPLACE);
        assert_eq!(marketplaces[1].0, OPENAI_MARKETPLACE);
    }

    #[test]
    fn deserialization_restores_the_canonical_official_marketplace_source() {
        let state: MarketplaceState = serde_json::from_str(
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

        assert_eq!(
            state
                .get("claude-plugins-official")
                .unwrap()
                .source
                .git_url(),
            "https://github.com/anthropics/claude-plugins-official.git"
        );
        assert!(state.get(OPENAI_MARKETPLACE).is_some());
    }

    #[test]
    fn builtin_marketplaces_cannot_be_removed() {
        let mut state = MarketplaceState::with_builtin();

        for name in [OFFICIAL_MARKETPLACE, OPENAI_MARKETPLACE] {
            assert!(state.remove(name).is_err(), "{name}");
        }
        assert!(state.get(OPENAI_MARKETPLACE).is_some());
    }
}
