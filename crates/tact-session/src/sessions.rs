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
    /// One-line title for a session list, from the session's opening message.
    ///
    /// A session the user has not named has no other label to print but the raw
    /// id, so the user's own first words are the honest stand-in. `None` for a
    /// session that has no user message yet.
    pub title: Option<String>,
    /// The name the user gave the session, when they renamed it.
    ///
    /// Beats [`Self::title`] wherever a front end labels the session, and
    /// clearing it in the store is what restores the derived label.
    pub name: Option<String>,
    /// Whether the session is archived.
    ///
    /// Archiving is a policy flag, not a delete: the row keeps its messages and
    /// stays listable, so a front end can group it or badge it but must not drop
    /// it -- clearing the flag has to restore the session.
    pub archived: bool,
    /// Whether the session is pinned.
    ///
    /// Pinned rows sort ahead of the rest and keep their own newest-first order
    /// among themselves. Like archiving, pinning changes no other property, so
    /// unpinning is the whole undo.
    pub pinned: bool,
}

/// Longest title a session list shows before eliding.
const TITLE_LIMIT: usize = 60;

/// Collapse a session's opening message into a one-line title.
///
/// Messages are typed as prose, so a title has to survive newlines and runs of
/// indentation without becoming a second paragraph: whitespace collapses to
/// single spaces and the result is cut at [`TITLE_LIMIT`] characters.
pub fn session_title(text: &str) -> Option<String> {
    let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return None;
    }
    let mut title: String = collapsed.chars().take(TITLE_LIMIT).collect();
    if collapsed.chars().count() > TITLE_LIMIT {
        title.push('…');
    }
    Some(title)
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
        let mut sessions: Vec<RecentSession> = sessions
            .into_iter()
            .map(|session| RecentSession {
                id: session.id,
                updated_at_unix: session.updated_at.timestamp(),
                message_count: session.message_count,
                title: session.first_user_text.as_deref().and_then(session_title),
                name: session.title,
                archived: session.archived_at.is_some(),
                pinned: session.pinned_at.is_some(),
            })
            .collect();
        // The store orders by `updated_at` alone. Pinning is a list policy, so
        // the split happens here rather than in SQL: a stable partition keeps
        // each group's own newest-first order without a second sort key the
        // store would have to know about.
        sessions.sort_by_key(|session| !session.pinned);
        Ok(sessions)
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

    /// Pinned rows sort ahead of newer unpinned ones.
    ///
    /// The store orders by `updated_at` alone, so this is the presentation
    /// policy: a stable partition, not a second sort key the store would have
    /// to learn.
    #[test]
    fn recent_sorts_pinned_sessions_first() {
        let workspace = temp_workspace();
        seed(
            &workspace,
            &["11111111-aaaa", "22222222-bbbb", "33333333-cccc"],
        );
        let tact_path = TactPath::new(workspace.clone());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(async {
                let store = tact::store::open_sqlite_session_store(&tact_path.session_db_path())
                    .await
                    .expect("store");
                store.pin_session("22222222-bbbb", true).await.expect("pin");
            });

        let sessions = recent(&workspace).expect("recent sessions");
        assert_eq!(
            sessions[0].id, "22222222-bbbb",
            "the pinned row leads: {sessions:?}"
        );
        assert!(sessions[0].pinned);
        assert!(
            sessions[1..].iter().all(|row| !row.pinned),
            "the rest keep their unpinned order"
        );

        let _ = std::fs::remove_dir_all(&workspace);
    }

    #[test]
    fn short_id_trims_the_uuid() {
        assert_eq!(short_id("11111111-aaaa-bbbb"), "11111111");
        assert_eq!(short_id("plain"), "plain");
    }

    #[test]
    fn session_titles_collapse_whitespace_and_elide() {
        assert_eq!(
            session_title("  Build\n\tthe  desktop shell  "),
            Some("Build the desktop shell".to_string())
        );
        assert_eq!(
            session_title("   \n  "),
            None,
            "blank messages have no title"
        );

        let long = "a".repeat(TITLE_LIMIT + 5);
        let title = session_title(&long).expect("a long message still titles");
        assert_eq!(title.chars().count(), TITLE_LIMIT + 1, "cut plus ellipsis");
        assert!(title.ends_with('…'), "elision is visible: {title}");
    }
}
