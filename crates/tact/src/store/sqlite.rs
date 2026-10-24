//! Shared SQLite pool for the domain stores.
//!
//! All domain stores (sessions, tasks, background, team, worktrees)
//! live in the same `<workdir>/.tact/tact.db`. This module owns the
//! open-or-create + busy-timeout boilerplate and caches **one**
//! `SqlitePool` per database file, shared by every store in the program.
//!
//! Pools are reference-counted: the registry drops a pool when the last
//! store holding it is dropped, so long test runs that create many
//! temporary databases do not leak file descriptors.
//!
//! # Transactions that write must say so at `BEGIN`
//!
//! `Pool::begin` issues a bare `BEGIN`, which SQLite treats as *deferred*: it
//! takes no lock, and the transaction's first statement decides what kind it
//! becomes. That is fine for a transaction whose first statement is the write,
//! and wrong for one that reads first — which is most of them here.
//!
//! The read opens a snapshot. When the write half then asks for the write
//! lock, SQLite compares the WAL header against that snapshot and refuses if
//! anything was committed in between (`SQLITE_BUSY_SNAPSHOT`), because writing
//! would fork the log. It also does not wait: the retry loop in
//! `btreeBeginTrans` only consults the busy handler while the connection has no
//! transaction open, and a read snapshot is one. The result is a failure in
//! microseconds on a connection configured with a five-second `busy_timeout`.
//!
//! So a transaction that reads before it writes opens with [`begin_write`],
//! which asks for the write lock at `BEGIN`, before any snapshot exists, where
//! the busy handler does run — a concurrent writer is waited out instead of
//! failing. Pure-read transactions keep using `begin()`; they must not queue
//! behind writers.
//!
//! # `busy_timeout` is not the whole story
//!
//! A single statement in autocommit mode queues for the write lock and waits
//! out `busy_timeout` (5s) on its own, so those are not retried here. Two
//! other places lose the race without ever waiting:
//!
//! - **Switching a database into WAL** needs an exclusive lock, and SQLite
//!   returns `SQLITE_BUSY` immediately rather than consulting the busy handler.
//!   Setting `journal_mode` on every pooled connection made this a per-connection
//!   hazard: a database in rollback-journal mode would fail to hand out a
//!   connection whenever anyone was writing. [`open_pool`] therefore switches
//!   the journal mode **only for a database that has no schema yet**, and
//!   leaves an initialized one in whatever mode it already has.
//! - **Writer admission** (`BEGIN IMMEDIATE`) does wait, but only for
//!   `busy_timeout`; a longer batch write in another process still surfaces as
//!   `SQLITE_BUSY`. [`begin_write`] retries those a couple of times, as does
//!   opening the pool.

use std::collections::HashMap;
use std::future::Future;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use crate::utils::LockExt;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous};
use sqlx::{Connection, SqliteConnection, SqlitePool, Transaction};

/// How many times a statement that lost the writer slot is retried, and how
/// long to wait before each attempt. Two retries add at most 300ms on top of
/// `busy_timeout`; past that the contention is a stall, not a race, and
/// failing loudly beats hiding it.
const BUSY_RETRIES: [Duration; 2] = [Duration::from_millis(100), Duration::from_millis(200)];

/// How long any connection waits for another process's write lock.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// Now, in the milliseconds every domain store persists time in.
///
/// The stores share one database and, in places, one row's timestamp — a store
/// that encoded time differently would be writable but unreadable by the
/// others, so the unit is stated once, here.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// A persisted millisecond timestamp as a `DateTime`.
///
/// This is also the one place that decides what an *unrepresentable* value
/// means: `Utc::now()` rather than an error, because the column is a display
/// field and a row whose timestamp cannot be read is still a row the panel has
/// to show. Three stores read timestamps back, so the alternative was three
/// answers to that question.
pub fn from_millis(millis: i64) -> DateTime<Utc> {
    DateTime::from_timestamp_millis(millis).unwrap_or_else(Utc::now)
}

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
        let mut pools = POOLS.lock_recover();
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
/// Creates the file (and its parent directory) when missing, puts a brand-new
/// database into WAL, and sets a 5s busy timeout so concurrent writers from
/// other processes wait instead of failing with SQLITE_BUSY.
pub async fn open_pool(path: &Path) -> Result<PoolRef> {
    let key = std::path::absolute(path)
        .with_context(|| format!("failed to resolve absolute path for {}", path.display()))?;

    if let Some(pool) = take_handle(&key) {
        return Ok(pool);
    }

    let pool = open_new_pool(path).await?;

    // If two callers raced on a fresh path, both open a pool; the first to
    // insert wins and the loser's pool is dropped (connections close).
    let mut pools = POOLS.lock_recover();
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
    let mut pools = POOLS.lock_recover();
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

    // WAL lets readers (the TUI reading session history) proceed while a
    // writer (the agent persisting a message) holds the lock, which matters
    // because every domain store here shares one database file.
    //
    // Switching a database *into* WAL needs an exclusive lock, and SQLite
    // answers SQLITE_BUSY *immediately* there — the busy handler is not
    // consulted, so `busy_timeout` cannot ride it out. Setting the mode on
    // every pooled connection made that a per-connection hazard: a database in
    // rollback-journal mode failed to hand out a connection whenever anyone
    // was writing. So the switch happens once, here, on one connection, and
    // only for a database with no schema yet. An initialized database keeps
    // whatever mode it already has — every database tact created is WAL, and
    // converting a live one under another process buys a stall, not safety.
    let journal = match prepare_journal_mode(path).await {
        Ok(mode) => mode,
        Err(error) => {
            tracing::warn!(
                error = %error,
                db = %path.display(),
                "could not prepare the sqlite journal mode; opening it as-is"
            );
            SqliteJournalMode::Delete
        }
    };
    if journal != SqliteJournalMode::Wal {
        tracing::warn!(
            db = %path.display(),
            mode = ?journal,
            "sqlite database is not in WAL mode; readers will block on writers"
        );
    }

    retry_busy(|| connect_with_pragmas(path, journal))
        .await
        .with_context(|| format!("failed to open sqlite database at {}", path.display()))
}

