use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, anyhow, bail};
use serde::Deserialize;
use uuid::Uuid;

use crate::consts::PluginHome;

use super::{
    MARKETPLACE_BACKUPS_DIRECTORY, MarketplaceRecord, MarketplaceSource, PluginStore,
    validate_marketplace_name,
};

const MARKETPLACE_FILE: &str = "marketplace.json";

/// A plugin location normalized from a marketplace catalog entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    Relative(String),
    Git {
        url: String,
        path: Option<String>,
        reference: Option<String>,
    },
}

impl PluginSource {
    /// Resolves an on-disk marketplace-relative source and verifies containment.
    pub fn resolve(value: &str, marketplace_root: &Path) -> Result<Self> {
        let relative = normalize_relative(value)?;
        let root = marketplace_root.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize marketplace root {}",
                marketplace_root.display()
            )
        })?;
        let resolved = root.join(&relative).canonicalize().with_context(|| {
            format!(
                "failed to resolve plugin source {value} within {}",
                root.display()
            )
        })?;
        if !resolved.starts_with(&root) {
            bail!("plugin source escapes marketplace root: {value}");
        }
        Ok(Self::Relative(relative))
    }

    fn from_catalog_value(value: RawPluginSource) -> Result<Self> {
        let (source, path, reference) = match value {
            RawPluginSource::String(source) => (source, None, None),
            RawPluginSource::Object {
                source,
                path,
                reference,
            } => (source, path, reference),
        };
        if is_relative_source(&source) {
            let source = match path {
                Some(path) => format!("{}/{}", source.trim_end_matches('/'), path),
                None => source,
            };
            if reference.is_some() {
                bail!("repository-relative plugin source cannot specify ref");
            }
            return Ok(Self::Relative(normalize_relative(&source)?));
        }

        let MarketplaceSource::GitUrl(url) = MarketplaceSource::parse(&source)? else {
            bail!("plugin source must be a Git repository, not a catalog URL");
        };
        if let Some(path) = &path {
            normalize_relative(path)?;
        }
        Ok(Self::Git {
            url,
            path,
            reference,
        })
    }
}

/// A catalog plugin keyed by its marketplace name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogPlugin {
    pub name: String,
    pub source: PluginSource,
}

/// A normalized marketplace catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarketplaceCatalog {
    pub name: String,
    pub plugins: BTreeMap<String, CatalogPlugin>,
}

impl MarketplaceCatalog {
    /// Parses Claude-compatible marketplace JSON without inspecting plugin manifests.
    pub fn parse(content: &str, marketplace_root: &Path) -> Result<Self> {
        let raw: RawCatalog =
            serde_json::from_str(content).context("failed to parse marketplace catalog")?;
        if raw.name.trim().is_empty() {
            bail!("marketplace catalog name cannot be empty");
        }

        let mut plugins = BTreeMap::new();
        for raw_plugin in raw.plugins {
            if raw_plugin.name.trim().is_empty() {
                bail!("marketplace plugin name cannot be empty");
            }
            let mut plugin = CatalogPlugin {
                name: raw_plugin.name.clone(),
                source: PluginSource::from_catalog_value(raw_plugin.source)?,
            };
            if let PluginSource::Relative(source) = &plugin.source {
                plugin.source = PluginSource::resolve(source, marketplace_root)?;
            }
            if plugins.insert(raw_plugin.name.clone(), plugin).is_some() {
                bail!(
                    "marketplace catalog contains duplicate plugin {}",
                    raw_plugin.name
                );
            }
        }

        Ok(Self {
            name: raw.name,
            plugins,
        })
    }
}

/// Persistent marketplace registry and retrieval service.
#[derive(Clone, Debug)]
pub struct MarketplaceService {
    home: PluginHome,
    store: PluginStore,
    client: reqwest::Client,
}

impl MarketplaceService {
    /// Creates a service for the given Tact plugin home.
    #[must_use]
    pub fn new(home: PluginHome) -> Self {
        Self {
            store: PluginStore::new(home.clone()),
            home,
            client: reqwest::Client::new(),
        }
    }

    /// Adds a marketplace source to the persistent registry.
    pub fn add_source(&self, name: &str, source: MarketplaceSource) -> Result<()> {
        let mut marketplaces = self.store.load_marketplaces()?;
        marketplaces.add(name, source)?;
        self.store.save_marketplaces(&marketplaces)
    }

