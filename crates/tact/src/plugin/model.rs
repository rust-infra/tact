use std::{
    collections::BTreeMap,
    path::{Component, Path},
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// The official marketplace that is available in every marketplace state.
pub const OFFICIAL_MARKETPLACE: &str = "claude-plugins-official";
pub(crate) const MARKETPLACE_BACKUPS_DIRECTORY: &str = ".backups";
const OFFICIAL_MARKETPLACE_URL: &str = "https://github.com/anthropics/claude-plugins-official.git";

/// A marketplace location, either a Git repository or a catalog document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MarketplaceSource {
    GitUrl(String),
    CatalogUrl(String),
}

impl MarketplaceSource {
    /// Parses a GitHub `owner/repository` shorthand or a complete source URL.
    pub fn parse(value: &str) -> Result<Self> {
        let value = value.trim();
        if value.is_empty() {
            bail!("marketplace source cannot be empty");
        }

        if value.contains("://") {
            let url = reqwest::Url::parse(value).with_context(|| format!("invalid marketplace source: {value}"))?;
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

    /// Returns the source URL used to fetch this marketplace.
    #[must_use]
    pub fn git_url(&self) -> String {
        match self {
            Self::GitUrl(url) | Self::CatalogUrl(url) => url.clone(),
        }
    }
}

/// A named marketplace source persisted in the marketplace registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarketplaceRecord {
    pub name: String,
    pub source: MarketplaceSource,
}

/// The persisted marketplace registry.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MarketplaceState {
    marketplaces: BTreeMap<String, MarketplaceRecord>,
}

impl MarketplaceState {
    /// Creates the marketplace registry with the built-in official marketplace.
    #[must_use]
    pub fn with_builtin() -> Self {
        let mut marketplaces = BTreeMap::new();
        marketplaces.insert(
            OFFICIAL_MARKETPLACE.to_owned(),
            MarketplaceRecord {
                name: OFFICIAL_MARKETPLACE.to_owned(),
                source: MarketplaceSource::GitUrl(OFFICIAL_MARKETPLACE_URL.to_owned()),
            },
        );
        Self { marketplaces }
    }

    /// Adds a user marketplace unless it would replace the built-in source.
    pub fn add(&mut self, name: &str, source: MarketplaceSource) -> Result<()> {
        validate_marketplace_name(name)?;
        if name == OFFICIAL_MARKETPLACE {
            bail!("the built-in marketplace cannot be replaced");
        }

        if let Some(existing) = self.marketplaces.get(name) {
            if existing.source == source {
                return Ok(());
            }
            bail!("marketplace {name} already exists with a different source");
        }

        self.marketplaces.insert(name.to_owned(), MarketplaceRecord { name: name.to_owned(), source });
        Ok(())
    }

    /// Removes a user-added marketplace from the registry.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        validate_marketplace_name(name)?;
        if name == OFFICIAL_MARKETPLACE {
            bail!("the built-in marketplace cannot be removed");
        }
        if self.marketplaces.remove(name).is_none() {
            bail!("unknown marketplace {name}");
        }
        Ok(())
    }

    /// Returns the marketplace record for a given name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&MarketplaceRecord> {
        self.marketplaces.get(name)
    }

    /// Iterates over marketplace names and their records.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &MarketplaceRecord)> {
        self.marketplaces.iter().map(|(name, record)| (name.as_str(), record))
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

    /// Restores the canonical built-in marketplace entry.
    pub(crate) fn ensure_builtin(&mut self) {
        self.marketplaces.insert(
            OFFICIAL_MARKETPLACE.to_owned(),
            MarketplaceRecord {
                name: OFFICIAL_MARKETPLACE.to_owned(),
                source: MarketplaceSource::GitUrl(OFFICIAL_MARKETPLACE_URL.to_owned()),
            },
        );
    }
}

pub(crate) fn validate_marketplace_name(name: &str) -> Result<()> {
    if name.trim().is_empty()
        || name == MARKETPLACE_BACKUPS_DIRECTORY
        || name.contains(['/', '\\'])
        || Path::new(name).components().any(|component| !matches!(component, Component::Normal(_)))
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
        let mut state = Self { marketplaces: raw.marketplaces };
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
}

/// The persisted installed-plugin registry.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledState {
    pub plugins: BTreeMap<String, InstalledPlugin>,
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{MarketplaceSource, MarketplaceState, OFFICIAL_MARKETPLACE};
    use crate::consts::PluginHome;

    #[test]
    fn plugin_home_uses_the_tact_plugin_layout() {
        let home = PluginHome::from_home(Path::new("/users/example"));

        assert_eq!(home.root, Path::new("/users/example/.tact/plugins"));
        assert_eq!(home.marketplaces, Path::new("/users/example/.tact/plugins/marketplaces"));
        assert_eq!(home.cache, Path::new("/users/example/.tact/plugins/cache"));
    }

    #[test]
    fn github_shorthand_normalizes_to_git_url() {
        assert_eq!(MarketplaceSource::parse("acme/plugins").unwrap().git_url(), "https://github.com/acme/plugins.git");
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
        for source in ["file:///tmp/marketplace.json", "mailto:marketplace@example.invalid", "git:///marketplace.git"] {
            assert!(MarketplaceSource::parse(source).is_err(), "{source}");
        }
    }

    #[test]
    fn rejects_marketplace_names_with_path_components_before_persistence() {
        let mut state = MarketplaceState::with_builtin();
        for name in ["../outside", "nested/name", "nested\\name", ".", ""] {
            assert!(
                state.add(name, MarketplaceSource::GitUrl("https://example.invalid/a.git".into())).is_err(),
                "{name}"
            );
        }
    }

    #[test]
    fn public_api_cannot_replace_the_builtin_marketplace() {
        let mut state = MarketplaceState::with_builtin();
        assert!(state.add("claude-plugins-official", MarketplaceSource::GitUrl("https://x/y.git".into())).is_err());
        assert_eq!(
            state.get(OFFICIAL_MARKETPLACE).unwrap().source.git_url(),
            "https://github.com/anthropics/claude-plugins-official.git"
        );
    }

    #[test]
    fn duplicate_user_marketplace_name_with_a_different_source_is_rejected() {
        let mut state = MarketplaceState::with_builtin();
        state.add("fixture", MarketplaceSource::GitUrl("https://example.invalid/one.git".into())).unwrap();

        assert!(state.add("fixture", MarketplaceSource::GitUrl("https://example.invalid/two.git".into()),).is_err());
    }

    #[test]
    fn iter_exposes_marketplaces_without_mutation() {
        let state = MarketplaceState::with_builtin();
        let marketplaces: Vec<_> = state.iter().collect();

        assert_eq!(marketplaces.len(), 1);
        assert_eq!(marketplaces[0].0, OFFICIAL_MARKETPLACE);
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
            state.get("claude-plugins-official").unwrap().source.git_url(),
            "https://github.com/anthropics/claude-plugins-official.git"
        );
    }
}
