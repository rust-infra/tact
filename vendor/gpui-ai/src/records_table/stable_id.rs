//! Stable-ID position lookup for one accepted snapshot, and what it costs.
//!
//! Accepting a snapshot already hashes every identity to prove it unique. This
//! module makes that same pass answer "where is this ID now?" for every later
//! asker — anchor recovery, selection, scrolling, reorder displacement — and
//! counts the comparisons so the cost is measured rather than argued about.

use std::collections::{HashMap, HashSet};

use gpui::SharedString;

use super::{RecordColumn, RecordRow};

#[cfg(test)]
thread_local! {
    /// Stable-ID comparisons performed on this thread.
    ///
    /// Thread-local rather than global: the test harness runs each test on its
    /// own thread, and a shared counter would report another test's lookups.
    static STABLE_ID_VISITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records `visited` stable-ID comparisons.
///
/// Lookup cost is the whole point of the index below, so it is measured rather
/// than asserted in prose. Nothing outside tests compiles the counter.
#[inline]
fn note_stable_id_visits(visited: usize) {
    #[cfg(test)]
    STABLE_ID_VISITS.with(|visits| visits.set(visits.get().saturating_add(visited)));
    #[cfg(not(test))]
    let _ = visited;
}

/// Stable-ID comparisons since the last call, and resets the counter.
#[cfg(test)]
pub(super) fn take_stable_id_visits() -> usize {
    STABLE_ID_VISITS.with(|visits| visits.replace(0))
}

/// Where each stable ID sits in one accepted snapshot.
///
/// Anchor recovery, selection validation, scrolling, and reorder displacement
/// all ask the same question — "where is this ID now?" — and a snapshot answers
/// it once here instead of once per asker. Positions are only ever read
/// alongside the snapshot they were built from: `records` and `columns` each
/// have a single assignment site, and each rebuilds its index there.
///
/// Retained rather than built per acceptance because selection, activation,
/// and scroll commands arrive between snapshots, where a temporary map has
/// already been dropped.
#[derive(Debug, Default)]
pub(super) struct StableIdIndex {
    positions: HashMap<SharedString, usize>,
}

impl StableIdIndex {
    /// The position `id` holds in the indexed snapshot, if it holds one.
    ///
    /// Snapshots carrying duplicate IDs are rejected before they reach an
    /// index, so one ID answers with one position.
    pub(super) fn position(&self, id: &str) -> Option<usize> {
        note_stable_id_visits(1);
        self.positions.get(id).copied()
    }
}

/// Indexes a candidate record snapshot, or reports it malformed.
///
/// Accepting a snapshot already hashes every row and cell ID to prove the
/// identities are unique, so the index falls out of that same pass rather than
/// costing a second one. A malformed snapshot yields no index and no state
/// change, which is what makes rejection atomic.
pub(super) fn index_valid_rows(rows: &[RecordRow]) -> Option<StableIdIndex> {
    note_stable_id_visits(rows.len());
    let mut positions = HashMap::with_capacity(rows.len());
    for (row_ix, row) in rows.iter().enumerate() {
        let mut cell_ids = HashSet::with_capacity(row.cells.len());
        if !row
            .cells
            .iter()
            .all(|cell| cell_ids.insert(cell.column_id()))
            || positions.insert(row.id.clone(), row_ix).is_some()
        {
            return None;
        }
    }
    Some(StableIdIndex { positions })
}

/// Indexes a candidate column snapshot, or reports it malformed.
pub(super) fn index_valid_columns(columns: &[RecordColumn]) -> Option<StableIdIndex> {
    note_stable_id_visits(columns.len());
    let mut positions = HashMap::with_capacity(columns.len());
    for (col_ix, column) in columns.iter().enumerate() {
        if positions.insert(column.id.clone(), col_ix).is_some() {
            return None;
        }
    }
    Some(StableIdIndex { positions })
}