    /// Retrieves a source and records it under the name declared by its catalog.
    pub async fn add_catalog_source(&self, source: MarketplaceSource) -> Result<String> {
        fs::create_dir_all(&self.home.marketplaces)?;
        let candidate = self
            .home
            .marketplaces
            .join(format!(".add-{}", Uuid::new_v4()));
        let refreshed = match &source {
            MarketplaceSource::GitUrl(url) => self.refresh_git(url, &candidate),
            MarketplaceSource::CatalogUrl(url) => self.refresh_catalog_url(url, &candidate).await,
        };
        if let Err(error) = refreshed {
            let _ = fs::remove_dir_all(&candidate);
            return Err(error);
        }

        let result = (|| -> Result<String> {
            let catalog = self.catalog_at(&candidate)?;
            validate_marketplace_name(&catalog.name)?;
            let mut marketplaces = self.store.load_marketplaces()?;
            marketplaces.add(&catalog.name, source)?;
            let destination = self.marketplace_path(&catalog.name)?;
            self.activate_candidate(&candidate, &destination)?;
            self.store.save_marketplaces(&marketplaces)?;
            Ok(catalog.name)
        })();
        if result.is_err() && candidate.exists() {
            let _ = fs::remove_dir_all(&candidate);
        }
        result
    }

    /// Removes a user marketplace from the persistent registry.
    pub fn remove_source(&self, name: &str) -> Result<()> {
        let mut marketplaces = self.store.load_marketplaces()?;
        marketplaces.remove(name)?;
        self.store.save_marketplaces(&marketplaces)
    }

    /// Refreshes one marketplace and returns its normalized catalog.
    pub async fn update_marketplace(&self, name: &str) -> Result<MarketplaceCatalog> {
        let marketplaces = self.store.load_marketplaces()?;
        let record = marketplaces
            .get(name)
            .cloned()
            .with_context(|| format!("unknown marketplace {name}"))?;
        self.refresh_record(&record).await?;
        self.catalog(name)
    }

    /// Reads a previously refreshed marketplace catalog.
    pub fn catalog(&self, name: &str) -> Result<MarketplaceCatalog> {
        let root = self.marketplace_path(name)?;
        self.restore_interrupted_replacement(&root)?;
        self.ensure_marketplace_root_is_contained(&root)?;
        self.catalog_at(&root)
    }

    fn catalog_at(&self, root: &Path) -> Result<MarketplaceCatalog> {
        self.ensure_marketplace_root_is_contained(root)?;
        let path = catalog_path(root);
        let content = fs::read_to_string(&path)
            .with_context(|| format!("failed to read marketplace catalog {}", path.display()))?;
        MarketplaceCatalog::parse(&content, root)
    }

    async fn refresh_record(&self, record: &MarketplaceRecord) -> Result<()> {
        let destination = self.marketplace_path(&record.name)?;
        self.restore_interrupted_replacement(&destination)?;
        fs::create_dir_all(&self.home.marketplaces).with_context(|| {
            format!(
                "failed to create marketplace directory {}",
                self.home.marketplaces.display()
            )
        })?;

        match &record.source {
            MarketplaceSource::GitUrl(url) => self.refresh_git(url, &destination),
            MarketplaceSource::CatalogUrl(url) => self.refresh_catalog_url(url, &destination).await,
        }
    }