/// Puts a brand-new database into WAL and reports the mode it will be opened
/// in.
///
/// One connection does both jobs: it asks whether the database has a schema,
/// switches it when it does not, and reads back the mode that took effect. A
/// database left behind by an interrupted first initialization is empty and
/// therefore treated as new — which is what its owner wants.
async fn prepare_journal_mode(path: &Path) -> Result<SqliteJournalMode> {
    let url = format!("sqlite:{}", path.display());
    let mut conn = retry_busy(|| SqliteConnection::connect(&url))
        .await
        .with_context(|| format!("failed to inspect sqlite database at {}", path.display()))?;

    let tables: i64 = sqlx::query_scalar("SELECT count(*) FROM sqlite_master")
        .fetch_one(&mut conn)
        .await
        .context("failed to read the sqlite schema")?;
    if tables == 0 {
        // Losing this to a concurrent opener means "stay in the mode you have",
        // not "refuse to start"; the read-back below reports which one won.
        let _ = sqlx::query("PRAGMA journal_mode = WAL")
            .fetch_one(&mut conn)
            .await;
    }
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut conn)
        .await
        .context("failed to read the sqlite journal mode")?;

    let _ = conn.close().await;
    Ok(SqliteJournalMode::from_str(&mode)?)
}

/// Opens a pool in the journal mode the database is already in.
///
/// `journal_mode` is deliberately **not** set here: it is persistent for WAL
/// and switching needs an exclusive lock, so setting it per connection is how
/// a pool stalls (or fails) under concurrency. Only `synchronous`, which is
/// per-connection, is decided here.
async fn connect_with_pragmas(
    path: &Path,
    journal: SqliteJournalMode,
) -> std::result::Result<SqlitePool, sqlx::Error> {
    // `synchronous = NORMAL` is corruption-safe in WAL mode; the default FULL
    // would fsync the WAL on every commit for no extra safety. Under a
    // rollback journal NORMAL *can* corrupt on power loss, so keep FULL there.
    let synchronous = if journal == SqliteJournalMode::Wal {
        SqliteSynchronous::Normal
    } else {
        SqliteSynchronous::Full
    };
    let url = format!("sqlite:{}", path.display());
    let options = SqliteConnectOptions::from_str(&url)?
        .synchronous(synchronous)
        // Wait for a concurrent writer (cross-process access to the same
        // workdir) instead of failing with SQLITE_BUSY immediately.
        .busy_timeout(BUSY_TIMEOUT);

    SqlitePool::connect_with(options).await
}

/// Begins a transaction that intends to write, retrying transient contention.
///
/// `BEGIN IMMEDIATE` takes the write lock at `BEGIN`, where the busy handler
/// runs; the retries cover the case where another process holds that lock for
/// longer than `busy_timeout`. Prefer this over `pool.begin()` for anything
/// that reads before it writes (see the module docs).
pub async fn begin_write(pool: &SqlitePool) -> Result<Transaction<'static, sqlx::Sqlite>> {
    retry_busy(|| pool.begin_with("BEGIN IMMEDIATE"))
        .await
        .context("failed to begin write transaction")
}

