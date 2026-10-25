//! The marketplace package protocol.
//!
//! A marketplace package is a Claude/Codex-compatible bundle (skills, commands,
//! hooks, MCP servers) — *not* an in-process code plugin. These types are the
//! messages a host and its package worker exchange, plus the records a view
//! renders.
//!
//! They live here because both ends need the same shape: the runtime asks a
//! worker to install or list, and a view draws the installed rows. Keeping them
//! in the extension crate made a view depend on it for a handful of strings.
//!
//! Parsing a source (which needs URL validation) stays with the extension crate;
//! this module only carries the value.

use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Serialize;

/// A request to the marketplace package worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginRequest {
    Install { plugin: String, marketplace: String },
    Uninstall { plugin: String },
    Update { plugin: String },
    List,
    Reload,
    MarketplaceAdd { source: String },
    MarketplaceList,
    MarketplaceUpdate { name: String },
    MarketplaceRemove { name: String },
}

/// The kind of operation associated with a plugin-worker failure.
///
/// A closed set rather than a string: the worker reports one of these, and a
/// view picks a message per variant, so a typo in a name would silently fall
/// through to a generic line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginOperation {
    Install { plugin: String, marketplace: String },
    Uninstall { plugin: String },
    Update { plugin: String },
    List,
    Reload,
    MarketplaceAdd,
    MarketplaceList,
    MarketplaceUpdate { marketplace: String },
    MarketplaceRemove { marketplace: String },
}

impl From<&PluginRequest> for PluginOperation {
    fn from(request: &PluginRequest) -> Self {
        match request {
            PluginRequest::Install {
                plugin,
                marketplace,
            } => Self::Install {
                plugin: plugin.clone(),
                marketplace: marketplace.clone(),
            },
            PluginRequest::Uninstall { plugin } => Self::Uninstall {
                plugin: plugin.clone(),
            },
            PluginRequest::Update { plugin } => Self::Update {
                plugin: plugin.clone(),
            },
            PluginRequest::List => Self::List,
            PluginRequest::Reload => Self::Reload,
            PluginRequest::MarketplaceAdd { .. } => Self::MarketplaceAdd,
            PluginRequest::MarketplaceList => Self::MarketplaceList,
            PluginRequest::MarketplaceUpdate { name } => Self::MarketplaceUpdate {
                marketplace: name.clone(),
            },
            PluginRequest::MarketplaceRemove { name } => Self::MarketplaceRemove {
                marketplace: name.clone(),
            },
        }
    }
}

/// A structured result produced by the plugin worker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginResult {
    Installed {
        plugin: String,
        marketplace: String,
    },
    Uninstalled {
        plugin: String,
    },
    Updated {
        plugin: String,
        marketplace: String,
        revision: String,
    },
    UpToDate {
        plugin: String,
        marketplace: String,
        revision: String,
    },
    ListedInstalled {
        plugins: Vec<InstalledPlugin>,
    },
    Reloaded {
        count: usize,
    },
    MarketplaceAdded {
        marketplace: String,
    },
    ListedMarketplaces {
        marketplaces: Vec<MarketplaceRecord>,
    },
    MarketplaceUpdated {
        marketplace: String,
        count: usize,
    },
    MarketplaceRemoved {
        marketplace: String,
    },
}

/// What the worker reports back.
///
/// `refresh_skills` is a request, not a fact: an install can change the skill
/// catalog, and only the caller knows whether it wants to rescan now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginEvent {
    Succeeded {
        result: PluginResult,
        refresh_skills: bool,
    },
    Failed {
        operation: PluginOperation,
        detail: String,
    },
}

/// A plugin installed from a marketplace.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstalledPlugin {
    pub id: String,
    pub marketplace: String,
    #[serde(default)]
    pub revision: String,
    #[serde(default, alias = "path")]
    pub cache_path: PathBuf,
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

/// A marketplace location, either a Git repository or a catalog document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MarketplaceSource {
    GitUrl(String),
    CatalogUrl(String),
    /// A Codex-style local marketplace root.
    ///
    /// The catalog lives at `<root>/.agents/plugins/marketplace.json`; local
    /// plugin source paths in that catalog are resolved relative to `<root>`.
    LocalPath(PathBuf),
}

