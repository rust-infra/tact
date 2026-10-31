//! Layers an application paints into a component's frame.
//!
//! A component owns its structure; what fills the space behind and in front
//! of that structure is the application's business. This is the slot for it:
//! one element under the content, one over it, both clipped to the frame's
//! own shape, neither taking any part in layout.
//!
//! # What can go in one
//!
//! Anything that is an element. A `div` with a patterned background, an
//! `img` — including an animated GIF or WebP — a `canvas` painting arbitrary
//! geometry, or a webview. This crate ships no effects of its own: a shake, a
//! ripple, a halftone field are an application's expression, not a library's.
//!
//! # Corners
//!
//! A layer that paints all the way to the edge has to carry the frame's
//! radius itself, because GPUI cannot clip a subtree to a rounded rectangle -
//! its `ContentMask` is a `Bounds`, so `overflow_hidden` clips to the box and
//! never to a corner. [`frame_radius`] is that radius. A layer that does not
//! reach the corners - scattered dots, a ring in the middle - needs nothing.
//!
//! # Example
//!
//! ```
//! use gpui_ai::prelude::{Decoration, decoration};
//! use gpui::{App, div, prelude::*};
//!
//! # fn example(cx: &App) -> Decoration {
//! Decoration::behind(div().size_full().rounded(decoration::frame_radius(cx)))
//!     .and_above(div().size_full())
//! # }
//! ```
//!
//! # Motion
//!
//! [`animated`] drives a decoration from a looping 0…1 and stops it when the
//! frame it sits in scrolls out of view or the reader has asked for less
//! motion. A decoration that animates on its own clock would keep a scrolled
//! away panel paying for itself forever.

use crate::motion::VisibleAnimationExt as _;
use gpui::{AnyElement, App, ElementId, IntoElement, ParentElement, Pixels, Styled, Window, div};
use gpui_component::ActiveTheme as _;
use std::time::Duration;

/// The layers an application paints into one component's frame.
///
/// Both slots are optional and independent. A decoration never affects the
/// component's size: the frame measures its content, and these are painted
/// into the space that measurement produced.
#[derive(Default)]
pub struct Decoration {
    behind: Option<AnyElement>,
    above: Option<AnyElement>,
}

impl Decoration {
    /// A layer painted under the component's content, over its background.
    pub fn behind(element: impl IntoElement) -> Self {
        Self {
            behind: Some(element.into_any_element()),
            above: None,
        }
    }

    /// A layer painted over the component's content.
    ///
    /// Passes pointer input through to the content underneath, so a veil, a
    /// scanline, or a vignette never costs the component a click.
    pub fn above(element: impl IntoElement) -> Self {
        Self {
            behind: None,
            above: Some(element.into_any_element()),
        }
    }

    /// Adds the layer painted over the component's content.
    pub fn and_above(mut self, element: impl IntoElement) -> Self {
        self.above = Some(element.into_any_element());
        self
    }

    /// Adds the layer painted under the component's content.
    ///
    /// So that either layer can be named first: paint order is fixed by the
    /// component, not by which of these was called.
    pub fn and_behind(mut self, element: impl IntoElement) -> Self {
        self.behind = Some(element.into_any_element());
        self
    }

    /// Takes the under layer, placed over the component's own background.
    fn take_under(&mut self) -> Option<impl IntoElement> {
        self.behind.take().map(clipped)
    }

    /// Takes the over layer, placed over the component's content.
    fn take_over(&mut self) -> Option<impl IntoElement> {
        self.above.take().map(clipped)
    }
}

/// The corner radius a decoration must carry to reach a component's edge.
///
/// Every framed component in this library rounds its frame by the theme's
/// large radius, so a layer that paints to the edge rounds itself by this and
/// lands on the frame exactly. It follows the active theme, including a custom
/// JSON one, which is why it is a call rather than a constant.
///
/// It exists because the alternative does not: GPUI's `ContentMask` is a
/// `Bounds`, so nothing here can clip a layer to a rounded shape on the
/// caller's behalf. Per-element radii on a `div` background or an `img` are
/// resolved in the shader and do work - and note that `ObjectFit::Cover` hands
/// a sprite bounds larger than the element, which puts those radii on corners
/// that are off screen. A covered photograph has to be cropped to the frame,
/// not fitted into it.
pub fn frame_radius(cx: &App) -> Pixels {
    cx.theme().semantic_tokens().radius.lg
}

