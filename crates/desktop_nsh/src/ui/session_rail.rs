use chrono::{DateTime, Utc};
use tact_catalog::CatalogLocale;
use tact_desktop_core::{ArchiveFilter, SessionListState, SessionSummaryView};

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

/// Short relative timestamp for rail rows ("2m ago", "Yesterday", "Oct 28").
pub fn relative_time(updated: DateTime<Utc>, now: DateTime<Utc>, locale: CatalogLocale) -> String {
    let seconds = now.signed_duration_since(updated).num_seconds().max(0);
    match seconds {
        0..=59 => match locale {
            CatalogLocale::En => "just now".into(),
            CatalogLocale::Zh => "刚刚".into(),
        },
        60..=3_599 => {
            let minutes = seconds / 60;
            match locale {
                CatalogLocale::En => format!("{minutes}m ago"),
                CatalogLocale::Zh => format!("{minutes} 分钟前"),
            }
        }
        3_600..=86_399 => {
            let hours = seconds / 3_600;
            match locale {
                CatalogLocale::En => format!("{hours}h ago"),
                CatalogLocale::Zh => format!("{hours} 小时前"),
            }
        }
        86_400..=172_799 => match locale {
            CatalogLocale::En => "Yesterday".into(),
            CatalogLocale::Zh => "昨天".into(),
        },
        172_800..=604_799 => {
            let days = seconds / 86_400;
            match locale {
                CatalogLocale::En => format!("{days}d ago"),
                CatalogLocale::Zh => format!("{days} 天前"),
            }
        }
        _ => match locale {
            CatalogLocale::En => updated.format("%b %-d").to_string(),
            CatalogLocale::Zh => updated.format("%m-%d").to_string(),
        },
    }
}

/// Second line of a session row: when it last moved and how much is in it.
pub fn session_detail(
    session: &SessionSummaryView,
    now: DateTime<Utc>,
    locale: CatalogLocale,
) -> String {
    let time = relative_time(session.updated_at, now, locale);
    match locale {
        CatalogLocale::En => format!("{time} · {} messages", session.message_count),
        CatalogLocale::Zh => format!("{time} · {} 条", session.message_count),
    }
}


