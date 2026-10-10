//! Plugin lifecycle: which extensions exist and what state they are in.
//!
//! The Kernel owns the *record* of a plugin — its manifest, declared
//! capabilities, and lifecycle state — so permission, capability routing, and
//! events can all agree on what is registered. The runtime machinery that
//! starts, supervises and stops a host process lives in `tact_plugin_host`;
//! the state transitions here are what that machinery reports into.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use tact_protocol::{CapabilityDeclaration, ErrorCategory, PluginId, ProtocolVersion};

use crate::KernelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Registered,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// What the Kernel knows about one plugin's runtime condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHealth {
    pub id: PluginId,
    pub state: PluginState,
}

impl PluginHealth {
    /// A crashed plugin is unhealthy; a stopped one is idle, not broken.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !matches!(self.state, PluginState::Failed)
    }
}

#[derive(Debug, Clone)]
pub struct RuntimePluginManifest {
    pub id: PluginId,
    pub version: String,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<CapabilityDeclaration>,
    /// Plugins that must be registered before this one can be.
    pub dependencies: Vec<PluginId>,
}

impl RuntimePluginManifest {
    pub fn validate(&self, supported: ProtocolVersion) -> Result<(), KernelError> {
        if self.version.trim().is_empty() {
            return Err(KernelError::new(
                ErrorCategory::InvalidRequest,
                "plugin version cannot be empty",
                "plugin",
                false,
            ));
        }
        if self.dependencies.contains(&self.id) {
            return Err(KernelError::new(
                ErrorCategory::InvalidRequest,
                "plugin cannot depend on itself",
                "plugin",
                false,
            ));
        }
        // Version negotiation is a Kernel primitive shared by every host, so
        // the registry delegates to it rather than re-checking major/minor.
        crate::protocol::negotiate(self.protocol, supported)?;
        for capability in &self.capabilities {
            capability.validate().map_err(|message| {
                KernelError::new(ErrorCategory::InvalidRequest, message, "plugin", false)
            })?;
        }
        Ok(())
    }
}

fn poisoned() -> KernelError {
    KernelError::new(
        ErrorCategory::InternalError,
        "plugin registry lock poisoned",
        "plugin",
        true,
    )
}

fn not_registered() -> KernelError {
    KernelError::new(
        ErrorCategory::CapabilityNotFound,
        "plugin is not registered",
        "plugin",
        false,
    )
}

/// Refuses a manifest whose dependencies are not registered yet.
///
/// Requiring dependencies to exist first is also what makes registration
/// cycles unrepresentable: two plugins that need each other can never both be
/// inserted.
fn check_dependencies(
    plugins: &BTreeMap<PluginId, (RuntimePluginManifest, PluginState)>,
    manifest: &RuntimePluginManifest,
) -> Result<(), KernelError> {
    for dependency in &manifest.dependencies {
        if !plugins.contains_key(dependency) {
            return Err(KernelError::new(
                ErrorCategory::PluginUnavailable,
                format!(
                    "plugin {} requires {} to be registered first",
                    manifest.id, dependency
                ),
                "plugin",
                false,
            ));
        }
    }
    Ok(())
}

#[derive(Clone)]
pub struct PluginRegistry {
    supported_protocol: ProtocolVersion,
    plugins: Arc<RwLock<BTreeMap<PluginId, (RuntimePluginManifest, PluginState)>>>,
}

impl PluginRegistry {
    #[must_use]
    pub fn new(supported_protocol: ProtocolVersion) -> Self {
        Self {
            supported_protocol,
            plugins: Arc::new(RwLock::new(BTreeMap::new())),
        }
    }

    /// Filters `candidates` down to the manifests that may be registered right
    /// now: each one validates, its ID is free, and its dependencies are
    /// already registered.
    ///
    /// Discovery is caller-driven — the Kernel does not know where a host keeps
    /// its plugins, only what makes one registrable.
    #[must_use]
    pub fn discover(
        &self,
        candidates: impl IntoIterator<Item = RuntimePluginManifest>,
    ) -> Vec<RuntimePluginManifest> {
        let Ok(plugins) = self.plugins.read() else {
            return Vec::new();
        };
        candidates
            .into_iter()
            .filter(|manifest| {
                !plugins.contains_key(&manifest.id)
                    && manifest.validate(self.supported_protocol).is_ok()
                    && check_dependencies(&plugins, manifest).is_ok()
            })
            .collect()
    }

