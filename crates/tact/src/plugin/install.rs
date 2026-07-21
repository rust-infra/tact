use std::{
    fs,
    path::{Component, Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use git2::{ObjectType, Repository};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;
use walkdir::WalkDir;

use super::{CatalogPlugin, InstalledPlugin, MarketplaceService, PluginSource, PluginStore, validate_marketplace_name};
use crate::consts::PluginHome;

/// Installs marketplace plugins into a revision-locked local cache.
#[derive(Clone, Debug)]
pub struct PluginInstaller {
    home: PluginHome,
    marketplace_service: MarketplaceService,
    store: PluginStore,
}

enum ResolvedPluginSource {
    Directory(PathBuf),
    GitTree { repository_root: PathBuf, revision: String, source: PathBuf },
}

impl PluginInstaller {
    /// Creates an installer rooted at the provided plugin home.
    #[must_use]
    pub fn new(home: PluginHome) -> Self {
        Self { marketplace_service: MarketplaceService::new(home.clone()), store: PluginStore::new(home.clone()), home }
    }

    /// Installs one catalog plugin after validating a staged cache candidate.
    pub fn install(&self, plugin_id: &str, marketplace_id: &str) -> Result<InstalledPlugin> {
        validate_plugin_id(plugin_id)?;
        validate_marketplace_name(marketplace_id)?;
        let catalog = match self.marketplace_service.catalog(marketplace_id) {
            Ok(catalog) => catalog,
            Err(_) => super::block_on_async(self.marketplace_service.update_marketplace(marketplace_id))?,
        };
        let plugin = catalog
            .plugins
            .get(plugin_id)
            .with_context(|| format!("unknown plugin {plugin_id} in marketplace {marketplace_id}"))?;
        let (source, revision, fetched_directory) = self.resolve_source(plugin, marketplace_id)?;
        let result = self.install_source(plugin_id, marketplace_id, &source, &revision);
        if let Some(directory) = fetched_directory {
            let _ = fs::remove_dir_all(directory);
        }
        result
    }

    /// Lists currently installed plugins in deterministic registry order.
    pub fn list(&self) -> Result<Vec<InstalledPlugin>> {
        Ok(self.store.load_installed()?.plugins.into_values().collect())
    }

    fn install_source(
        &self,
        plugin_id: &str,
        marketplace_id: &str,
        source: &ResolvedPluginSource,
        revision: &str,
    ) -> Result<InstalledPlugin> {
        let state = self.store.load_installed()?;
        if state.plugins.values().any(|installed| installed.id == plugin_id && installed.marketplace != marketplace_id)
        {
            bail!("plugin namespace {plugin_id} is already installed from another marketplace");
        }
        let destination = self.home.cache.join(marketplace_id).join(plugin_id).join(revision);
        let candidate = destination.with_file_name(format!("{revision}.tmp"));
        let result = (|| -> Result<InstalledPlugin> {
            fs::create_dir_all(destination.parent().context("plugin cache destination has no parent")?)?;
            if candidate.exists() {
                bail!("plugin installation candidate already exists: {}", candidate.display());
            }
            fs::create_dir(&candidate)
                .with_context(|| format!("failed to create plugin installation candidate {}", candidate.display()))?;
            copy_source(source, &candidate)?;
            let skill_count = validate_plugin_candidate(&candidate, plugin_id)?;
            if !destination.exists() {
                fs::rename(&candidate, &destination)
                    .with_context(|| format!("failed to activate plugin cache {}", destination.display()))?;
            } else {
                fs::remove_dir_all(&candidate)
                    .with_context(|| format!("failed to remove duplicate plugin candidate {}", candidate.display()))?;
            }

            let installed = InstalledPlugin {
                id: plugin_id.to_owned(),
                marketplace: marketplace_id.to_owned(),
                revision: revision.to_owned(),
                cache_path: destination,
                skill_count,
            };
            let mut state = state;
            state.plugins.insert(installation_key(marketplace_id, plugin_id), installed.clone());
            self.store.commit_install(&state, &installed.cache_path)?;
            Ok(installed)
        })();
        if result.is_err() && candidate.exists() {
            let _ = fs::remove_dir_all(&candidate);
        }
        result
    }

    fn resolve_source(
        &self,
        plugin: &CatalogPlugin,
        marketplace_id: &str,
    ) -> Result<(ResolvedPluginSource, String, Option<PathBuf>)> {
        match &plugin.source {
            PluginSource::Relative(relative) => {
                let root = self.marketplace_root(marketplace_id);
                let root = root
                    .canonicalize()
                    .with_context(|| format!("failed to resolve marketplace root {}", root.display()))?;
                let source = root
                    .join(relative)
                    .canonicalize()
                    .with_context(|| format!("failed to resolve plugin source {relative} in {}", root.display()))?;
                if !source.starts_with(&root) || !source.is_dir() {
                    bail!("plugin source escapes marketplace root: {}", source.display());
                }
                match git_revision(&root) {
                    Ok(revision) => Ok((
                        ResolvedPluginSource::GitTree {
                            repository_root: root,
                            revision: revision.clone(),
                            source: PathBuf::from(relative),
                        },
                        revision,
                        None,
                    )),
                    Err(_) => {
                        let revision = content_digest(&source)?;
                        Ok((ResolvedPluginSource::Directory(source), revision, None))
                    },
                }
            },
            PluginSource::Git { url, path, reference } => {
                fs::create_dir_all(&self.home.cache)
                    .with_context(|| format!("failed to create plugin cache {}", self.home.cache.display()))?;
                let fetch_root = self.home.cache.join(format!(".fetch-{}", Uuid::new_v4()));
                let repo = Repository::clone(url, &fetch_root)
                    .with_context(|| format!("failed to clone plugin source {url}"))?;
                let revision = checkout_revision(&repo, reference.as_deref())?;
                let source = match path {
                    Some(path) => fetch_root.join(path),
                    None => fetch_root.clone(),
                }
                .canonicalize()
                .with_context(|| format!("failed to resolve fetched plugin source {url}"))?;
                let fetch_root_canonical = fetch_root.canonicalize()?;
                if !source.starts_with(&fetch_root_canonical) || !source.is_dir() {
                    bail!("fetched plugin source escapes repository root");
                }
                Ok((ResolvedPluginSource::Directory(source), revision, Some(fetch_root)))
            },
        }
    }

    fn marketplace_root(&self, marketplace_id: &str) -> PathBuf {
        self.home.marketplaces.join(marketplace_id)
    }
}

fn validate_plugin_id(plugin_id: &str) -> Result<()> {
    if plugin_id.trim().is_empty()
        || plugin_id.contains(['/', '\\'])
        || Path::new(plugin_id).components().count() != 1
        || !matches!(Path::new(plugin_id).components().next(), Some(Component::Normal(_)))
    {
        bail!("invalid plugin id: {plugin_id}");
    }
    Ok(())
}

fn installation_key(marketplace: &str, plugin: &str) -> String {
    format!("{marketplace}/{plugin}")
}

fn copy_source(source: &ResolvedPluginSource, destination: &Path) -> Result<()> {
    match source {
        ResolvedPluginSource::Directory(source) => copy_regular_files(source, destination),
        ResolvedPluginSource::GitTree { repository_root, revision, source } => {
            copy_git_tree(repository_root, revision, source, destination)
        },
    }
}

fn copy_regular_files(source: &Path, destination: &Path) -> Result<()> {
    for entry in WalkDir::new(source).follow_links(false).sort_by_file_name() {
        let entry = entry?;
        let relative = entry.path().strip_prefix(source).expect("walk root prefix");
        if relative.as_os_str().is_empty() {
            continue;
        }
        let target = destination.join(relative);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&target)?;
        } else if entry.file_type().is_file() {
            fs::copy(entry.path(), &target)
                .with_context(|| format!("failed to copy plugin file {}", entry.path().display()))?;
        }
    }
    Ok(())
}

