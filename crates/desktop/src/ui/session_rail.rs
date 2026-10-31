use tact_desktop_core::{ArchiveFilter, SessionListState};

pub fn filter_label(filter: ArchiveFilter) -> &'static str {
    match filter {
        ArchiveFilter::Active => "Active",
        ArchiveFilter::Archived => "Archived",
        ArchiveFilter::All => "All",
    }
}

pub fn visible_session_count(state: &SessionListState) -> usize {
    state.items.len()
}

pub fn filtered_sessions(
    state: &SessionListState,
    filter: ArchiveFilter,
) -> impl Iterator<Item = &tact_desktop_core::SessionSummaryView> {
    state.items.iter().filter(move |session| match filter {
        ArchiveFilter::Active => session.archived_at.is_none(),
        ArchiveFilter::Archived => session.archived_at.is_some(),
        ArchiveFilter::All => true,
    })
}
