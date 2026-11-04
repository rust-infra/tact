use tact_desktop_core::{ArchiveFilter, ProjectId, SessionSummaryView};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionManagerView {
    pub project_filter: Option<String>,
    pub filter: ArchiveFilter,
    pub query: String,
}

impl Default for SessionManagerView {
    fn default() -> Self {
        Self {
            project_filter: None,
            filter: ArchiveFilter::Active,
            query: String::new(),
        }
    }
}

/// The session count for one project row.
///
/// Only the active project's sessions are loaded in the snapshot, so a count
/// for any other registered project would read as a false zero. The pane
/// shows a number for the loaded project and leaves the other rows bare.
pub fn project_session_count(
    project: &ProjectId,
    active: Option<&ProjectId>,
    sessions: &[SessionSummaryView],
) -> Option<usize> {
    (active == Some(project)).then(|| {
        sessions
            .iter()
            .filter(|session| &session.project_id == project)
            .count()
    })
}

pub fn filter_sessions<'a>(
    sessions: &'a [SessionSummaryView],
    view: &SessionManagerView,
) -> Vec<&'a SessionSummaryView> {
    let mut rows = sessions
        .iter()
        .filter(|session| match view.filter {
            ArchiveFilter::Active => session.archived_at.is_none(),
            ArchiveFilter::Archived => session.archived_at.is_some(),
            ArchiveFilter::All => true,
        })
        .filter(|session| {
            view.project_filter
                .as_ref()
                .is_none_or(|project| session.project_id.0 == *project)
        })
        .filter_map(|session| {
            if view.query.trim().is_empty() {
                return Some((0, session));
            }
            tact_catalog::fuzzy_score(&view.query, &session.title)
                .or_else(|| tact_catalog::fuzzy_score(&view.query, &session.id))
                .map(|score| (score, session))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    rows.into_iter().map(|(_, session)| session).collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use tact_desktop_core::ProjectId;

    use super::*;

    fn session(id: &str, title: &str, archived: bool) -> SessionSummaryView {
        session_in("p", id, title, archived)
    }

    fn session_in(project: &str, id: &str, title: &str, archived: bool) -> SessionSummaryView {
        SessionSummaryView {
            id: id.into(),
            project_id: ProjectId(project.into()),
            root_dir: "/tmp/p".into(),
            title: title.into(),
            updated_at: Utc.with_ymd_and_hms(2026, 10, 30, 0, 0, 0).unwrap(),
            archived_at: archived.then(Utc::now),
            locked: false,
            message_count: 1,
            search_score: 0,
        }
    }

    

    

    
}
