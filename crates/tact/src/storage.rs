//! Namespaced storage facade for Runtime and plugin state.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use async_trait::async_trait;
use serde_json::Value;
use sqlx::SqlitePool;

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

    fn parse(value: &str) -> Result<Self, KernelError> {
        if value == "runtime" || value == "sessions" || value == "trajectories" {
            return Ok(Self(value.to_string()));
        }
        let Some(plugin_id) = value.strip_prefix("plugins/") else {
            return Err(KernelError::new(
                tact_protocol::ErrorCategory::InvalidRequest,
                "unknown storage namespace",
                "storage",
                false,
            ));
        };
        Self::plugin(plugin_id)
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
        self.get(&StorageNamespace::parse(namespace)?, key)
    }

    async fn set(&self, namespace: &str, key: &str, value: Value) -> Result<(), KernelError> {
        self.set(StorageNamespace::parse(namespace)?, key.to_string(), value)
    }
}

/// SQLite-backed implementation used by Runtime sessions. It uses a separate
/// table in the existing database and never alters domain tables owned by the
/// session, task, or trajectory stores.
#[derive(Clone)]
pub struct SqliteStorageService {
    pool: SqlitePool,
}

impl SqliteStorageService {
    pub async fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let pool = crate::sqlite::open_pool(path).await?;
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS runtime_storage (
                namespace TEXT NOT NULL,
                key TEXT NOT NULL,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL,
                PRIMARY KEY (namespace, key)
            )",
        )
        .execute(&*pool)
        .await
        .context("create runtime_storage table")?;
        Ok(Self {
            pool: (*pool).clone(),
        })
    }
}

#[async_trait]
impl StorageService for SqliteStorageService {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>, KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        let value: Option<String> =
            sqlx::query_scalar("SELECT value FROM runtime_storage WHERE namespace = ? AND key = ?")
                .bind(namespace.as_str())
                .bind(key)
                .fetch_optional(&self.pool)
                .await
                .map_err(|error| KernelError::storage(error.to_string()))?;
        value
            .map(|value| {
                serde_json::from_str(&value)
                    .map_err(|error| KernelError::storage(error.to_string()))
            })
            .transpose()
    }

    async fn set(&self, namespace: &str, key: &str, value: Value) -> Result<(), KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        let encoded = serde_json::to_string(&value)
            .map_err(|error| KernelError::storage(error.to_string()))?;
        sqlx::query(
            "INSERT INTO runtime_storage (namespace, key, value, updated_at)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(namespace, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
        )
        .bind(namespace.as_str())
        .bind(key)
        .bind(encoded)
        .bind(crate::sqlite::now_millis())
        .execute(&self.pool)
        .await
        .map_err(|error| KernelError::storage(error.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn sqlite_storage_round_trips_namespaced_values() {
        let directory = tempfile::tempdir().expect("temp directory");
        let storage = SqliteStorageService::open(&directory.path().join("tact.db"))
            .await
            .expect("open storage");
        storage
            .set(
                "plugins/example",
                "state",
                serde_json::json!({"ready": true}),
            )
            .await
            .expect("set value");
        assert_eq!(
            storage
                .get("plugins/example", "state")
                .await
                .expect("get value"),
            Some(serde_json::json!({"ready": true}))
        );
    }

    #[tokio::test]
    async fn storage_rejects_unknown_namespaces() {
        let storage = StorageServiceImpl::default();
        let error = StorageService::set(&storage, "other", "key", serde_json::json!(null))
            .await
            .expect_err("unknown namespace must fail");
        assert_eq!(
            error.category(),
            tact_protocol::ErrorCategory::InvalidRequest
        );
    }
}