fn copy_git_tree(repository_root: &Path, revision: &str, source: &Path, destination: &Path) -> Result<()> {
    let repository = Repository::open(repository_root)?;
    let commit = repository.revparse_single(revision)?.peel_to_commit()?;
    let tree = commit.tree()?;
    let source_tree = if source == Path::new(".") {
        tree
    } else {
        let entry =
            tree.get_path(source).with_context(|| format!("plugin source is absent from revision {revision}"))?;
        entry.to_object(&repository)?.peel_to_tree()?
    };
    copy_git_tree_entries(&repository, &source_tree, destination)
}

fn copy_git_tree_entries(repository: &Repository, tree: &git2::Tree<'_>, destination: &Path) -> Result<()> {
    for entry in tree {
        let destination = destination.join(entry.name().context("Git tree entry has no filename")?);
        match entry.kind() {
            Some(ObjectType::Tree) => {
                fs::create_dir_all(&destination)?;
                let child = entry.to_object(repository)?.peel_to_tree()?;
                copy_git_tree_entries(repository, &child, &destination)?;
            },
            Some(ObjectType::Blob) if entry.filemode() != 0o120000 => {
                let blob = entry.to_object(repository)?.peel_to_blob()?;
                fs::write(&destination, blob.content())
                    .with_context(|| format!("failed to copy plugin file {}", destination.display()))?;
            },
            _ => {},
        }
    }
    Ok(())
}