/// Runs `op`, retrying while SQLite reports the writer slot as busy.
///
/// Only `SQLITE_BUSY` / `SQLITE_LOCKED` (and the snapshot variant) are retried:
/// they are the codes that mean "try again", and a closure re-run is safe
/// precisely because the failed attempt changed nothing.
pub async fn retry_busy<T, F, Fut>(mut op: F) -> Result<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = std::result::Result<T, sqlx::Error>>,
{
    let mut attempt = 0;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err(error) if attempt < BUSY_RETRIES.len() && is_busy(&error) => {
                tokio::time::sleep(BUSY_RETRIES[attempt]).await;
                attempt += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// Whether `error` is SQLite saying another writer holds the lock.
///
/// Matched on the numeric code when there is one: `SQLITE_BUSY` is 5,
/// `SQLITE_LOCKED` is 6, and their extended variants (`SQLITE_BUSY_SNAPSHOT`
/// is 517) share the low byte. The message check is the fallback for errors
/// that arrive without a code.
fn is_busy(error: &sqlx::Error) -> bool {
    let Some(db) = error.as_database_error() else {
        return false;
    };
    if let Some(code) = db.code()
        && let Ok(code) = code.parse::<i32>()
    {
        return matches!(code & 0xff, 5 | 6);
    }
    let message = db.message();
    message.contains("database is locked") || message.contains("database table is locked")
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

    /// The TUI reads session history while the agent writes it; without WAL
    /// those lock each other out.
    #[tokio::test]
    async fn opened_pools_use_wal() {
        let dir = temp_dir("wal");
        let pool = open_pool(&dir.join("tact.db")).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool.deref())
            .await
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    /// `synchronous` is per-connection, so it is set through the connection
    /// options rather than a one-shot `PRAGMA` that later connections miss.
    #[tokio::test]
    async fn wal_pools_downgrade_synchronous() {
        let dir = temp_dir("synchronous");
        let pool = open_pool(&dir.join("tact.db")).await.unwrap();
        // NORMAL == 1; the rollback-journal default would be FULL == 2.
        let level: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(pool.deref())
            .await
            .unwrap();
        assert_eq!(level, 1);
    }

    /// Opening a pool for an initialized database must not try to switch its
    /// journal mode: SQLite refuses that switch while anyone else holds a
    /// lock, and it refuses *immediately* rather than waiting out
    /// `busy_timeout`. This holds a write lock on a rollback-journal database
    /// and expects `open_pool` to hand out a usable pool anyway.
    #[tokio::test]
    async fn opening_a_locked_rollback_journal_database_succeeds() {
        let dir = temp_dir("locked_delete_journal");
        let db = dir.join("tact.db");
        std::fs::File::create(&db).unwrap();

        // An initialized, non-WAL database — what an older tact left behind.
        let mut setup = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        sqlx::query("CREATE TABLE sessions (id TEXT PRIMARY KEY)")
            .execute(&mut setup)
            .await
            .unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode = DELETE")
            .fetch_one(&mut setup)
            .await
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "delete");

        // Another process mid-write, holding the lock past `open_pool`'s call.
        let mut blocker = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut blocker)
            .await
            .unwrap();

        let opened = tokio::time::timeout(std::time::Duration::from_secs(3), open_pool(&db))
            .await
            .expect("open_pool must not block on the journal-mode switch");

        sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();

        let pool = opened.expect("open_pool must ride out a concurrent writer");
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool.deref())
            .await
            .unwrap();
        assert_eq!(
            mode.to_ascii_lowercase(),
            "delete",
            "an initialized database keeps the journal mode it already had"
        );
    }

    /// A brand-new database still gets WAL, which is what the TUI's
    /// concurrent reads depend on.
    #[tokio::test]
    async fn a_new_database_is_created_in_wal() {
        let dir = temp_dir("fresh_wal");
        let db = dir.join("tact.db");
        // The file exists but holds no schema, like one left by an interrupted
        // first initialization.
        std::fs::File::create(&db).unwrap();
        let pool = open_pool(&db).await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(pool.deref())
            .await
            .unwrap();
        assert_eq!(mode.to_ascii_lowercase(), "wal");
    }

    /// Writer admission retries instead of surfacing the first `SQLITE_BUSY`.
    ///
    /// The pool here has no `busy_timeout` at all, so the first attempt fails
    /// instantly and only the retry can succeed — which is the case the
    /// timeout alone cannot cover (another process holding the write lock for
    /// longer than it).
    #[tokio::test]
    async fn begin_write_retries_a_busy_writer() {
        let dir = temp_dir("begin_write_retry");
        let db = dir.join("tact.db");
        std::fs::File::create(&db).unwrap();
        let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db.display()))
            .unwrap()
            .busy_timeout(Duration::ZERO);
        let pool = SqlitePool::connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS t (x INTEGER)")
            .execute(&pool)
            .await
            .unwrap();

        let mut blocker = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut blocker)
            .await
            .unwrap();

        // Released inside the retry window: the first attempt fails, the one
        // after the 100ms backoff succeeds.
        let release = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();
        });

        let mut tx = begin_write(&pool)
            .await
            .expect("begin_write must retry the busy writer");
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        release.await.unwrap();
    }

    /// When the writer outlasts every retry, the error surfaces instead of
    /// hanging: a stall is not a race and the caller has to see it.
    #[tokio::test]
    async fn begin_write_gives_up_after_the_retries() {
        let dir = temp_dir("begin_write_gives_up");
        let db = dir.join("tact.db");
        std::fs::File::create(&db).unwrap();
        let options = SqliteConnectOptions::from_str(&format!("sqlite:{}", db.display()))
            .unwrap()
            .busy_timeout(Duration::ZERO);
        let pool = SqlitePool::connect_with(options).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS t (x INTEGER)")
            .execute(&pool)
            .await
            .unwrap();

        let mut blocker = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut blocker)
            .await
            .unwrap();

        let error = begin_write(&pool)
            .await
            .expect_err("the writer never released the lock");
        assert!(
            is_busy(error.downcast_ref::<sqlx::Error>().expect("sqlx error")),
            "the surfaced error is the busy one: {error}"
        );

        sqlx::query("ROLLBACK").execute(&mut blocker).await.unwrap();
    }

    /// Only busy/locked errors are retried: a constraint violation is the
    /// caller's problem and must surface on the first attempt.
    #[tokio::test]
    async fn retry_busy_does_not_retry_a_constraint_violation() {
        let dir = temp_dir("retry_non_busy");
        let db = dir.join("tact.db");
        let pool = open_pool(&db).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS t (x INTEGER PRIMARY KEY)")
            .execute(pool.deref())
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(pool.deref())
            .await
            .unwrap();

        let mut attempts = 0;
        let error = retry_busy(|| {
            attempts += 1;
            sqlx::query("INSERT INTO t VALUES (1)").execute(pool.deref())
        })
        .await
        .expect_err("the duplicate key must fail");
        assert_eq!(attempts, 1, "a constraint violation is not transient");
        assert!(!is_busy(
            error.downcast_ref::<sqlx::Error>().expect("sqlx error")
        ));
    }

    /// A real `SQLITE_BUSY` (no `busy_timeout` to wait it out) must be
    /// recognised, and a constraint violation must not.
    #[tokio::test]
    async fn busy_is_recognised_and_constraints_are_not() {
        let dir = temp_dir("busy_detection");
        let db = dir.join("tact.db");
        let pool = open_pool(&db).await.unwrap();
        sqlx::query("CREATE TABLE IF NOT EXISTS t (x INTEGER PRIMARY KEY)")
            .execute(pool.deref())
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (1)")
            .execute(pool.deref())
            .await
            .unwrap();

        let mut blocker = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut blocker)
            .await
            .unwrap();
        sqlx::query("INSERT INTO t VALUES (2)")
            .execute(&mut blocker)
            .await
            .unwrap();

        // A second connection with no patience: the write lock is taken.
        let mut impatient = SqliteConnection::connect(&format!("sqlite:{}", db.display()))
            .await
            .unwrap();
        sqlx::query("PRAGMA busy_timeout = 0")
            .execute(&mut impatient)
            .await
            .unwrap();
        let busy = sqlx::query("BEGIN IMMEDIATE")
            .execute(&mut impatient)
            .await
            .expect_err("the lock is held");
        assert!(is_busy(&busy), "SQLITE_BUSY must be recognised: {busy}");

        sqlx::query("COMMIT").execute(&mut blocker).await.unwrap();

        let duplicate = sqlx::query("INSERT INTO t VALUES (1)")
            .execute(pool.deref())
            .await
            .expect_err("the key exists");
        assert!(
            !is_busy(&duplicate),
            "a constraint violation is not transient: {duplicate}"
        );
    }

    #[test]
    fn a_persisted_timestamp_round_trips() {
        let millis = 1_700_000_000_000;
        assert_eq!(from_millis(millis).timestamp_millis(), millis);
        assert!(now_millis() > millis, "the clock is past 2023");
    }

    /// The one policy in this pair: a value that is not a representable instant
    /// becomes "now" rather than failing, because the column is a display field
    /// and a row nobody can date is still a row the panel has to show.
    #[test]
    fn an_unrepresentable_timestamp_becomes_now() {
        let before = Utc::now();
        let fallback = from_millis(i64::MAX);
        assert!(
            fallback >= before && fallback <= Utc::now(),
            "{fallback} is not between {before} and now"
        );
    }
}