impl MarketplaceSource {
    /// How the source is shown to a user.
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

/// Where a Codex-style local marketplace keeps its catalog.
///
/// Candidate order: the personal `.agents/plugins/marketplace.json`, then a
/// repo-local `.codex-plugin/marketplace.json`, then a bare `marketplace.json`.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A request maps to the operation it reports on failure.
    ///
    /// The worker echoes the operation back so a view can say which action
    /// failed; a mismatch would blame the wrong one.
    #[test]
    fn every_request_maps_to_its_operation() {
        let cases = [
            (
                PluginRequest::Install {
                    plugin: "p".into(),
                    marketplace: "m".into(),
                },
                PluginOperation::Install {
                    plugin: "p".into(),
                    marketplace: "m".into(),
                },
            ),
            (
                PluginRequest::Uninstall { plugin: "p".into() },
                PluginOperation::Uninstall { plugin: "p".into() },
            ),
            (
                PluginRequest::Update { plugin: "p".into() },
                PluginOperation::Update { plugin: "p".into() },
            ),
            (PluginRequest::List, PluginOperation::List),
            (PluginRequest::Reload, PluginOperation::Reload),
            (
                PluginRequest::MarketplaceAdd { source: "s".into() },
                PluginOperation::MarketplaceAdd,
            ),
            (
                PluginRequest::MarketplaceList,
                PluginOperation::MarketplaceList,
            ),
            (
                PluginRequest::MarketplaceUpdate { name: "m".into() },
                PluginOperation::MarketplaceUpdate {
                    marketplace: "m".into(),
                },
            ),
            (
                PluginRequest::MarketplaceRemove { name: "m".into() },
                PluginOperation::MarketplaceRemove {
                    marketplace: "m".into(),
                },
            ),
        ];
        for (request, expected) in cases {
            assert_eq!(PluginOperation::from(&request), expected, "{request:?}");
        }
    }

    /// A git-backed source displays its URL verbatim.
    #[test]
    fn a_git_source_displays_its_url() {
        let source = MarketplaceSource::GitUrl("https://example.test/repo.git".into());
        assert_eq!(source.display_source(), "https://example.test/repo.git");
        assert_eq!(source.git_url(), source.display_source());
    }

    /// A local source also names the catalog, so the path is not ambiguous.
    ///
    /// The source path is the resolution root; pointing at it alone reads as
    /// "the wrong directory" because the catalog lives inside it.
    #[test]
    fn a_local_source_names_its_catalog() {
        let dir = tempfile::tempdir().expect("tempdir");
        let source = MarketplaceSource::LocalPath(dir.path().to_path_buf());
        let shown = source.display_source();
        assert!(shown.contains(&dir.path().display().to_string()), "{shown}");
        assert!(shown.contains("catalog:"), "{shown}");
        assert!(shown.contains("marketplace.json"), "{shown}");
    }

    /// The catalog candidate order prefers the personal root.
    #[test]
    fn the_catalog_path_prefers_agents_then_codex_then_bare() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Nothing exists yet: the bare file in the root.
        assert_eq!(
            marketplace_catalog_path(dir.path()),
            dir.path().join("marketplace.json")
        );

        // A Codex-style catalog wins over the bare one.
        std::fs::create_dir_all(dir.path().join(".codex-plugin")).expect("mkdir");
        std::fs::write(dir.path().join(".codex-plugin/marketplace.json"), "{}").expect("write");
        assert_eq!(
            marketplace_catalog_path(dir.path()),
            dir.path().join(".codex-plugin/marketplace.json")
        );

        // The personal agents root wins over both.
        std::fs::create_dir_all(dir.path().join(".agents/plugins")).expect("mkdir");
        std::fs::write(dir.path().join(".agents/plugins/marketplace.json"), "{}").expect("write");
        assert_eq!(
            marketplace_catalog_path(dir.path()),
            dir.path().join(".agents/plugins/marketplace.json")
        );
    }
}
