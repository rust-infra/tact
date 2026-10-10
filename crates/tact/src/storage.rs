//! Namespaced storage facade for Runtime and plugin state.

use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};

use anyhow::Context;
use async_trait::async_trait;
use serde_json::Value;
use sqlx::SqlitePool;

use super::KernelError;

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

    pub fn parse(value: &str) -> Result<Self, KernelError> {
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

    /// The plugin that owns this namespace, for `plugins/<id>`.
    #[must_use]
    pub fn plugin_owner(&self) -> Option<&str> {
        self.0.strip_prefix("plugins/")
    }

    /// Whether this is a Runtime-owned namespace (`runtime` / `sessions` /
    /// `trajectories`) rather than a plugin one.
    #[must_use]
    pub fn is_runtime_owned(&self) -> bool {
        matches!(self.0.as_str(), "runtime" | "sessions" | "trajectories")
    }
}

/// One write applied inside a [`StorageService::transaction`].
///
/// A transaction takes a list rather than a closure so the same shape can be
/// serialized across a plugin host boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum StorageOperation {
    Set { key: String, value: Value },
    Delete { key: String },
}

impl StorageOperation {
    pub fn set(key: impl Into<String>, value: Value) -> Self {
        Self::Set {
            key: key.into(),
            value,
        }
    }

    pub fn delete(key: impl Into<String>) -> Self {
        Self::Delete { key: key.into() }
    }
}

/// The error a backend returns for an operation it does not implement.
fn storage_unavailable(operation: &str) -> KernelError {
    KernelError::new(
        tact_protocol::ErrorCategory::StorageError,
        format!("storage {operation} is not available"),
        "storage",
        false,
    )
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

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), KernelError> {
        StorageServiceImpl::delete(self, &StorageNamespace::parse(namespace)?, key)
    }

    async fn list(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<Vec<(String, Value)>, KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        let values = self
            .values
            .read()
            .map_err(|_| StorageServiceImpl::poisoned())?;
        Ok(values
            .iter()
            .filter(|((ns, key), _)| ns == &namespace && key.starts_with(prefix))
            .map(|((_, key), value)| (key.clone(), value.clone()))
            .collect())
    }

    async fn transaction(
        &self,
        namespace: &str,
        operations: Vec<StorageOperation>,
    ) -> Result<(), KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        let mut values = self
            .values
            .write()
            .map_err(|_| StorageServiceImpl::poisoned())?;
        for operation in operations {
            match operation {
                StorageOperation::Set { key, value } => {
                    values.insert((namespace.clone(), key), value);
                }
                StorageOperation::Delete { key } => {
                    values.remove(&(namespace.clone(), key));
                }
            }
        }
        Ok(())
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

    async fn delete(&self, namespace: &str, key: &str) -> Result<(), KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        sqlx::query("DELETE FROM runtime_storage WHERE namespace = ? AND key = ?")
            .bind(namespace.as_str())
            .bind(key)
            .execute(&self.pool)
            .await
            .map_err(|error| KernelError::storage(error.to_string()))?;
        Ok(())
    }

    async fn list(
        &self,
        namespace: &str,
        prefix: &str,
    ) -> Result<Vec<(String, Value)>, KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        // `LIKE`'s wildcards are escaped so a key is matched literally.
        let pattern = format!(
            "{}%",
            prefix
                .replace('\\', "\\\\")
                .replace('%', "\\%")
                .replace('_', "\\_")
        );
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT key, value FROM runtime_storage
             WHERE namespace = ? AND key LIKE ? ESCAPE '\\'
             ORDER BY key",
        )
        .bind(namespace.as_str())
        .bind(pattern)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| KernelError::storage(error.to_string()))?;
        rows.into_iter()
            .map(|(key, value)| {
                serde_json::from_str(&value)
                    .map(|value| (key, value))
                    .map_err(|error| KernelError::storage(error.to_string()))
            })
            .collect()
    }

    async fn transaction(
        &self,
        namespace: &str,
        operations: Vec<StorageOperation>,
    ) -> Result<(), KernelError> {
        let namespace = StorageNamespace::parse(namespace)?;
        let mut transaction = self
            .pool
            .begin()
            .await
            .map_err(|error| KernelError::storage(error.to_string()))?;
        for operation in operations {
            match operation {
                StorageOperation::Set { key, value } => {
                    let encoded = serde_json::to_string(&value)
                        .map_err(|error| KernelError::storage(error.to_string()))?;
                    sqlx::query(
                        "INSERT INTO runtime_storage (namespace, key, value, updated_at)
                         VALUES (?, ?, ?, ?)
                         ON CONFLICT(namespace, key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
                    )
                    .bind(namespace.as_str())
                    .bind(&key)
                    .bind(encoded)
                    .bind(crate::sqlite::now_millis())
                    .execute(&mut *transaction)
                    .await
                    .map_err(|error| KernelError::storage(error.to_string()))?;
                }
                StorageOperation::Delete { key } => {
                    sqlx::query("DELETE FROM runtime_storage WHERE namespace = ? AND key = ?")
                        .bind(namespace.as_str())
                        .bind(&key)
                        .execute(&mut *transaction)
                        .await
                        .map_err(|error| KernelError::storage(error.to_string()))?;
                }
            }
        }
        transaction
            .commit()
            .await
            .map_err(|error| KernelError::storage(error.to_string()))?;
        Ok(())
    }
}
/// Namespaced storage boundary supplied by a Runtime host.
#[async_trait::async_trait]
pub trait StorageService: Send + Sync {
    async fn get(&self, namespace: &str, key: &str) -> Result<Option<Value>, KernelError>;