    fn refresh_git(&self, url: &str, destination: &Path) -> Result<()> {
        let temporary = self.temporary_directory(destination)?;
        let result = (|| -> Result<()> {
            git2::Repository::clone(url, &temporary)
                .with_context(|| format!("failed to clone marketplace {url}"))?;
            self.activate_candidate(&temporary, destination)
        })();
        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary);
        }
        result
    }

    async fn refresh_catalog_url(&self, url: &str, destination: &Path) -> Result<()> {
        let temporary = self.temporary_directory(destination)?;
        let result = async {
            fs::create_dir_all(&temporary).with_context(|| {
                format!(
                    "failed to create temporary catalog directory {}",
                    temporary.display()
                )
            })?;
            let response = self.client.get(url).send().await?.error_for_status()?;
            let content = response.text().await?;
            fs::write(temporary.join(MARKETPLACE_FILE), content).with_context(|| {
                format!(
                    "failed to write temporary marketplace catalog {}",
                    temporary.display()
                )
            })?;
            self.activate_candidate(&temporary, destination)
        }
        .await;
        if result.is_err() {
            let _ = fs::remove_dir_all(&temporary);
        }
        result
    }

    fn marketplace_path(&self, name: &str) -> Result<PathBuf> {
        validate_marketplace_name(name)?;
        Ok(self.home.marketplaces.join(name))
    }

    fn ensure_marketplace_root_is_contained(&self, root: &Path) -> Result<()> {
        let marketplaces = self.home.marketplaces.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize marketplace home {}",
                self.home.marketplaces.display()
            )
        })?;
        let root = root.canonicalize().with_context(|| {
            format!("failed to canonicalize marketplace root {}", root.display())
        })?;
        if !root.starts_with(&marketplaces) {
            bail!(
                "marketplace root escapes marketplace home: {}",
                root.display()
            );
        }
        Ok(())
    }

    fn activate_candidate(&self, candidate: &Path, destination: &Path) -> Result<()> {
        self.ensure_marketplace_root_is_contained(candidate)?;
        let catalog = catalog_path(candidate);
        let content = fs::read_to_string(&catalog).with_context(|| {
            format!(
                "failed to read candidate marketplace catalog {}",
                catalog.display()
            )
        })?;
        MarketplaceCatalog::parse(&content, candidate)?;
        if destination.exists() {
            self.ensure_marketplace_root_is_contained(destination)?;
        }
        let backup = self.backup_path(destination)?;
        if destination.exists() {
            self.prepare_backup_directory(&backup)?;
        }
        replace_directory(candidate, destination, &backup)
    }

    fn restore_interrupted_replacement(&self, destination: &Path) -> Result<()> {
        let backup = self.backup_path(destination)?;
        if !destination.exists() && backup.exists() {
            let backup_directory = backup
                .parent()
                .context("marketplace backup has no parent directory")?;
            self.ensure_backup_directory_is_contained(backup_directory)?;
            fs::rename(&backup, destination).with_context(|| {
                format!(
                    "failed to restore marketplace backup {} to {}",
                    backup.display(),
                    destination.display()
                )
            })?;
        }
        Ok(())
    }

    fn backup_path(&self, destination: &Path) -> Result<PathBuf> {
        let name = destination
            .file_name()
            .context("marketplace destination has no name")?;
        Ok(self
            .home
            .marketplaces
            .join(MARKETPLACE_BACKUPS_DIRECTORY)
            .join(name))
    }

    fn prepare_backup_directory(&self, backup: &Path) -> Result<()> {
        let backup_directory = backup
            .parent()
            .context("marketplace backup has no parent directory")?;
        fs::create_dir_all(backup_directory).with_context(|| {
            format!(
                "failed to create marketplace backup directory {}",
                backup_directory.display()
            )
        })?;
        self.ensure_backup_directory_is_contained(backup_directory)
    }

    fn ensure_backup_directory_is_contained(&self, backup_directory: &Path) -> Result<()> {
        let marketplaces = self.home.marketplaces.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize marketplace home {}",
                self.home.marketplaces.display()
            )
        })?;
        let backup_directory = backup_directory.canonicalize().with_context(|| {
            format!(
                "failed to canonicalize marketplace backup directory {}",
                backup_directory.display()
            )
        })?;
        let expected = marketplaces.join(MARKETPLACE_BACKUPS_DIRECTORY);
        if backup_directory != expected {
            bail!(
                "marketplace backup directory escapes marketplace home: {}",
                backup_directory.display()
            );
        }
        Ok(())
    }

    fn temporary_directory(&self, destination: &Path) -> Result<PathBuf> {
        let parent = destination
            .parent()
            .context("marketplace destination has no parent")?;
        let name = destination
            .file_name()
            .context("marketplace destination has no name")?;
        Ok(parent.join(format!(
            ".{}.{}.tmp",
            name.to_string_lossy(),
            Uuid::new_v4()
        )))
    }
}

#[derive(Deserialize)]
struct RawCatalog {
    name: String,
    #[serde(default)]
    plugins: Vec<RawCatalogPlugin>,
}

#[derive(Deserialize)]
struct RawCatalogPlugin {
    name: String,
    source: RawPluginSource,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawPluginSource {
    String(String),
    Object {
        source: String,
        #[serde(default)]
        path: Option<String>,
        #[serde(default, rename = "ref")]
        reference: Option<String>,
    },
}

fn is_relative_source(value: &str) -> bool {
    value.starts_with('.') || Path::new(value).is_absolute()
}

fn normalize_relative(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() {
        bail!("plugin source cannot be empty");
    }
    let path = Path::new(value);
    if path.is_absolute() {
        bail!("plugin source must be repository-relative: {value}");
    }
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                bail!("plugin source escapes marketplace root: {value}")
            }
        }
    }
    if normalized.as_os_str().is_empty() {
        bail!("plugin source cannot be the marketplace root");
    }
    Ok(normalized.to_string_lossy().into_owned())
}

