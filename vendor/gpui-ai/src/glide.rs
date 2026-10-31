//! One hover highlight that glides between the uniform rows of a list.
//!
//! Per-row hover fills flicker: each pointer move extinguishes one row's
//! background and ignites the next. A gliding highlight is one element that
//! chases the pointer between rows at the hover-glide tempo, so hovering
//! reads as a single object in motion. Rows opt in by attaching
//! [`glide_row`], the list container mounts [`glide_highlight`] inside a
//! `relative()` frame it captured with [`glide_frame`], and rows stop
//! painting their own hover fill — the highlight is the hover.
//!
//! Geometry rules: the highlight tweens between *different* rows; a bounds
//! change under the same hovered row (the list scrolled beneath a
//! stationary pointer) repositions instantly, so the highlight never lags
//! its own row. Any reduced motion preference snaps every move. Leaving
//! the rows drops the highlight the way per-row hover would.

use std::collections::HashMap;

use gpui::canvas;
use gpui::{
    App, Bounds, Div, ElementId, Entity, InteractiveElement as _, ParentElement as _, Pixels,
    SharedString, StatefulInteractiveElement, Styled as _, Window, div, px,
};
use gpui_base::motion::{Transition, transition};
use gpui_component::ActiveTheme as _;

/// One element's box, in window coordinates, every frame it is painted.
///
/// Not `ElementExt::on_prepaint`, which measures a `size_full` child and so
/// reports the **content** box - inside the element's border *and* its
/// padding. The highlight has to cover a row, not a row's text: measured that
/// way, a model option with eight pixels of left padding put the highlight
/// nine pixels in and ten pixels short, and the row's own selection fill sat
/// somewhere else entirely.
///
/// `inset_0` reports the padding box instead, which is the element's own box
/// less its border. Both the layer and the rows go through here, so what is
/// left is one border width on each, and it cancels in the subtraction.
fn capture<E: gpui::ParentElement>(
    element: E,
    mut record: impl FnMut(Bounds<Pixels>, &mut Window, &mut App) + 'static,
) -> E {
    element.child(
        canvas(
            move |bounds, window, cx| record(bounds, window, cx),
            |_, _, _, _| {},
        )
        .absolute()
        .inset_0(),
    )
}

/// One row's last painted geometry, stamped with the frame that painted it.
#[derive(Clone, Copy)]
struct RowGeometry {
    bounds: Bounds<Pixels>,
    painted: u64,
}

/// Window-keyed hover state shared by one list's rows and its highlight.
pub(crate) struct GlideHover {
    rows: HashMap<SharedString, RowGeometry>,
    /// Where the highlight's own coordinate space starts, in window
    /// coordinates.
    ///
    /// Captured from the layer the highlight sits in rather than from the
    /// list's frame, and those are not the same box. A frame carries a
    /// border and padding; an absolutely positioned child is placed inside
    /// them. Subtracting the frame's origin from a row's therefore left the
    /// highlight off by whatever inset the frame happened to have - three
    /// pixels up and five across on the model picker, enough to clip the
    /// group label above it, and one pixel on a list with only a border.
    ///
    /// The layer is `absolute inset_0`, so it *is* the box the highlight is
    /// positioned in, whatever GPUI resolves that to. The arithmetic stops
    /// depending on knowing.
    layer: Option<Bounds<Pixels>>,
    hovered: Option<SharedString>,
    /// Bumps when hovering starts from nothing or the hovered row moves
    /// beneath the pointer; a fresh generation keys fresh transition
    /// clocks, which start settled at their target — a snap.
    generation: u64,
    /// The frame rows are stamping now. The container advances it, and a
    /// row whose geometry carries an older stamp was not painted last
    /// frame — it has scrolled away, been filtered out, or unmounted.
    epoch: u64,
    /// The frame the pointer last left a row on. Leaving one row and
    /// entering the next happens inside a single mouse dispatch, so a
    /// departure recorded on this same frame is a move between rows, not
    /// an arrival from nothing.
    departed: Option<u64>,
}

