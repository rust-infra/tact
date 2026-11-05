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

/// The number of loaded sessions belonging to one project.
pub fn project_session_count(project: &ProjectId, sessions: &[SessionSummaryView]) -> usize {
    sessions
        .iter()
        .filter(|session| &session.project_id == project)
        .count()
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
    // The manager paints a project heading on every project change. Keep a
    // project's rows contiguous after the global score/recency sort so one
    // project never produces two headings.
    let mut grouped: Vec<(ProjectId, Vec<&SessionSummaryView>)> = Vec::new();
    for (_, session) in rows {
        if let Some((_, sessions)) = grouped
            .iter_mut()
            .find(|(project_id, _)| project_id == &session.project_id)
        {
            sessions.push(session);
        } else {
            grouped.push((session.project_id.clone(), vec![session]));
        }
    }
    grouped
        .into_iter()
        .flat_map(|(_, sessions)| sessions)
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone as _, Utc};
    use gpui::TestAppContext;
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

    fn session_at(project: &str, id: &str, title: &str, minutes_ago: i64) -> SessionSummaryView {
        let mut session = session_in(project, id, title, false);
        session.updated_at = Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap()
            - chrono::Duration::minutes(minutes_ago);
        session
    }

    #[gpui::test]
    fn session_manager_filters_active_and_archived(_cx: &mut TestAppContext) {
        let rows = vec![
            session("a", "Active", false),
            session("b", "Archived", true),
        ];
        let mut view = SessionManagerView::default();
        assert_eq!(filter_sessions(&rows, &view).len(), 1);
        view.filter = ArchiveFilter::Archived;
        assert_eq!(filter_sessions(&rows, &view)[0].id, "b");
        view.filter = ArchiveFilter::All;
        assert_eq!(filter_sessions(&rows, &view).len(), 2);
    }

    #[gpui::test]
    fn session_manager_fuzzy_search_finds_abbreviations(_cx: &mut TestAppContext) {
        let rows = vec![
            session("a", "Fix authentication retry", false),
            session("b", "Update dashboard", false),
        ];
        let view = SessionManagerView {
            query: "auth".into(),
            ..SessionManagerView::default()
        };
        assert_eq!(filter_sessions(&rows, &view)[0].id, "a");
    }

    #[gpui::test]
    fn project_counts_include_every_loaded_project(_cx: &mut TestAppContext) {
        let rows = vec![
            session_in("a", "a1", "Loaded one", false),
            session_in("a", "a2", "Loaded two", false),
            session_in("b", "b1", "Other project", false),
        ];
        assert_eq!(project_session_count(&ProjectId("a".into()), &rows), 2);
        assert_eq!(project_session_count(&ProjectId("b".into()), &rows), 1);
        assert_eq!(project_session_count(&ProjectId("c".into()), &rows), 0);
    }

    #[gpui::test]
    fn filtered_rows_keep_each_projects_group_contiguous(_cx: &mut TestAppContext) {
        let rows = vec![
            session_at("a", "a-new", "A new", 1),
            session_at("b", "b-new", "B new", 2),
            session_at("a", "a-old", "A old", 3),
            session_at("b", "b-old", "B old", 4),
        ];
        let filtered = filter_sessions(&rows, &SessionManagerView::default());
        let projects: Vec<&str> = filtered
            .iter()
            .map(|session| session.project_id.0.as_str())
            .collect();
        let ids: Vec<&str> = filtered.iter().map(|session| session.id.as_str()).collect();
        assert_eq!(projects, ["a", "a", "b", "b"]);
        assert_eq!(ids, ["a-new", "a-old", "b-new", "b-old"]);
    }
}