fn validate_plugin_candidate(candidate: &Path, plugin_id: &str) -> Result<usize> {
    let manifest = candidate.join(".claude-plugin/plugin.json");
    if manifest.exists() {
        let manifest: CompatibilityManifest = serde_json::from_reader(
            fs::File::open(&manifest)
                .with_context(|| format!("failed to read plugin manifest {}", manifest.display()))?,
        )
        .with_context(|| format!("failed to parse plugin manifest {}", manifest.display()))?;
        if let Some(name) = manifest.name
            && name != plugin_id
        {
            bail!("plugin manifest name {name} conflicts with catalog id {plugin_id}");
        }
    }

    let skills = candidate.join("skills");
    let entries = fs::read_dir(&skills)
        .with_context(|| format!("plugin has no readable skills directory {}", skills.display()))?;
    let mut count = 0;
    for entry in entries {
        let entry = entry?;
        let skill = entry.path().join("SKILL.md");
        if skill.is_file() && fs::File::open(&skill).is_ok() {
            count += 1;
        }
    }
    if count == 0 {
        bail!("plugin {plugin_id} contains no readable skills/*/SKILL.md files");
    }
    Ok(count)
}

#[derive(Deserialize)]
struct CompatibilityManifest {
    #[serde(default)]
    name: Option<String>,
}

fn git_revision(repository_root: &Path) -> Result<String> {
    let repository = Repository::open(repository_root)?;
    checkout_revision(&repository, None)
}

fn checkout_revision(repository: &Repository, reference: Option<&str>) -> Result<String> {
    if let Some(reference) = reference {
        let object = repository
            .revparse_single(reference)
            .with_context(|| format!("failed to resolve Git reference {reference}"))?;
        repository.checkout_tree(&object, None)?;
        repository.set_head_detached(object.peel_to_commit()?.id())?;
    }
    Ok(repository.head()?.peel_to_commit()?.id().to_string())
}