impl GlideHover {
    fn new() -> Self {
        Self {
            rows: HashMap::new(),
            layer: None,
            hovered: None,
            generation: 0,
            epoch: 0,
            departed: None,
        }
    }
}

/// The list's shared glide state, keyed by the list's own identity.
pub(crate) fn glide_hover_state(
    list: ElementId,
    window: &mut Window,
    cx: &mut App,
) -> Entity<GlideHover> {
    window.use_keyed_state((list, "glide-hover"), cx, |_, _| GlideHover::new())
}

/// Keeps the row bookkeeping honest for one list. The frame must be
/// `relative()`, so the highlight's layer resolves against it.
///
/// It no longer captures where anything is: the highlight's layer does that
/// for itself, because the layer is the only box whose origin the highlight's
/// own coordinates are measured from.
pub(crate) fn glide_frame<E>(frame: E, state: &Entity<GlideHover>) -> E
where
    E: gpui::ParentElement,
{
    let state = state.clone();
    capture(frame, move |_, window, cx| {
        state.update(cx, |state, cx| {
            // Rows stamp the epoch this container hands them, so anything
            // still carrying the previous stamp was not painted last
            // frame. Dropping those is what keeps the map bounded over a
            // long scroll, and what stops a highlight from outliving the
            // row it belongs to: a row filtered out from under a
            // stationary pointer fires no hover-left, because an unmounted
            // element has no listener to fire it.
            let painted = state.epoch;
            state.rows.retain(|_, row| row.painted == painted);
            if state
                .hovered
                .as_ref()
                .is_some_and(|key| !state.rows.contains_key(key))
            {
                state.hovered = None;
                // A real state change, and the frame that drew the stale
                // highlight has already gone out; ask for the one that
                // does not.
                cx.notify();
            }
            state.epoch = painted.wrapping_add(1);
        });
        // The container's own bounds move without any hover changing —
        // a resize, a pane collapse — and the highlight is positioned
        // relative to them.
        if state.read(cx).hovered.is_some() {
            window.request_animation_frame();
        }
    })
}

/// Instruments one row: its bounds feed the highlight, and hovering it
/// moves the highlight here. The row should paint no hover fill of its
/// own.
pub(crate) fn glide_row<E>(row: E, key: SharedString, state: &Entity<GlideHover>) -> E
where
    E: StatefulInteractiveElement + gpui::ParentElement,
{
    let bounds_state = state.clone();
    let bounds_key = key.clone();
    let hover_state = state.clone();
    capture(row, move |bounds, window, cx| {
        let moved = bounds_state.update(cx, |state, _| {
            let epoch = state.epoch;
            let previous = state.rows.insert(
                bounds_key.clone(),
                RowGeometry {
                    bounds,
                    painted: epoch,
                },
            );
            let moved = state.hovered.as_ref() == Some(&bounds_key)
                && previous.is_some_and(|previous| previous.bounds != bounds);
            if moved {
                // The hovered row moved beneath a stationary pointer —
                // scrolling, most often. Snap with it rather than chase it.
                state.generation += 1;
            }
            moved
        });
        // This runs after the render that placed the highlight, so the
        // corrected position needs a frame of its own. Only when the row
        // actually moved, so a settled list still asks for nothing.
        if moved {
            window.request_animation_frame();
        }
    })
    .on_hover(move |hovered, _, cx| {
        hover_state.update(cx, |state, cx| {
            if *hovered {
                // Leaving one row and entering the next happens inside a
                // single mouse dispatch, and the order the two listeners
                // run in depends on paint order — moving up a list fires
                // the departure first. Treating "nothing is hovered" as an
                // arrival from rest would therefore teleport the highlight
                // in one direction and glide it in the other; a departure
                // stamped on this same frame is a move between rows.
                let arriving_from_rest =
                    state.hovered.is_none() && state.departed != Some(state.epoch);
                if arriving_from_rest {
                    state.generation += 1;
                }
                if state.hovered.as_ref() != Some(&key) {
                    state.hovered = Some(key.clone());
                    cx.notify();
                }
            } else if state.hovered.as_ref() == Some(&key) {
                state.hovered = None;
                state.departed = Some(state.epoch);
                cx.notify();
            }
        });
    })
}

