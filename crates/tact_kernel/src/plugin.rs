//! Plugin identity and manifests.
//!
//! A plugin is a code-level, in-process extension unit. Its manifest is the
//! stable identity the host uses for diagnostics; capability IDs are recorded
//! separately by the registrar.

use std::sync::Arc;

use anyhow::{Result, bail};

use crate::capability::command::Command;
use crate::capability::prompt::PromptSection;
use crate::capability::tool::Tool;
use crate::capability::view::View;
use crate::capability::{CapabilityFilter, CapabilityId, CapabilityKind, SkippedCapability};
use crate::catalog::{
    Catalogs, CommandCatalog, ContributionBatch, PromptCatalog, ToolCatalog, ViewCatalog,
};

/// Stable identity of one code-level plugin.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginId(String);

impl PluginId {
    /// Validates a plugin ID.
    ///
    /// IDs are namespaced labels, not free text: no whitespace and no ASCII
    /// control characters, so they can appear unquoted in diagnostics and
    /// capability IDs.
    pub fn new(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() {
            bail!("plugin id cannot be empty");
        }
        if value.chars().any(char::is_whitespace) {
            bail!("plugin id cannot contain whitespace");
        }
        if value.chars().any(|c| c.is_ascii_control()) {
            bail!("plugin id cannot contain control characters");
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for PluginId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kind of plugin, used for host diagnostics and ordering intent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    Policy,
    Infrastructure,
    Runtime,
    Capability,
    View,
}

/// Static identity of one plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    pub id: PluginId,
    pub name: &'static str,
    pub version: &'static str,
    pub kind: PluginKind,
    pub description: &'static str,
}

impl PluginManifest {
    pub fn new(
        id: PluginId,
        name: &'static str,
        version: &'static str,
        kind: PluginKind,
        description: &'static str,
    ) -> Self {
        Self {
            id,
            name,
            version,
            kind,
            description,
        }
    }
}

/// One code-level plugin.
///
/// Registration is synchronous and one-shot; async setup belongs in the
/// constructor. The host retains the plugin through the runtime lifetime so
/// `shutdown` can release resources in reverse install order.
#[async_trait::async_trait]
pub trait Plugin: Send + Sync {
    fn manifest(&self) -> PluginManifest;

    fn register(&self, registrar: &mut Registrar<'_>) -> anyhow::Result<()>;

    async fn shutdown(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

/// The contribution surface a plugin sees during `register`.
///
/// A disabled capability is skipped without failing the plugin; the skip is
/// recorded with the owning plugin ID. Duplicate names are rejected when the
/// staged batch is committed, so a failed plugin leaves no partial catalogs.
pub struct Registrar<'a> {
    plugin: PluginId,
    filter: &'a CapabilityFilter,
    batch: &'a mut ContributionBatch,
}

impl<'a> Registrar<'a> {
    pub fn new(
        plugin: &PluginId,
        filter: &'a CapabilityFilter,
        batch: &'a mut ContributionBatch,
    ) -> Self {
        Self {
            plugin: plugin.clone(),
            filter,
            batch,
        }
    }

    pub fn tool<T: Tool + 'static>(&mut self, tool: T) -> anyhow::Result<()> {
        let id = CapabilityId::new(CapabilityKind::Tool, tool.descriptor().name.clone());
        self.add(id, |batch| batch.tools.push(Arc::new(tool)))
    }

    pub fn command<T: Command + 'static>(&mut self, command: T) -> anyhow::Result<()> {
        let id = CapabilityId::new(CapabilityKind::Command, command.meta().name.clone());
        self.add(id, |batch| batch.commands.push(Arc::new(command)))
    }

    pub fn prompt_section<T: PromptSection + 'static>(&mut self, section: T) -> anyhow::Result<()> {
        let id = CapabilityId::new(CapabilityKind::PromptSection, section.id());
        self.add(id, |batch| batch.prompts.push(Arc::new(section)))
    }

    pub fn view<T: View + 'static>(&mut self, view: T) -> anyhow::Result<()> {
        let id = CapabilityId::new(CapabilityKind::View, view.id());
        self.add(id, |batch| batch.views.push(Arc::new(view)))
    }

    fn add(
        &mut self,
        id: CapabilityId,
        push: impl FnOnce(&mut ContributionBatch),
    ) -> anyhow::Result<()> {
        crate::capability::validate_name(&id.name)
            .map_err(|error| anyhow::anyhow!("{error} (plugin {})", self.plugin))?;
        if !self.filter.allows(&id) {
            self.batch.skipped.push(SkippedCapability {
                plugin: self.plugin.clone(),
                capability: id,
            });
            return Ok(());
        }
        push(self.batch);
        Ok(())
    }
}