    pub fn register(&self, manifest: RuntimePluginManifest) -> Result<(), KernelError> {
        manifest.validate(self.supported_protocol)?;
        let mut plugins = self.plugins.write().map_err(|_| poisoned())?;
        if plugins.contains_key(&manifest.id) {
            return Err(KernelError::new(
                ErrorCategory::InvalidRequest,
                "plugin is already registered",
                "plugin",
                false,
            ));
        }
        check_dependencies(&plugins, &manifest)?;
        plugins.insert(manifest.id.clone(), (manifest, PluginState::Registered));
        Ok(())
    }

    /// Replaces a stopped extension manifest while retaining the same stable
    /// plugin ID. A running or stopping extension must be shut down first.
    pub fn replace(&self, manifest: RuntimePluginManifest) -> Result<(), KernelError> {
        manifest.validate(self.supported_protocol)?;
        let mut plugins = self.plugins.write().map_err(|_| poisoned())?;
        let Some((_, state)) = plugins.get(&manifest.id) else {
            return Err(not_registered());
        };
        if matches!(state, PluginState::Running | PluginState::Stopping) {
            return Err(KernelError::new(
                ErrorCategory::InvalidRequest,
                "running plugin must stop before its manifest can be replaced",
                "plugin",
                false,
            ));
        }
        check_dependencies(&plugins, &manifest)?;
        plugins.insert(manifest.id.clone(), (manifest, PluginState::Registered));
        Ok(())
    }

    /// Drops a plugin from the registry. A running plugin must stop first, so
    /// an in-flight host is never forgotten while it still owns capabilities.
    pub fn unregister(&self, id: &PluginId) -> Result<(), KernelError> {
        let mut plugins = self.plugins.write().map_err(|_| poisoned())?;
        let Some((_, state)) = plugins.get(id) else {
            return Err(not_registered());
        };
        if matches!(state, PluginState::Running | PluginState::Stopping) {
            return Err(KernelError::new(
                ErrorCategory::InvalidRequest,
                "running plugin must stop before it can be unregistered",
                "plugin",
                false,
            ));
        }
        plugins.remove(id);
        Ok(())
    }

    pub fn set_state(&self, id: &PluginId, state: PluginState) -> Result<(), KernelError> {
        let mut plugins = self.plugins.write().map_err(|_| poisoned())?;
        let Some((_, current)) = plugins.get_mut(id) else {
            return Err(not_registered());
        };
        *current = state;
        Ok(())
    }

    /// Marks a plugin as serving. The host process itself is started by
    /// `tact_plugin_host`; this is the registry's half of that transition.
    pub fn start(&self, id: &PluginId) -> Result<(), KernelError> {
        self.set_state(id, PluginState::Running)
    }

    /// Marks a plugin as no longer serving.
    pub fn stop(&self, id: &PluginId) -> Result<(), KernelError> {
        self.set_state(id, PluginState::Stopped)
    }

    /// Stops then starts, which is also how a crashed plugin is brought back.
    pub fn restart(&self, id: &PluginId) -> Result<(), KernelError> {
        self.stop(id)?;
        self.start(id)
    }

    pub fn enable(&self, id: &PluginId) -> Result<(), KernelError> {
        self.start(id)
    }

    pub fn disable(&self, id: &PluginId) -> Result<(), KernelError> {
        self.stop(id)
    }

    #[must_use]
    pub fn state(&self, id: &PluginId) -> Option<PluginState> {
        self.plugins.read().ok()?.get(id).map(|(_, state)| *state)
    }

    /// The health record for one plugin, or `None` when it is not registered.
    #[must_use]
    pub fn health(&self, id: &PluginId) -> Option<PluginHealth> {
        let state = self.state(id)?;
        Some(PluginHealth {
            id: id.clone(),
            state,
        })
    }