fn replace_directory(temporary: &Path, destination: &Path, backup: &Path) -> Result<()> {
    if !destination.exists() {
        return fs::rename(temporary, destination).with_context(|| {
            format!(
                "failed to activate refreshed marketplace {}",
                destination.display()
            )
        });
    }
    if backup.exists() {
        fs::remove_dir_all(backup).with_context(|| {
            format!(
                "failed to remove stale marketplace backup {}",
                backup.display()
            )
        })?;
    }
    fs::rename(destination, backup).with_context(|| {
        format!(
            "failed to stage existing marketplace {}",
            destination.display()
        )
    })?;
    if let Err(error) = fs::rename(temporary, destination) {
        return match fs::rename(backup, destination) {
            Ok(()) => Err(error).with_context(|| {
                format!(
                    "failed to activate refreshed marketplace {}",
                    destination.display()
                )
            }),
            Err(rollback_error) => Err(anyhow!(
                "failed to activate refreshed marketplace {}; rollback from {} also failed: {}; activation error: {}",
                destination.display(),
                backup.display(),
                rollback_error,
                error
            )),
        };
    }
    fs::remove_dir_all(backup)
        .with_context(|| format!("failed to remove previous marketplace {}", backup.display()))
}

fn catalog_path(root: &Path) -> PathBuf {
    let claude_path = root.join(".claude-plugin").join(MARKETPLACE_FILE);
    if claude_path.exists() {
        claude_path
    } else {
        root.join(MARKETPLACE_FILE)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, os::unix::fs::symlink};

    use git2::{Repository, Signature};
    use tempfile::tempdir;

    use super::{
        MARKETPLACE_BACKUPS_DIRECTORY, MarketplaceCatalog, MarketplaceService, MarketplaceSource,
        PluginSource,
    };
    use crate::consts::PluginHome;

    fn commit_repository(path: &std::path::Path) {
        let repository = Repository::init(path).unwrap();
        let mut index = repository.index().unwrap();
        index
            .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
            .unwrap();
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::now("fixture", "fixture@example.invalid").unwrap();
        repository
            .commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[])
            .unwrap();
    }

    #[tokio::test]
    async fn add_catalog_source_uses_the_catalog_declared_name() {
        let home = tempdir().unwrap();
        let repository = tempdir().unwrap();
        fs::write(
            repository.path().join("marketplace.json"),
            r#"{"name":"catalog-owned","plugins":[]}"#,
        )
        .unwrap();
        commit_repository(repository.path());
        let service = MarketplaceService::new(PluginHome::from_home(home.path()));

        let name = service
            .add_catalog_source(MarketplaceSource::GitUrl(
                repository.path().display().to_string(),
            ))
            .await
            .unwrap();

        assert_eq!(name, "catalog-owned");
        assert!(service.catalog("catalog-owned").is_ok());
    }

    #[test]
    fn parses_claude_catalog_without_requiring_plugin_manifest() {
        const CLAUDE_FIXTURE: &str = r#"{
          "name":"fixture-market",
          "plugins":[{"name":"superpowers","source":"./plugins/superpowers"}]
        }"#;
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("plugins/superpowers")).unwrap();

        let catalog = MarketplaceCatalog::parse(CLAUDE_FIXTURE, root.path()).unwrap();

        assert_eq!(catalog.name, "fixture-market");
        assert_eq!(
            catalog.plugins["superpowers"].source,
            PluginSource::Relative("plugins/superpowers".into())
        );
    }

    #[test]
    fn parses_object_plugin_source() {
        let root = tempdir().unwrap();
        let catalog = MarketplaceCatalog::parse(
            r#"{
                "name":"fixture-market",
                "plugins":[{
                    "name":"external",
                    "source":{"source":"acme/plugins","path":"plugin"}
                }]
            }"#,
            root.path(),
        )
        .unwrap();

        assert_eq!(
            catalog.plugins["external"].source,
            PluginSource::Git {
                url: "https://github.com/acme/plugins.git".into(),
                path: Some("plugin".into()),
                reference: None,
            }
        );
    }

    #[test]
    fn rejects_relative_source_that_escapes_git_root() {
        let root = tempdir().unwrap();
        assert!(PluginSource::resolve("../../etc", root.path()).is_err());
    }

    #[test]
    fn resolves_relative_source_only_within_marketplace_root() {
        let root = tempdir().unwrap();
        fs::create_dir_all(root.path().join("plugins/superpowers")).unwrap();

        assert_eq!(
            PluginSource::resolve("./plugins/superpowers", root.path()).unwrap(),
            PluginSource::Relative("plugins/superpowers".into())
        );
    }

    #[test]
    fn catalog_rejects_relative_source_that_escapes_through_a_symlink() {
        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let root = plugin_home.marketplaces.join("fixture-market");
        fs::create_dir_all(root.join("plugins")).unwrap();
        symlink(outside.path(), root.join("plugins/escaped")).unwrap();
        fs::write(
            root.join("marketplace.json"),
            r#"{
                "name":"fixture-market",
                "plugins":[{"name":"escaped","source":"./plugins/escaped"}]
            }"#,
        )
        .unwrap();

        assert!(
            MarketplaceService::new(plugin_home)
                .catalog("fixture-market")
                .is_err()
        );
    }

    #[test]
    fn catalog_rejects_marketplace_root_symlink_that_escapes_marketplace_home() {
        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        fs::create_dir_all(&plugin_home.marketplaces).unwrap();
        fs::create_dir_all(outside.path().join("plugins/inside")).unwrap();
        fs::write(
            outside.path().join("marketplace.json"),
            r#"{"name":"fixture-market","plugins":[{"name":"inside","source":"./plugins/inside"}]}"#,
        )
        .unwrap();
        symlink(
            outside.path(),
            plugin_home.marketplaces.join("fixture-market"),
        )
        .unwrap();

        assert!(
            MarketplaceService::new(plugin_home)
                .catalog("fixture-market")
                .is_err()
        );
    }

    #[test]
    fn invalid_candidate_does_not_replace_existing_catalog() {
        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let service = MarketplaceService::new(plugin_home.clone());
        let destination = plugin_home.marketplaces.join("fixture-market");
        let candidate = plugin_home.marketplaces.join("candidate");
        fs::create_dir_all(destination.join("plugins/old")).unwrap();
        fs::write(
            destination.join("marketplace.json"),
            r#"{"name":"fixture-market","plugins":[{"name":"old","source":"./plugins/old"}]}"#,
        )
        .unwrap();
        fs::create_dir_all(&candidate).unwrap();
        fs::write(
            candidate.join("marketplace.json"),
            r#"{"name":"fixture-market","plugins":[{"name":"bad","source":"./missing"}]}"#,
        )
        .unwrap();

        assert!(
            service
                .activate_candidate(&candidate, &destination)
                .is_err()
        );
        assert_eq!(service.catalog("fixture-market").unwrap().plugins.len(), 1);
        assert!(
            service
                .catalog("fixture-market")
                .unwrap()
                .plugins
                .contains_key("old")
        );
    }

    #[test]
    fn replacing_marketplace_does_not_collide_with_backup_suffix_marketplace() {
        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let service = MarketplaceService::new(plugin_home.clone());
        let destination = plugin_home.marketplaces.join("foo");
        let suffix_marketplace = plugin_home.marketplaces.join("foo.backup");
        let candidate = plugin_home.marketplaces.join("candidate");

        fs::create_dir_all(destination.join("plugins/old")).unwrap();
        fs::write(
            destination.join("marketplace.json"),
            r#"{"name":"foo","plugins":[{"name":"old","source":"./plugins/old"}]}"#,
        )
        .unwrap();
        fs::create_dir_all(suffix_marketplace.join("plugins/other")).unwrap();
        fs::write(
            suffix_marketplace.join("marketplace.json"),
            r#"{"name":"foo.backup","plugins":[{"name":"other","source":"./plugins/other"}]}"#,
        )
        .unwrap();
        fs::create_dir_all(candidate.join("plugins/new")).unwrap();
        fs::write(
            candidate.join("marketplace.json"),
            r#"{"name":"foo","plugins":[{"name":"new","source":"./plugins/new"}]}"#,
        )
        .unwrap();

        service
            .activate_candidate(&candidate, &destination)
            .unwrap();

        assert!(service.catalog("foo").unwrap().plugins.contains_key("new"));
        assert!(
            service
                .catalog("foo.backup")
                .unwrap()
                .plugins
                .contains_key("other")
        );
    }

    #[test]
    fn catalog_restores_backup_left_by_interrupted_replacement() {
        let home = tempdir().unwrap();
        let plugin_home = PluginHome::from_home(home.path());
        let service = MarketplaceService::new(plugin_home.clone());
        let backup = plugin_home
            .marketplaces
            .join(MARKETPLACE_BACKUPS_DIRECTORY)
            .join("fixture-market");
        fs::create_dir_all(backup.join("plugins/restored")).unwrap();
        fs::write(
            backup.join("marketplace.json"),
            r#"{"name":"fixture-market","plugins":[{"name":"restored","source":"./plugins/restored"}]}"#,
        )
        .unwrap();

        let catalog = service.catalog("fixture-market").unwrap();

        assert!(catalog.plugins.contains_key("restored"));
        assert!(plugin_home.marketplaces.join("fixture-market").is_dir());
        assert!(!backup.exists());
    }
}