fn content_digest(source: &Path) -> Result<String> {
    let mut hasher = Sha256::new();
    for entry in WalkDir::new(source).follow_links(false).sort_by_file_name().into_iter() {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(source).expect("walk root prefix");
        hasher.update(relative.as_os_str().as_encoded_bytes());
        hasher.update([0]);
        hasher.update(fs::read(entry.path())?);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use git2::{Repository, Signature};
    use tempfile::TempDir;

    use super::{PluginInstaller, validate_plugin_id};
    use crate::{
        consts::PluginHome,
        plugin::{MarketplaceService, MarketplaceSource, PluginStore},
    };

    struct Fixture {
        _home: TempDir,
        installer: PluginInstaller,
    }

    fn fixture_installer() -> Fixture {
        let home = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let repo = Repository::init(repository.path()).unwrap();
        fs::create_dir_all(repository.path().join("plugins/demo/skills/check")).unwrap();
        fs::create_dir_all(repository.path().join("plugins/broken")).unwrap();
        fs::write(
            repository.path().join("marketplace.json"),
            r#"{
                "name": "fixture-market",
                "plugins": [
                    { "name": "demo", "source": "./plugins/demo" },
                    { "name": "broken", "source": "./plugins/broken" }
                ]
            }"#,
        )
        .unwrap();
        fs::write(repository.path().join("plugins/demo/skills/check/SKILL.md"), "---\nname: check\n---\n").unwrap();
        fs::write(repository.path().join("plugins/broken/README.md"), "no skills").unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = Signature::now("fixture", "fixture@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[]).unwrap();

        let plugin_home = PluginHome::from_home(home.path());
        let service = MarketplaceService::new(plugin_home.clone());
        service
            .add_source("fixture-market", MarketplaceSource::GitUrl(repository.path().display().to_string()))
            .unwrap();
        tokio::runtime::Runtime::new().unwrap().block_on(service.update_marketplace("fixture-market")).unwrap();

        Fixture { installer: PluginInstaller::new(plugin_home), _home: home }
    }

    #[test]
    fn install_copies_skill_only_plugin_and_locks_head_revision() {
        let fixture = fixture_installer();
        let installed = fixture.installer.install("demo", "fixture-market").unwrap();

        assert!(installed.cache_path.join("skills/check/SKILL.md").exists());
        assert_eq!(installed.revision.len(), 40);
        assert_eq!(installed.skill_count, 1);
    }

    #[test]
    fn install_refreshes_an_absent_local_marketplace_checkout() {
        let home = tempfile::tempdir().unwrap();
        let repository = tempfile::tempdir().unwrap();
        let repo = Repository::init(repository.path()).unwrap();
        fs::create_dir_all(repository.path().join("plugins/demo/skills/check")).unwrap();
        fs::write(
            repository.path().join("marketplace.json"),
            r#"{
                "name": "fresh-market",
                "plugins": [{ "name": "demo", "source": "./plugins/demo" }]
            }"#,
        )
        .unwrap();
        fs::write(repository.path().join("plugins/demo/skills/check/SKILL.md"), "---\nname: check\n---\n").unwrap();
        let mut index = repo.index().unwrap();
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::now("fixture", "fixture@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &signature, &signature, "fixture", &tree, &[]).unwrap();

        let plugin_home = PluginHome::from_home(home.path());
        MarketplaceService::new(plugin_home.clone())
            .add_source("fresh-market", MarketplaceSource::GitUrl(repository.path().display().to_string()))
            .unwrap();
        assert!(!plugin_home.marketplaces.join("fresh-market").exists());

        let installer = PluginInstaller::new(plugin_home.clone());
        let installed = tokio::runtime::Runtime::new()
            .unwrap()
            .block_on(async move {
                tokio::task::spawn_blocking(move || installer.install("demo", "fresh-market")).await.unwrap()
            })
            .unwrap();

        assert!(plugin_home.marketplaces.join("fresh-market/marketplace.json").is_file());
        assert!(installed.cache_path.join("skills/check/SKILL.md").is_file());
    }

    #[test]
    fn install_copies_relative_source_from_recorded_commit_not_dirty_checkout() {
        let fixture = fixture_installer();
        let skill = fixture.installer.marketplace_root("fixture-market").join("plugins/demo/skills/check/SKILL.md");
        fs::write(&skill, "---\nname: dirty-check\n---\n").unwrap();

        let installed = fixture.installer.install("demo", "fixture-market").unwrap();

        assert_eq!(
            fs::read_to_string(installed.cache_path.join("skills/check/SKILL.md")).unwrap(),
            "---\nname: check\n---\n"
        );
        assert_eq!(installed.revision.len(), 40);
    }

    #[test]
    fn install_allows_missing_compatibility_manifest() {
        let fixture = fixture_installer();

        assert!(fixture.installer.install("demo", "fixture-market").is_ok());
    }

    #[test]
    fn install_rejects_conflicting_compatibility_manifest_name() {
        let fixture = fixture_installer();
        let root = fixture.installer.marketplace_root("fixture-market");
        let source = root.join("plugins/demo/.claude-plugin");
        fs::create_dir_all(&source).unwrap();
        fs::write(source.join("plugin.json"), r#"{ "name": "other" }"#).unwrap();
        commit_worktree(&root);

        assert!(fixture.installer.install("demo", "fixture-market").is_err());
    }

    #[test]
    fn failed_candidate_does_not_replace_previous_install() {
        let fixture = fixture_installer();
        let before = fixture.installer.install("demo", "fixture-market").unwrap();

        assert!(fixture.installer.install("broken", "fixture-market").is_err());
        assert_eq!(fixture.installer.list().unwrap()[0].cache_path, before.cache_path);
    }

    #[test]
    fn install_rejects_plugin_ids_with_path_separators() {
        assert!(validate_plugin_id("demo/other").is_err());
        assert!(validate_plugin_id("demo\\other").is_err());
    }

    #[test]
    fn rejects_plugin_namespace_collision_from_another_marketplace() {
        let fixture = fixture_installer();
        let installed = fixture.installer.install("demo", "fixture-market").unwrap();
        let store = PluginStore::new(fixture.installer.home.clone());
        let mut state = store.load_installed().unwrap();
        let plugin = state.plugins.values_mut().next().unwrap();
        plugin.marketplace = "other-market".into();
        store.commit_install(&state, &installed.cache_path).unwrap();

        assert!(fixture.installer.install("demo", "fixture-market").is_err());
    }

    fn commit_worktree(path: &std::path::Path) {
        let repository = Repository::open(path).unwrap();
        let mut index = repository.index().unwrap();
        index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).unwrap();
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        let signature = Signature::now("fixture", "fixture@example.invalid").unwrap();
        let parent = repository.head().unwrap().peel_to_commit().unwrap();
        repository.commit(Some("HEAD"), &signature, &signature, "fixture update", &tree, &[&parent]).unwrap();
    }
}
