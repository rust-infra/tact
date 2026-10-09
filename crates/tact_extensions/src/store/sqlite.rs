//! Shared SQLite pool, re-exported from the Runtime Kernel.
//!
//! The pool belongs to the Kernel because the Kernel's minimal storage and the
//! trajectory recorder also open the same database; every domain store in this
//! crate shares that one registry through this path.

pub use tact::sqlite::*;

#[cfg(test)]
mod tests {
    use crate::store::session_store::SqliteSessionStore;
    use crate::store::task_store::SqliteTaskStore;
    use crate::store::team_store::SqliteTeamStore;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("tact-pool-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Every domain store on one database shares one pool, and the pool is
    /// released once the last store drops.
    #[tokio::test]
    async fn domain_stores_share_one_pool() {
        let dir = temp_dir("domain_stores");
        let db = dir.join("tact.db");
        {
            let _task = SqliteTaskStore::new(&db).await.unwrap();
            let _team = SqliteTeamStore::new(&db).await.unwrap();
            let _session = SqliteSessionStore::new(&db).await.unwrap();
            assert_eq!(tact::sqlite::pool_refcount_under(&dir), 1);
        }
        assert_eq!(tact::sqlite::pool_refcount_under(&dir), 0);
    }
}
