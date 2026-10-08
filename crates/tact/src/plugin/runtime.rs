//! Runtime plugin lifecycle and manifest registry.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tact_protocol::{CapabilityDeclaration, PluginId, ProtocolVersion};

use crate::kernel::KernelError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginState {
    Registered,
    Running,
    Stopping,
    Stopped,
    Failed,
}

/// Common lifecycle boundary implemented by external and in-process hosts.
#[async_trait]
pub trait PluginHost: Send {
    fn plugin_id(&self) -> &PluginId;
    fn protocol(&self) -> ProtocolVersion;
    fn capabilities(&self) -> &[CapabilityDeclaration];
    fn state(&self) -> PluginState;
    async fn request(
        &mut self,
        request: tact_protocol::PluginRequest,
    ) -> anyhow::Result<tact_protocol::PluginResponse>;
    async fn shutdown(&mut self) -> anyhow::Result<()>;
}

#[derive(Debug, Clone)]
pub struct RuntimePluginManifest {
    pub id: PluginId,
    pub version: String,
    pub protocol: ProtocolVersion,
    pub capabilities: Vec<CapabilityDeclaration>,
}

impl RuntimePluginManifest {
    pub fn validate(&self, supported: ProtocolVersion) -> Result<(), KernelError> {
        if self.version.trim().is_empty() {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                "plugin version cannot be empty",
                "plugin",
                false,
            ));
        }
        if self.protocol.major != supported.major || self.protocol.minor > supported.minor {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::ProtocolMismatch,
                "plugin protocol version is not supported",
                "plugin",
                false,
            ));
        }
        for capability in &self.capabilities {
            capability.validate().map_err(|message| {
                KernelError::new(
                    tact_protocol::ErrorCategory::InvalidRequest,
                    message,
                    "plugin",
                    false,
                )
            })?;
        }
        Ok(())
    }
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

    pub fn register(&self, manifest: RuntimePluginManifest) -> Result<(), KernelError> {
        manifest.validate(self.supported_protocol)?;
        let mut plugins = self.plugins.write().map_err(|_| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                "plugin registry lock poisoned",
                "plugin",
                true,
            )
        })?;
        if plugins.contains_key(&manifest.id) {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                "plugin is already registered",
                "plugin",
                false,
            ));
        }
        plugins.insert(manifest.id.clone(), (manifest, PluginState::Registered));
        Ok(())
    }

    pub fn set_state(&self, id: &PluginId, state: PluginState) -> Result<(), KernelError> {
        let mut plugins = self.plugins.write().map_err(|_| {
            KernelError::new(
                tact_protocol::ErrorCategory::InternalError,
                "plugin registry lock poisoned",
                "plugin",
                true,
            )
        })?;
        let Some((_, current)) = plugins.get_mut(id) else {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::CapabilityNotFound,
                "plugin is not registered",
                "plugin",
                false,
            ));
        };
        *current = state;
        Ok(())
    }

    #[must_use]
    pub fn state(&self, id: &PluginId) -> Option<PluginState> {
        self.plugins.read().ok()?.get(id).map(|(_, state)| *state)
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
