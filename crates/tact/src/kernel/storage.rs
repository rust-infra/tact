//! Namespaced storage facade for Runtime and plugin state.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use serde_json::Value;

use super::{KernelError, StorageService};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StorageNamespace(String);

impl StorageNamespace {
    pub fn runtime() -> Self {
        Self("runtime".into())
    }
    pub fn sessions() -> Self {
        Self("sessions".into())
    }
    pub fn trajectories() -> Self {
        Self("trajectories".into())
    }
    pub fn plugin(plugin_id: &str) -> Result<Self, KernelError> {
        if plugin_id.trim().is_empty() || plugin_id.contains('/') || plugin_id.contains(' ') {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                "invalid plugin storage namespace",
                "storage",
                false,
            ));
        }
        Ok(Self(format!("plugins/{plugin_id}")))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Default)]
pub struct StorageServiceImpl {
    values: Arc<RwLock<BTreeMap<(StorageNamespace, String), Value>>>,
}

impl StorageServiceImpl {
    pub fn get(
        &self,
        namespace: &StorageNamespace,
        key: &str,
    ) -> Result<Option<Value>, KernelError> {
        self.values
            .read()
            .map_err(|_| Self::poisoned())
            .map(|values| values.get(&(namespace.clone(), key.into())).cloned())
    }
    pub fn set(
        &self,
        namespace: StorageNamespace,
        key: impl Into<String>,
        value: Value,
    ) -> Result<(), KernelError> {
        self.values
            .write()
            .map_err(|_| Self::poisoned())
            .map(|mut values| {
                values.insert((namespace, key.into()), value);
            })
    }
    pub fn delete(&self, namespace: &StorageNamespace, key: &str) -> Result<(), KernelError> {
        self.values
            .write()
            .map_err(|_| Self::poisoned())
            .map(|mut values| {
                values.remove(&(namespace.clone(), key.into()));
            })
    }
    fn poisoned() -> KernelError {
        KernelError::new(
            tact_protocol::ErrorCategory::StorageError,
            "storage lock poisoned",
            "storage",
            true,
        )
    }
}

#[async_trait]
impl StorageService for StorageServiceImpl {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>, KernelError> {
        self.get(&StorageNamespace(namespace.to_string()), key)
    }

    async fn set(&self, namespace: &str, key: &str, value: Value) -> Result<(), KernelError> {
        self.set(
            StorageNamespace(namespace.to_string()),
            key.to_string(),
            value,
        )
    }
}
