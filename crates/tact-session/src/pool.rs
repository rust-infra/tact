use std::collections::HashMap;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};

static POOLS: LazyLock<Mutex<HashMap<PathBuf, (SqlitePool, usize)>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

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
        let Ok(mut pools) = POOLS.lock() else {
            return;
        };
        if let Some((_, refs)) = pools.get_mut(&self.key) {
            *refs = refs.saturating_sub(1);
            if *refs == 0 {
                pools.remove(&self.key);
            }
        }
    }
}

pub async fn open_pool(path: &Path) -> Result<PoolRef> {
    let key = std::path::absolute(path)
        .with_context(|| format!("failed to resolve absolute path for {}", path.display()))?;
    if let Some(pool) = take_handle(&key) {
        return Ok(pool);
    }

    let pool = open_new_pool(path).await?;
    let mut pools = POOLS
        .lock()
        .map_err(|_| anyhow::anyhow!("sqlite pool registry lock poisoned"))?;
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

#[doc(hidden)]
pub fn pool_count_under(dir: &Path) -> usize {
    let Ok(dir) = std::path::absolute(dir) else {
        return 0;
    };
    POOLS
        .lock()
        .map(|pools| pools.keys().filter(|key| key.starts_with(&dir)).count())
        .unwrap_or(0)
}

fn take_handle(key: &Path) -> Option<PoolRef> {
    let mut pools = POOLS.lock().ok()?;
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
    if let Err(error) = tokio::fs::metadata(path).await
        && error.kind() == std::io::ErrorKind::NotFound
    {
        tokio::fs::File::create(path)
            .await
            .context("failed to create database file")?;
    }
    match connect_with_pragmas(path, SqliteJournalMode::Wal).await {
        Ok(pool) => Ok(pool),
        Err(error) => {
            tracing::warn!(
                error = %error,
                db = %path.display(),
                "sqlite rejected WAL mode; falling back to the default journal"
            );
            connect_with_pragmas(path, SqliteJournalMode::Delete).await
        }
    }
}

async fn connect_with_pragmas(path: &Path, journal: SqliteJournalMode) -> Result<SqlitePool> {
    let synchronous = if journal == SqliteJournalMode::Wal {
        SqliteSynchronous::Normal
    } else {
        SqliteSynchronous::Full
    };
    let url = format!("sqlite:{}", path.display());
    let options = SqliteConnectOptions::from_str(&url)
        .with_context(|| format!("invalid sqlite url for {}", path.display()))?
        .journal_mode(journal)
        .synchronous(synchronous)
        .busy_timeout(std::time::Duration::from_secs(5));
    SqlitePool::connect_with(options)
        .await
        .with_context(|| format!("failed to open sqlite database at {}", path.display()))
}
