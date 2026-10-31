//! Invalidation key for layout a window resolves rather than a snapshot.
//!
//! A virtualized list caches the height it measured for each row, and that
//! measurement is wrapped text laid out against the window's rem size. Nothing
//! in a content snapshot changes when the reader zooms, so without a key on the
//! resolved inputs a list keeps serving heights measured at the previous type
//! scale: rows overlap or leave gaps, and the scrollbar describes a document
//! that is no longer there.
//!
//! The key is value-only by design. It owns no list, notifies nothing, and
//! chooses no scroll anchor — anchoring is a per-surface policy, and a shared
//! owner would have to pick one for surfaces that legitimately differ.
//! Components read it from their own render, react in their own retained
//! owner, and call their own remeasurement path.

use gpui::Pixels;

/// The window-resolved inputs that cached layout depends on.
///
/// One value today; a future addition (a text-scale factor, a font stack)
/// belongs here rather than in a second key, so that one comparison still
/// answers "is what I measured still valid".
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct ResolvedLayoutKey {
    rem_size: Option<Pixels>,
}

impl ResolvedLayoutKey {
    /// Whether `rem_size` is already the recorded value.
    ///
    /// Mutates nothing, so a render may ask; the reaction belongs outside it.
    pub(crate) fn matches(&self, rem_size: Pixels) -> bool {
        self.rem_size == Some(rem_size)
    }

    /// Records `rem_size` and reports whether it replaced a *different* one.
    ///
    /// The first observation records without reporting a change: nothing was
    /// measured under an earlier value, so there is nothing to invalidate.
    pub(crate) fn observe(&mut self, rem_size: Pixels) -> bool {
        self.rem_size
            .replace(rem_size)
            .is_some_and(|previous| previous != rem_size)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    #[test]
    fn the_first_observation_records_without_reporting_a_change() {
        let mut key = ResolvedLayoutKey::default();
        assert!(!key.matches(px(16.)));
        assert!(!key.observe(px(16.)));
        assert!(key.matches(px(16.)));
    }

    #[test]
    fn only_a_different_value_reports_a_change() {
        let mut key = ResolvedLayoutKey::default();
        key.observe(px(16.));

        assert!(!key.observe(px(16.)), "the same rem invalidates nothing");
        assert!(
            key.observe(px(24.)),
            "a changed rem invalidates measurement"
        );
        assert!(key.matches(px(24.)));
        assert!(!key.matches(px(16.)));
        assert!(key.observe(px(16.)), "changing back is still a change");
    }
}