/// Host state that installs plugins and then becomes a sealed [`Runtime`].
pub struct PluginHost {
    filter: CapabilityFilter,
    catalogs: Catalogs,
    plugins: Vec<Box<dyn Plugin>>,
    skipped: Vec<SkippedCapability>,
    poisoned: bool,
    sealed: bool,
}

impl PluginHost {
    pub fn new(filter: CapabilityFilter) -> Self {
        Self {
            filter,
            catalogs: Catalogs::default(),
            plugins: Vec::new(),
            skipped: Vec::new(),
            poisoned: false,
            sealed: false,
        }
    }

    pub fn skipped(&self) -> &[SkippedCapability] {
        &self.skipped
    }

    /// Register one plugin.
    ///
    /// A plugin whose own `register` fails poisons the host: the staged batch
    /// is dropped, earlier plugins and the failed plugin are shut down, and
    /// `seal` refuses to produce a runtime. A duplicate contribution only fails
    /// this install; the previous catalog stays intact and the host stays
    /// usable.
    pub async fn install(&mut self, plugin: impl Plugin + 'static) -> anyhow::Result<()> {
        anyhow::ensure!(
            !self.poisoned,
            "plugin host is unusable after a failed install"
        );
        anyhow::ensure!(!self.sealed, "plugin host is already sealed");

        let plugin: Box<dyn Plugin> = Box::new(plugin);
        let manifest = plugin.manifest();
        if self
            .plugins
            .iter()
            .any(|installed| installed.manifest().id == manifest.id)
        {
            let _ = plugin.shutdown().await;
            anyhow::bail!("duplicate plugin id: {}", manifest.id);
        }

        let mut batch = ContributionBatch::default();
        {
            let mut registrar = Registrar::new(&manifest.id, &self.filter, &mut batch);
            if let Err(error) = plugin.register(&mut registrar) {
                for installed in self.plugins.iter().rev() {
                    let _ = installed.shutdown().await;
                }
                let _ = plugin.shutdown().await;
                self.poisoned = true;
                anyhow::bail!("plugin {} failed to register: {error}", manifest.id);
            }
        }

        if let Err(error) = self.catalogs.commit(&mut batch) {
            let _ = plugin.shutdown().await;
            anyhow::bail!("plugin {} failed to register: {error}", manifest.id);
        }

        self.skipped.append(&mut batch.skipped);
        self.plugins.push(plugin);
        Ok(())
    }

    /// Freeze registration and hand the catalogs to the runtime.
    pub async fn seal(&mut self) -> anyhow::Result<Runtime> {
        anyhow::ensure!(
            !self.poisoned,
            "plugin host is unusable after a failed install"
        );
        anyhow::ensure!(!self.sealed, "plugin host is already sealed");
        self.sealed = true;
        Ok(Runtime {
            catalogs: std::mem::take(&mut self.catalogs),
            plugins: std::mem::take(&mut self.plugins),
            skipped: std::mem::take(&mut self.skipped),
            shut_down: false,
        })
    }
}

/// Sealed catalogs plus the plugins that own their resources.
pub struct Runtime {
    catalogs: Catalogs,
    plugins: Vec<Box<dyn Plugin>>,
    skipped: Vec<SkippedCapability>,
    shut_down: bool,
}

impl Runtime {
    pub fn tools(&self) -> &ToolCatalog {
        &self.catalogs.tools
    }

    pub fn commands(&self) -> &CommandCatalog {
        &self.catalogs.commands
    }

    pub fn prompts(&self) -> &PromptCatalog {
        &self.catalogs.prompts
    }

    pub fn views(&self) -> &ViewCatalog {
        &self.catalogs.views
    }

    pub fn skipped(&self) -> &[SkippedCapability] {
        &self.skipped
    }

    /// Shut every plugin down in reverse install order.
    ///
    /// Idempotent; an individual failure does not skip the remaining plugins.
    pub async fn shutdown(&mut self) -> anyhow::Result<()> {
        if self.shut_down {
            return Ok(());
        }
        self.shut_down = true;
        let mut first_error = None;
        for plugin in self.plugins.iter().rev() {
            if let Err(error) = plugin.shutdown().await
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}