    async fn set(&self, namespace: &str, key: &str, value: Value) -> Result<(), KernelError>;

    /// Removes `key` from `namespace`. Deleting an absent key is a no-op.
    ///
    /// A backend that only offers read/write reports `StorageError` here rather
    /// than pretending the delete happened.
    async fn delete(&self, _namespace: &str, _key: &str) -> Result<(), KernelError> {
        Err(storage_unavailable("delete"))
    }

    /// Every `(key, value)` in `namespace` whose key starts with `prefix`,
    /// ordered by key. Pass `""` for the whole namespace.
    async fn list(
        &self,
        _namespace: &str,
        _prefix: &str,
    ) -> Result<Vec<(String, Value)>, KernelError> {
        Err(storage_unavailable("list"))
    }

    /// Applies `operations` atomically inside one namespace: either every
    /// operation lands or none does.
    async fn transaction(
        &self,
        _namespace: &str,
        _operations: Vec<StorageOperation>,
    ) -> Result<(), KernelError> {
        Err(storage_unavailable("transaction"))
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

    #[tokio::test]
    async fn in_memory_storage_lists_deletes_and_transacts_in_one_namespace() {
        let storage = StorageServiceImpl::default();
        StorageService::set(&storage, "plugins/example", "a", serde_json::json!(1))
            .await
            .expect("set a");
        StorageService::set(&storage, "plugins/example", "b", serde_json::json!(2))
            .await
            .expect("set b");
        StorageService::set(&storage, "plugins/other", "a", serde_json::json!(9))
            .await
            .expect("set other namespace");

        assert_eq!(
            StorageService::list(&storage, "plugins/example", "")
                .await
                .expect("list"),
            vec![
                ("a".to_string(), serde_json::json!(1)),
                ("b".to_string(), serde_json::json!(2)),
            ]
        );
        assert_eq!(
            StorageService::list(&storage, "plugins/example", "b")
                .await
                .expect("list prefix"),
            vec![("b".to_string(), serde_json::json!(2))]
        );

        StorageService::transaction(
            &storage,
            "plugins/example",
            vec![
                StorageOperation::set("b", serde_json::json!(20)),
                StorageOperation::delete("a"),
                StorageOperation::set("c", serde_json::json!(3)),
            ],
        )
        .await
        .expect("transaction");

        assert_eq!(
            StorageService::list(&storage, "plugins/example", "")
                .await
                .expect("list after transaction"),
            vec![
                ("b".to_string(), serde_json::json!(20)),
                ("c".to_string(), serde_json::json!(3)),
            ]
        );
        assert_eq!(
            StorageService::get(&storage, "plugins/other", "a")
                .await
                .expect("other namespace untouched"),
            Some(serde_json::json!(9))
        );

        StorageService::delete(&storage, "plugins/example", "b")
            .await
            .expect("delete");
        assert_eq!(
            StorageService::get(&storage, "plugins/example", "b")
                .await
                .expect("get deleted"),
            None
        );
    }

    #[tokio::test]
    async fn sqlite_storage_deletes_lists_and_transacts() {
        let directory = tempfile::tempdir().expect("temp directory");
        let path = directory.path().join("tact.db");
        let storage = SqliteStorageService::open(&path)
            .await
            .expect("open storage");

        storage
            .transaction(
                "plugins/example",
                vec![
                    StorageOperation::set("alpha", serde_json::json!(1)),
                    StorageOperation::set("beta", serde_json::json!(2)),
                    StorageOperation::set("gamma", serde_json::json!(3)),
                ],
            )
            .await
            .expect("transaction");

        let keys = |rows: Vec<(String, Value)>| -> Vec<String> {
            rows.into_iter().map(|(key, _)| key).collect()
        };
        assert_eq!(
            keys(storage.list("plugins/example", "").await.expect("list")),
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()]
        );
        assert_eq!(
            keys(
                storage
                    .list("plugins/example", "ga")
                    .await
                    .expect("list prefix")
            ),
            vec!["gamma".to_string()]
        );

        storage
            .delete("plugins/example", "beta")
            .await
            .expect("delete");
        assert_eq!(
            storage.get("plugins/example", "beta").await.expect("get"),
            None
        );

        // The writes survive a fresh handle, and an unused namespace stays empty.
        let reopened = SqliteStorageService::open(&path).await.expect("reopen");
        assert_eq!(
            reopened.get("plugins/example", "alpha").await.expect("get"),
            Some(serde_json::json!(1))
        );
        assert!(
            reopened
                .list("plugins/other", "")
                .await
                .expect("list other")
                .is_empty()
        );
    }

    struct ReadWriteOnly;

    #[async_trait::async_trait]
    impl StorageService for ReadWriteOnly {
        async fn get(&self, _namespace: &str, _key: &str) -> Result<Option<Value>, KernelError> {
            Ok(None)
        }

        async fn set(
            &self,
            _namespace: &str,
            _key: &str,
            _value: Value,
        ) -> Result<(), KernelError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_read_write_backend_reports_the_operations_it_lacks() {
        let storage = ReadWriteOnly;
        let errors = [
            storage.delete("plugins/x", "k").await.expect_err("delete"),
            storage.list("plugins/x", "").await.expect_err("list"),
            storage
                .transaction("plugins/x", Vec::new())
                .await
                .expect_err("transaction"),
        ];
        for error in errors {
            assert_eq!(error.category(), tact_protocol::ErrorCategory::StorageError);
        }
    }
}