/// Places one layer over the component's own background, clipped to its box.
///
/// Rectangularly, and that is a real limit rather than an oversight - see
/// [`frame_radius`] for why, and for the radius a layer rounds itself by.
///
/// Painting the corners back in the frame's colour was tried and is wrong: the
/// sliver outside the radius is where whatever sits *behind* the component
/// shows through, so filling it with the component's own surface draws a blob
/// over the backdrop rather than a rounded corner.
fn clipped(layer: AnyElement) -> impl IntoElement {
    div().absolute().inset_0().overflow_hidden().child(layer)
}

/// Places a component's decoration layers.
///
/// Two calls rather than one, because paint order is child order: the under
/// layer has to be the frame's first child and the over layer its last. The
/// `every_framed_component_places_both_decoration_layers` test is what keeps
/// a component from quietly honouring only one of them.
pub(crate) trait DecoratedExt: Sized {
    /// Places the under layer. Call before adding any content.
    fn decoration_under(self, decoration: &mut Decoration) -> Self;

    /// Places the over layer. Call after adding all content.
    fn decoration_over(self, decoration: &mut Decoration) -> Self;
}

impl<E: ParentElement + Styled + Sized> DecoratedExt for E {
    fn decoration_under(mut self, decoration: &mut Decoration) -> Self {
        if let Some(layer) = decoration.take_under() {
            // The frame becomes the positioning context for both layers. Only
            // when there is one: a component with no decoration keeps exactly
            // the layout it had before this existed.
            self.style().position = Some(gpui::Position::Relative);
            self.extend(std::iter::once(layer.into_any_element()));
        }
        self
    }

    fn decoration_over(mut self, decoration: &mut Decoration) -> Self {
        if let Some(layer) = decoration.take_over() {
            self.style().position = Some(gpui::Position::Relative);
            self.extend(std::iter::once(layer.into_any_element()));
        }
        self
    }
}

/// A decoration that animates, and stops when nobody can see it.
///
/// `build` is handed a 0…1 that loops over `period`, and returns the element
/// to paint at that point in the loop. The loop runs only while the region is
/// on screen and unclipped, and holds at 0 when the reader has asked for less
/// motion — so an application's own effect answers to the same preference
/// every animation in this library does, without the application arranging it.
///
/// # Example
///
/// ```
/// use gpui_ai::prelude::decoration;
/// use gpui::{div, prelude::*, px};
/// use std::time::Duration;
///
/// let drifting = decoration::animated("tide", Duration::from_secs(8), |delta| {
///     div().absolute().top(px(delta * 40.)).size_full()
/// });
/// ```
pub fn animated<E>(
    id: impl Into<ElementId>,
    period: Duration,
    build: impl Fn(f32) -> E + 'static,
) -> impl IntoElement
where
    E: IntoElement + 'static,
{
    let id = id.into();
    // The animator receives the container, not the caller's element, so the
    // caller's closure runs once per frame and its result is the child.
    div().size_full().with_visible_animation(
        id,
        gpui::Animation::new(period).repeat(),
        move |container, delta| container.child(build(delta).into_any_element()),
    )
}

/// Drives a decoration from a value the application already has.
///
/// Where [`animated`] loops on its own, this eases towards `target` whenever
/// the application moves it — a ripple started by a press, a tilt that
/// follows a pointer, a shake that grows as a value approaches its limit.
/// Returns `target` immediately when the reader has asked for less motion.
pub fn toward(
    id: impl Into<gpui_base::motion::TransitionId>,
    target: f32,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    if !crate::motion::motion_is_full(cx) {
        return target;
    }
    let standard = crate::motion::MotionTokens::read(cx).standard();
    gpui_base::motion::transition(
        id,
        target,
        gpui_base::motion::Transition::new(standard).ease(crate::motion::ease_out_quint),
        window,
        cx,
    )
}
