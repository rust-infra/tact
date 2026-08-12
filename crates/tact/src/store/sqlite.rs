//! Shared SQLite pool for the domain stores.
//!
//! All domain stores (sessions, tasks, cron, background, team, worktrees)
//! live in the same `<workdir>/.tact/tact.db`. This module owns the
//! open-or-create + busy-timeout boilerplate and caches **one**
//! `SqlitePool` per database file, shared by every store in the program.
//!
//! Pools are reference-counted: the registry drops a pool when the last
//! store holding it is dropped, so long test runs that create many
//! temporary databases do not leak file descriptors.

use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// One shared pool per database file, keyed by absolute path, together
/// with the number of live [`PoolRef`] handles handed out for it.
static POOLS: LazyLock<Mutex<HashMap<PathBuf, (SqlitePool, usize)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// A borrowed handle to the shared pool for one database file.
///
/// Dereferences to the underlying [`SqlitePool`]. Releases the registry's
/// reference when dropped; the pool is closed once no store uses it.
pub struct PoolRef {
    pool: SqlitePool,
    key: PathBuf,
}

impl Deref for PoolRef {
    type Target = SqlitePool;

    fn deref(&self) -> &Self::Target {
        &self.pool
    }
}

impl Drop for PoolRef {
    fn drop(&mut self) {
        let mut pools = POOLS.lock().expect("tact sqlite pool lock poisoned");
        if let Some((_, refs)) = pools.get_mut(&self.key) {
            *refs -= 1;
            if *refs == 0 {
                pools.remove(&self.key);
            }
        }
    }
}

/// Returns the shared pool for `path`, opening it on first use.
///
/// Creates the file (and its parent directory) when missing and sets a 5s
/// busy timeout so concurrent writers from other processes wait instead of
/// failing with SQLITE_BUSY.
pub async fn open_pool(path: &Path) -> Result<PoolRef> {
    let key = std::path::absolute(path)
        .with_context(|| format!("failed to resolve absolute path for {}", path.display()))?;

    if let Some(pool) = take_handle(&key) {
        return Ok(pool);
    }

    let pool = open_new_pool(path).await?;

    // If two callers raced on a fresh path, both open a pool; the first to
    // insert wins and the loser's pool is dropped (connections close).
    let mut pools = POOLS.lock().expect("tact sqlite pool lock poisoned");
    Ok(match pools.entry(key.clone()) {
        std::collections::hash_map::Entry::Occupied(mut entry) => {
            let (pool, refs) = entry.get_mut();
            *refs += 1;
            PoolRef {
                pool: pool.clone(),
                key,
            }
        }
        std::collections::hash_map::Entry::Vacant(entry) => {
            entry.insert((pool.clone(), 1));
            PoolRef { pool, key }
        }
    })
}

/// Returns a handle for `key` and bumps its refcount, or `None` when the
/// pool is not cached yet.
fn take_handle(key: &Path) -> Option<PoolRef> {
    let mut pools = POOLS.lock().expect("tact sqlite pool lock poisoned");
    let (pool, refs) = pools.get_mut(key)?;
    *refs += 1;
    Some(PoolRef {
        pool: pool.clone(),
        key: key.to_path_buf(),
    })
}

async fn open_new_pool(path: &Path) -> Result<SqlitePool> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create database directory")?;
    }
    // sqlx may fail to open a non-existent database file in some environments;
    // create an empty file first to ensure it's present.
    if let Err(e) = tokio::fs::metadata(path).await
        && e.kind() == std::io::ErrorKind::NotFound
    {
        tokio::fs::File::create(path)
            .await
            .context("failed to create database file")?;
    }
    let url = format!("sqlite:{}", path.display());
    let pool = SqlitePool::connect(&url)
        .await
        .with_context(|| format!("failed to open sqlite database at {}", path.display()))?;

    // Wait up to 5s for a concurrent writer (cross-process access to the
    // same workdir) instead of failing with SQLITE_BUSY immediately.
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(&pool)
        .await
        .context("failed to set busy_timeout")?;
    Ok(pool)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::store::session_store::SqliteSessionStore;
    use crate::store::task_store::SqliteTaskStore;
    use crate::store::team_store::SqliteTeamStore;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-pool-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Pools cached for databases under `dir`. Other tests run in parallel
    /// and open pools for their own temp dirs, so the global map is only
    /// inspected through this per-directory view.
    fn pool_count_under(dir: &Path) -> usize {
        let dir = std::path::absolute(dir).unwrap();
        POOLS
            .lock()
            .unwrap()
            .keys()
            .filter(|key| key.starts_with(&dir))
            .count()
    }

    #[tokio::test]
    async fn same_path_returns_the_same_pool() {
        let dir = temp_dir("same_path");
        let db = dir.join("tact.db");
        let a = open_pool(&db).await.unwrap();
        let b = open_pool(&db).await.unwrap();
        let c = open_pool(&db).await.unwrap();
        assert_eq!(pool_count_under(&dir), 1);
        drop(a);
        drop(b);
        drop(c);
        assert_eq!(pool_count_under(&dir), 0);
    }

    #[tokio::test]
    async fn different_paths_get_their_own_pool() {
        let dir = temp_dir("different_paths");
        let a = open_pool(&dir.join("a.db")).await.unwrap();
        let b = open_pool(&dir.join("b.db")).await.unwrap();
        assert_eq!(pool_count_under(&dir), 2);
        drop(a);
        drop(b);
        assert_eq!(pool_count_under(&dir), 0);
    }

    #[tokio::test]
    async fn domain_stores_share_one_pool() {
        let dir = temp_dir("domain_stores");
        let db = dir.join("tact.db");
        {
            let _task = SqliteTaskStore::new(&db).await.unwrap();
            let _team = SqliteTeamStore::new(&db).await.unwrap();
            let _session = SqliteSessionStore::new(&db).await.unwrap();
            assert_eq!(pool_count_under(&dir), 1);
        }
        assert_eq!(pool_count_under(&dir), 0);
    }
}