    /// The health record for every registered plugin, ordered by plugin ID.
    #[must_use]
    pub fn health_all(&self) -> Vec<PluginHealth> {
        self.plugins
            .read()
            .map(|plugins| {
                plugins
                    .iter()
                    .map(|(id, (_, state))| PluginHealth {
                        id: id.clone(),
                        state: *state,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    #[must_use]
    pub fn manifests(&self) -> Vec<RuntimePluginManifest> {
        self.plugins
            .read()
            .map(|plugins| {
                plugins
                    .values()
                    .map(|(manifest, _)| manifest.clone())
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use tact_protocol::{CapabilityKind, CapabilityRisk};

    fn manifest(id: &str) -> RuntimePluginManifest {
        RuntimePluginManifest {
            id: PluginId::from(id),
            version: "1".into(),
            protocol: ProtocolVersion::CURRENT,
            capabilities: vec![CapabilityDeclaration {
                name: format!("{id}.run"),
                kind: CapabilityKind::Service,
                version: "1".into(),
                description: None,
                input_schema: None,
                output_schema: None,
                risk: CapabilityRisk::ReadOnly,
            }],
            dependencies: Vec::new(),
        }
    }

    #[test]
    fn a_missing_dependency_blocks_registration_until_it_is_registered() {
        let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
        let mut dependent = manifest("tact.chat");
        dependent.dependencies = vec![PluginId::from("tact.agent")];

        let error = registry
            .register(dependent.clone())
            .expect_err("unregistered dependency must fail");
        assert_eq!(error.category(), ErrorCategory::PluginUnavailable);
        assert!(registry.state(&dependent.id).is_none());

        registry
            .register(manifest("tact.agent"))
            .expect("dependency");
        registry
            .register(dependent.clone())
            .expect("dependent registers once its dependency exists");
        assert_eq!(registry.state(&dependent.id), Some(PluginState::Registered));
    }

    #[test]
    fn a_plugin_cannot_depend_on_itself() {
        let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
        let mut selfish = manifest("tact.loop");
        selfish.dependencies = vec![selfish.id.clone()];
        let error = registry.register(selfish).expect_err("self dependency");
        assert_eq!(error.category(), ErrorCategory::InvalidRequest);
    }

    #[test]
    fn discover_keeps_only_manifests_that_can_be_registered() {
        let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
        registry.register(manifest("tact.agent")).expect("register");

        let mut missing_dependency = manifest("tact.chat");
        missing_dependency.dependencies = vec![PluginId::from("tact.nowhere")];

        let mut wrong_protocol = manifest("tact.old");
        wrong_protocol.protocol = ProtocolVersion::new(2, 0);

        let discovered = registry.discover(vec![
            manifest("tact.agent"),
            missing_dependency,
            wrong_protocol,
            manifest("tact.tools"),
        ]);

        assert_eq!(
            discovered
                .iter()
                .map(|manifest| manifest.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tact.tools"],
            "an already-registered id, a missing dependency, and a protocol mismatch are all skipped"
        );
    }

    #[test]
    fn health_reports_crash_and_lifecycle_transitions() {
        let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
        let plugin = PluginId::from("tact.tools");
        registry.register(manifest("tact.tools")).expect("register");

        let health = registry.health(&plugin).expect("health");
        assert_eq!(health.state, PluginState::Registered);
        assert!(health.is_healthy());

        registry.start(&plugin).expect("start");
        assert_eq!(registry.state(&plugin), Some(PluginState::Running));

        registry.restart(&plugin).expect("restart");
        assert_eq!(registry.state(&plugin), Some(PluginState::Running));

        registry
            .set_state(&plugin, PluginState::Failed)
            .expect("crash");
        let crashed = registry.health(&plugin).expect("health");
        assert!(!crashed.is_healthy(), "a crashed plugin is not healthy");

        assert_eq!(registry.health_all().len(), 1);
        assert!(registry.health(&PluginId::from("tact.absent")).is_none());
    }

    #[test]
    fn unregister_refuses_a_running_plugin_and_removes_a_stopped_one() {
        let registry = PluginRegistry::new(ProtocolVersion::CURRENT);
        let plugin = PluginId::from("tact.tools");
        registry.register(manifest("tact.tools")).expect("register");

        registry.start(&plugin).expect("start");
        let error = registry
            .unregister(&plugin)
            .expect_err("a running plugin cannot be unregistered");
        assert_eq!(error.category(), ErrorCategory::InvalidRequest);

        registry.stop(&plugin).expect("stop");
        registry.unregister(&plugin).expect("unregister");
        assert!(registry.state(&plugin).is_none());
        assert!(registry.manifests().is_empty());

        let error = registry.unregister(&plugin).expect_err("already gone");
        assert_eq!(error.category(), ErrorCategory::CapabilityNotFound);
    }
}
