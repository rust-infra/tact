//! Session listing shared by both front ends.
//!
//! Resuming is id reuse: `Agent::ensure_session` reloads a session's stored
//! conversation when it is constructed with an id that already has messages, so
//! a front end only has to show the ids a workspace already owns and hand one
//! back to [`crate::SessionOptions::resume`]. Nothing here builds an agent, so a
//! session list is cheap enough to load before the first frame.

use std::path::Path;

use tact::consts::TactPath;

/// One row in a front end's session list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecentSession {
    /// Id the agent resumes.
    pub id: String,
    /// Last activity as a Unix timestamp in seconds.
    pub updated_at_unix: i64,
    /// Persisted message count, so an untouched session reads as empty rather
    /// than as missing.
    pub message_count: i64,
}

/// Recent sessions for `workdir`, newest first.
///
/// The store orders by `updated_at DESC`; this only reshapes the rows into a
/// presentation-neutral type. Sessions live in the workspace's own
/// `.tact/tact.db`, so a list is always scoped to one workspace.
pub fn recent(workdir: &Path) -> anyhow::Result<Vec<RecentSession>> {
    let tact_path = TactPath::new(workdir.to_path_buf());
    let root_dir = tact_path.workdir().display().to_string();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    runtime.block_on(async move {
        let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path()).await?;
        let sessions = store.list_sessions(Some(&root_dir)).await?;
        Ok(sessions
            .into_iter()
            .map(|session| RecentSession {
                id: session.id,
                updated_at_unix: session.updated_at.timestamp(),
                message_count: session.message_count,
            })
            .collect())
    })
}

/// First UUID segment, used for thread names and session labels.
pub fn short_id(session_id: &str) -> &str {
    session_id.split('-').next().unwrap_or(session_id)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn temp_workspace() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "tact_session_list_{}_{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).expect("temp workspace");
        dir
    }

    /// Seed a workspace's store, then drop the runtime so `recent` may build its
    /// own the way a front end calls it.
    fn seed(workspace: &Path, ids: &[&str]) {
        let tact_path = TactPath::new(workspace.to_path_buf());
        let root_dir = workspace.display().to_string();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                    .await
                    .expect("store");
                for id in ids {
                    store
                        .ensure_session_row(id, &root_dir, "")
                        .await
                        .expect("row");
                }
            });
    }

    #[test]
    fn recent_lists_only_this_workspaces_sessions() {
        let workspace = temp_workspace();
        let other = temp_workspace();
        seed(&workspace, &["11111111-aaaa"]);
        seed(&other, &["22222222-bbbb"]);

        let sessions = recent(&workspace).expect("recent sessions");

        assert_eq!(
            sessions.len(),
            1,
            "only the workspace's own session: {sessions:?}"
        );
        assert_eq!(sessions[0].id, "11111111-aaaa");
        assert_eq!(sessions[0].message_count, 0);
        assert!(sessions[0].updated_at_unix > 1_600_000_000);

        let _ = std::fs::remove_dir_all(&workspace);
        let _ = std::fs::remove_dir_all(&other);
    }

    #[test]
    fn short_id_trims_the_uuid() {
        assert_eq!(short_id("11111111-aaaa-bbbb"), "11111111");
        assert_eq!(short_id("plain"), "plain");
    }
}
