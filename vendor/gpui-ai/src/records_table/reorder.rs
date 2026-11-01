//! Retained row-reorder springs, and the sampling cost they are held to.
//!
//! A records grid defers the whole spring lifecycle here. An accepted snapshot
//! projects one signed travel per candidate row onto the retained springs; a
//! row being rendered samples the offset it should paint. The invariants those
//! two entry points share are stated once, on [`RowReorderState`].

use std::{
    collections::{HashMap, HashSet},
    time::Duration,
};

use gpui::{App, Pixels, SharedString, Window};
use gpui_base::motion::{Spring, spring};

use super::scoped_records_id;

/// One row's retained reorder spring.
///
/// The spring's sampled value never renders directly: the row paints at
/// `sampled - target`, so rest is exactly zero offset in whatever layout slot
/// the row currently owns. A new snapshot retargets by subtracting the new
/// displacement from `target` instead of touching the sample, which is what
/// carries position and velocity through a mid-flight reversal — the channel
/// itself never restarts.
#[derive(Clone, Copy)]
pub(super) struct RowReorderMotion {
    /// Cumulative spring target, in the row's own offset space.
    target: Pixels,
    /// Names the spring channel. Stable while the row keeps moving; fresh
    /// when a settled row starts moving again, so a stale retained sample
    /// cannot leak into the new motion.
    pub(super) incarnation: usize,
    /// The next render must create the channel at rest before retargeting
    /// it, so the first painted frame already carries the full displacement
    /// instead of flashing at the destination for one frame.
    needs_adopt: bool,
}

/// Half a pixel: coarse enough that a settling spring stops requesting frames
/// the eye cannot see, fine enough that the final snap is invisible.
const ROW_REORDER_SPRING_EPSILON: f32 = 0.5;