/// The one highlight element. Mount it as an early child of the captured
/// `relative()` frame so rows and their content paint above it.
pub(crate) fn glide_highlight(
    list: ElementId,
    state: &Entity<GlideHover>,
    radius: Pixels,
    debug_selector: &'static str,
    window: &mut Window,
    cx: &mut App,
) -> Div {
    let placed = placement(state, cx);
    let Some((target, generation)) = placed else {
        // No layer yet, or nothing hovered. The layer still mounts: it is what
        // measures the box the highlight will be placed in, so it has to be on
        // screen for a frame before there is anything to place.
        return highlight_layer(state);
    };
    // Between rows the highlight tweens on the glide tempo and the strong
    // ease-out; reduced motion collapses the tween to a snap. A fresh
    // generation keys fresh clocks that begin settled at the target.
    let duration = if crate::motion::motion_is_full(cx) {
        crate::motion::MotionTokens::read(cx).hover_glide()
    } else {
        std::time::Duration::ZERO
    };
    let mut channel = |name: &str, value: f32| {
        transition(
            ElementId::Name(format!("{list:?}-glide-{name}-{generation}").into()),
            value,
            Transition::new(duration).ease(crate::motion::ease_out_quint),
            window,
            cx,
        )
    };
    let x = channel("x", f32::from(target.origin.x));
    let y = channel("y", f32::from(target.origin.y));
    let w = channel("w", f32::from(target.size.width));
    let h = channel("h", f32::from(target.size.height));

    highlight_layer(state).child(
        div()
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(w))
            .h(px(h))
            .rounded(radius)
            .bg(cx.theme().list_hover)
            .debug_selector(move || debug_selector.to_owned()),
    )
}

/// Where the highlight goes, once there is a hovered row and a layer to
/// measure it against.
fn placement(state: &Entity<GlideHover>, cx: &App) -> Option<(Bounds<Pixels>, u64)> {
    let state = state.read(cx);
    let layer = state.layer?;
    let hovered = state.hovered.as_ref()?;
    // Only geometry from the frame just painted. A row that has left the tree
    // keeps an older stamp, and rendering happens before the prepaint that
    // prunes it - so the stamp, not the map, is what decides whether there is
    // still a row to highlight.
    let row = state
        .rows
        .get(hovered)
        .filter(|row| row.painted == state.epoch)?
        .bounds;
    Some((
        Bounds::new(row.origin - layer.origin, row.size),
        state.generation,
    ))
}

/// The box the highlight's coordinates are measured from.
///
/// `absolute inset_0`, so it fills whatever GPUI treats as the positioning
/// box for an absolute child of the list's frame — which is the same box the
/// highlight inside it is placed in. Capturing its origin rather than the
/// frame's is what makes `row.origin - layer.origin` exact instead of off by
/// the frame's border and padding.
fn highlight_layer(state: &Entity<GlideHover>) -> Div {
    let state = state.clone();
    capture(div().absolute().inset_0(), move |bounds, window, cx| {
        let moved = state.update(cx, |state, _| {
            let moved = state
                .layer
                .is_some_and(|layer| layer.origin != bounds.origin);
            state.layer = Some(bounds);
            moved
        });
        // The list moved under a highlight that is already up - a popover
        // flipping to the other side, a pane resizing. The position was
        // computed against the old origin, so the corrected one needs a frame
        // of its own.
        if moved {
            window.request_animation_frame();
        }
    })
}