#[cfg(test)]
thread_local! {
    /// Reorder-map writes performed while sampling on this thread.
    ///
    /// Thread-local for the reason the stable-ID counter is: the harness runs
    /// each test on its own thread, and a shared counter would report another
    /// test's frames.
    static ROW_REORDER_SAMPLE_WRITES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records `written` inserts or removals performed by [`RowReorderState::sample`].
///
/// Sampling runs once per visible row per frame, so its map churn is the only
/// reorder cost that scales with both the viewport and the frame rate; it is
/// measured rather than asserted in prose. Nothing outside tests compiles the
/// counter.
#[inline]
fn note_row_reorder_sample_writes(written: usize) {
    #[cfg(test)]
    ROW_REORDER_SAMPLE_WRITES.with(|writes| writes.set(writes.get().saturating_add(written)));
    #[cfg(not(test))]
    let _ = written;
}

/// Sampling writes since the last call, and resets the counter.
#[cfg(test)]
pub(super) fn take_row_reorder_sample_writes() -> usize {
    ROW_REORDER_SAMPLE_WRITES.with(|writes| writes.replace(0))
}

/// Every retained reorder spring for one records grid.
///
/// The delegate renders rows and the entity accepts snapshots; both defer the
/// whole spring lifecycle here, so these constraints hold at one site rather
/// than at four:
///
/// - **Rest is zero.** A row paints `sampled - target`, so a spring at rest
///   paints exactly the layout slot the row currently owns. Every value in
///   `offsets` is a sample relative to its own target, never a position.
/// - **Seeding and retargeting share one render pass.** [`Self::sample`] is
///   the only site that may clear `needs_adopt`, and it creates the channel at
///   rest and retargets it before returning, so the first painted frame of a
///   move already carries the full displacement instead of flashing at the
///   destination for one frame.
/// - **A live channel is retargeted, never restarted.** [`Self::project`]
///   shifts an existing entry's `target` and keeps its `incarnation`, which is
///   what carries position *and velocity* through a mid-flight reversal.
/// - **A settled row owns nothing**, so `motion.len()` counts rows in motion
///   and a quiet grid retains no state at all.
/// - **An unsampled row owns nothing.** This state owns the rendered window as
///   stable IDs and prunes motion as that membership changes.
/// - **Reduced motion is the caller's gate.** It declines to project at all, so
///   a reader who asked for less motion has no retained state rather than
///   retained state suppressed at render.
/// - **Disabling settles immediately.** `response: None` clears every retained
///   channel and sample because no future frame would be allowed to settle it.
#[derive(Clone, Default)]
pub(super) struct RowReorderState {
    /// Spring response, and the gate: `None` disables reorder motion entirely.
    response: Option<Duration>,
    /// The rows currently under a spring, keyed by stable row ID.
    motion: HashMap<SharedString, RowReorderMotion>,
    /// The offset each moving row last painted, which is what a projection
    /// continues from. Cleared with the snapshot that produced it.
    offsets: HashMap<SharedString, Pixels>,
    /// The stable rows in the virtualized window most recently reported by
    /// upstream, including grids with reorder motion disabled.
    visible: HashSet<SharedString>,
    /// Names the next unseeded channel. Advanced once per accepted snapshot so
    /// a row that settled and starts moving again cannot answer to the
    /// retained sample of its previous journey.
    generation: usize,
}

impl RowReorderState {
    pub(super) fn is_enabled(&self) -> bool {
        self.response.is_some()
    }

    pub(super) fn set_response(&mut self, response: Option<Duration>) {
        self.response = response;
        if response.is_none() {
            self.motion.clear();
            self.offsets.clear();
        }
    }

    pub(super) fn animating_len(&self) -> usize {
        self.motion.len()
    }

    pub(super) fn visible_len(&self) -> usize {
        self.visible.len()
    }

    pub(super) fn visible_ids(&self) -> &HashSet<SharedString> {
        &self.visible
    }

    /// Notes a row constructed before upstream reports the complete window.
    pub(super) fn note_visible(&mut self, row_id: SharedString) {
        self.visible.insert(row_id);
    }

    /// Samples `row_id`'s spring for this frame and returns the offset to paint.
    ///
    /// `None` means the row paints in its layout slot: it owns no motion, or
    /// this sample settled it. Settlement prunes here because this is the only
    /// site that can observe it.
    ///
    /// Call this while rendering the row, which is where GPUI keyed element
    /// state is available.
    pub(super) fn sample(
        &mut self,
        component_id: &str,
        row_id: &SharedString,
        window: &mut Window,
        cx: &mut App,
    ) -> Option<Pixels> {
        let response = self.response?;
        let motion = *self.motion.get(row_id)?;
        let channel: SharedString = format!(
            "{}:{}",
            scoped_records_id("reorder", component_id, row_id),
            motion.incarnation
        )
        .into();
        let policy = Spring::new(response).with_epsilon(ROW_REORDER_SPRING_EPSILON);
        if motion.needs_adopt {
            // Create the channel at rest — zero is the animation's physical
            // origin, not a spacing value — so the retarget below starts the
            // travel with the full displacement already painted. Both calls
            // land in this same render pass, which is what keeps the first
            // frame of a move from flashing at the destination.
            let _ = spring(
                (channel.clone(), "position"),
                gpui::px(0.),
                policy,
                window,
                cx,
            );
            if let Some(entry) = self.motion.get_mut(row_id) {
                entry.needs_adopt = false;
                note_row_reorder_sample_writes(1);
            }
        }
        let sampled = spring((channel, "position"), motion.target, policy, window, cx);
        if sampled == motion.target {
            // Settled: the spring snapped to its target and stopped requesting
            // frames, so the row owns no motion state.
            self.motion.remove(row_id);
            self.offsets.remove(row_id);
            note_row_reorder_sample_writes(2);
            return None;
        }
        let offset = sampled - motion.target;
        self.offsets.insert(row_id.clone(), offset);
        note_row_reorder_sample_writes(1);
        Some(offset)
    }

    /// Replaces the rendered membership and drops motion outside that window.
    pub(super) fn set_visible(&mut self, visible: HashSet<SharedString>) {
        self.motion.retain(|row_id, _| visible.contains(row_id));
        self.offsets.retain(|row_id, _| visible.contains(row_id));
        self.visible = visible;
    }

    /// Projects one signed pixel travel per candidate row onto the retained
    /// springs, yielding the motion an accepted snapshot starts from.
    ///
    /// The caller decides which rows are candidates and how far each travels —
    /// that is viewport geometry. What a spring does with a travel is this
    /// type's decision, so a row already in flight keeps its channel and a row
    /// at rest gets a fresh one.
    pub(super) fn project<'rows>(
        &self,
        travelled: impl Iterator<Item = (&'rows SharedString, Pixels)>,
    ) -> HashMap<SharedString, RowReorderMotion> {
        let fresh_incarnation = self.generation.wrapping_add(1);
        travelled
            .filter_map(|(row_id, displacement)| {
                // Motion offsets default to rest; zero is animation geometry,
                // not a spacing value.
                let prior_offset = self
                    .offsets
                    .get(row_id)
                    .copied()
                    .unwrap_or_else(|| gpui::px(0.));
                let motion = match self.motion.get(row_id) {
                    // Mid-flight: keep the channel and shift only its target,
                    // so the sample — position and velocity — carries straight
                    // through the new projection.
                    Some(prior) => RowReorderMotion {
                        target: prior.target - displacement,
                        incarnation: prior.incarnation,
                        needs_adopt: prior.needs_adopt,
                    },
                    // Never rendered (or newly moving): there is no retained
                    // sample to carry, so seed from rest with the full
                    // displacement painted on the first frame.
                    None => RowReorderMotion {
                        target: gpui::px(0.) - displacement,
                        incarnation: fresh_incarnation,
                        needs_adopt: true,
                    },
                };
                let projected_offset = if motion.needs_adopt {
                    // No frame has sampled this channel yet. Its retained
                    // target still describes every unpainted projection change
                    // from the last visible frame, so two snapshots that cancel
                    // before a draw correctly collapse to rest.
                    gpui::px(0.) - motion.target
                } else {
                    displacement + prior_offset
                };
                if projected_offset == gpui::px(0.) {
                    // At rest in its new slot: no entry, which is also how a
                    // settled row's state gets pruned at the next snapshot even
                    // when it was never rendered again.
                    return None;
                }
                Some((row_id.clone(), motion))
            })
            .collect()
    }

    /// Adopts a projection as the accepted snapshot's motion.
    pub(super) fn accept(&mut self, motion: HashMap<SharedString, RowReorderMotion>) {
        self.motion = motion;
        // The retained samples describe the outgoing snapshot's slots and the
        // projection above already folded them in, so they retire with it.
        self.offsets.clear();
        self.generation = self.generation.wrapping_add(1);
    }

    #[cfg(test)]
    pub(super) fn contains_motion(&self, row_id: &str) -> bool {
        self.motion.contains_key(row_id)
    }

    #[cfg(test)]
    pub(super) fn sampled_offset(&self, row_id: &str) -> Option<Pixels> {
        self.offsets.get(row_id).copied()
    }

    #[cfg(test)]
    pub(super) fn incarnation(&self, row_id: &str) -> Option<usize> {
        self.motion.get(row_id).map(|motion| motion.incarnation)
    }

    #[cfg(test)]
    pub(super) fn is_empty(&self) -> bool {
        self.motion.is_empty() && self.offsets.is_empty()
    }

    #[cfg(test)]
    pub(super) fn retains_only_visible(&self) -> bool {
        self.motion
            .keys()
            .chain(self.offsets.keys())
            .all(|row_id| self.visible.contains(row_id))
    }
}
