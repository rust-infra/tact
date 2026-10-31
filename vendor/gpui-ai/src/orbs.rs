//! An ambient, always-moving indicator for "the model is alive" moments.
//!
//! The glyph is a 3×3 dot lattice whose cells pulse with choreographed
//! phase offsets. Variants give the same geometry distinct motion
//! personalities — a radial wavefront, a diagonal band sweep, a comet
//! running the lattice perimeter, a traveling column, and a scrambled
//! pulse — so an application can pick the one that matches its voice.
//! Under reduced motion every variant resolves to a useful static frame.

use crate::motion::{AmbientLoopSpec, MotionTokens, VisibleAnimationExt as _};
use gpui::InteractiveElement as _;
use gpui::{
    App, ElementId, IntoElement, ParentElement as _, Pixels, RenderOnce, StyleRefinement, Styled,
    Window, div, px,
};
use gpui_component::{ActiveTheme as _, StyledExt as _};

/// Lattice is N×N dots. Dot positions derive from the cluster diameter and
/// dot size; no separate pitch constant is needed.
const N: usize = 3;

/// Stage edge length the geometry is tuned on.
const STAGE: f32 = 28.0;

/// Center-to-center lattice spacing at stage size, in stage pixels.
const PITCH_STAGE: f32 = 6.5;

/// One cycle of any variant, in milliseconds. Phase offsets are fractions of
/// this duration. Derived from the ambient-loop role, not owned here: the
/// choreography below phases in integer beats and needs the cycle as a
/// number, not a `Duration`.
const CYCLE_MS: u64 = AmbientLoopSpec::ORB_LATTICE.period_millis();

/// A choreography for the lattice's per-dot animation delays and motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OrbVariant {
    /// Radiates from the center on a round wavefront.
    #[default]
    Radial,
    /// A broad band crosses the grid on the diagonal.
    Diagonal,
    /// One head with a decaying tail runs the perimeter clockwise.
    Comet,
    /// A soft column travels left to right.
    Column,
    /// Like `Comet`, but the pulse jumps pseudo-randomly around the ring.
    Scattered,
}

impl OrbVariant {
    /// All variants, in stable display order.
    pub const ALL: [Self; 5] = [
        Self::Radial,
        Self::Diagonal,
        Self::Comet,
        Self::Column,
        Self::Scattered,
    ];

    /// Stable identifier used in element IDs and tests.
    pub fn slug(self) -> &'static str {
        match self {
            Self::Radial => "radial",
            Self::Diagonal => "diagonal",
            Self::Comet => "comet",
            Self::Column => "column",
            Self::Scattered => "scattered",
        }
    }

    /// Per-cell animation delay in milliseconds. Negative values seed a cell
    /// partway into the shared cycle, which turns nine identical dots on one
    /// clock into one traveling pattern.
    fn cell_delay(self, x: usize, y: usize) -> i64 {
        let mid = (N - 1) as f32 / 2.0;
        let dx = x as f32 - mid;
        let dy = y as f32 - mid;
        match self {
            // Center leads a beat early so the next swell doesn't sit behind
            // the outer fade.
            Self::Radial => {
                (dx.hypot(dy) * 700.0) as i64 - if dx == 0.0 && dy == 0.0 { 180 } else { 0 }
            }
            // Spread close to the cycle keeps the sweep continuous: the far
            // corner restarts as the near one does.
            Self::Diagonal => ((x + y) as f32 / (2.0 * (N - 1) as f32) * 1500.0) as i64,
            Self::Comet | Self::Scattered => {
                let Some(ring_index) = perimeter_index(x, y) else {
                    return 0;
                };
                let count = perimeter_len() as f32;
                match self {
                    Self::Comet => {
                        -(((perimeter_len() - ring_index) % perimeter_len()) as f32 / count
                            * CYCLE_MS as f32) as i64
                    }
                    // (i * 3) % len walks the ring in a scattered order.
                    Self::Scattered => {
                        -(((ring_index * 3) % perimeter_len()) as f32 / count * CYCLE_MS as f32)
                            as i64
                    }
                    _ => unreachable!("outer match covers both arms"),
                }
            }
            Self::Column => (x as f32 / (N - 1) as f32 * 1100.0) as i64,
        }
    }
}

/// Clockwise walk of the lattice perimeter — the track `Comet`/`Scattered`
/// pulses run on. Returns the ring position of a cell, or `None` for the
/// interior center.
fn perimeter_index(x: usize, y: usize) -> Option<usize> {
    let last = N - 1;
    let on_ring = x == 0 || x == last || y == 0 || y == last;
    if !on_ring {
        return None;
    }
    // Walk: top row left→right, right column top→bottom, bottom row
    // right→left, left column bottom→top.
    let index = if y == 0 {
        x
    } else if x == last {
        last + y
    } else if y == last {
        last + last + (last - x)
    } else {
        last + last + last + (last - y)
    };
    Some(index)
}

/// Number of cells on the lattice perimeter.
fn perimeter_len() -> usize {
    4 * (N - 1)
}

/// A cluster of softly pulsing dots — an ambient thinking indicator for
/// moments with no progress to report, where a spinner would feel too
/// mechanical (voice idle states, model warm-up, long silent reasoning).
///
/// Dots take their colors from the active theme (`primary`, `info`, `cyan`),
/// so the indicator follows light/dark modes and custom themes like every
/// other component.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// # use gpui::px;
/// Orbs::new();                                  // default variant, 40px
/// Orbs::new().variant(OrbVariant::Comet);       // comet choreography
/// Orbs::new().diameter(px(64.));                // larger
/// ```
#[derive(IntoElement)]
pub struct Orbs {
    id: ElementId,
    style: StyleRefinement,
    diameter: Pixels,
    variant: OrbVariant,
}

impl Orbs {
    /// Creates a 40px orb cluster using the [`OrbVariant::Radial`] variant.
    #[track_caller]
    pub fn new() -> Self {
        Self {
            id: ElementId::CodeLocation(*std::panic::Location::caller()),
            style: StyleRefinement::default(),
            diameter: px(40.),
            variant: OrbVariant::default(),
        }
    }

    /// Sets stable instance identity. Supply a domain ID when constructing
    /// multiple clusters from the same call site (for example, in a loop).
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.id = id.into();
        self
    }

    /// Sets the overall cluster diameter. Dot sizes and spacing scale with it.
    pub fn diameter(mut self, diameter: impl Into<Pixels>) -> Self {
        self.diameter = diameter.into();
        self
    }

    /// Sets the lattice choreography.
    pub fn variant(mut self, variant: OrbVariant) -> Self {
        self.variant = variant;
        self
    }
}

impl Default for Orbs {
    #[track_caller]
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for Orbs {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Everything one dot needs per animation frame, computed once per render.
///
/// The whole lattice samples a single clock, so per-frame work is this table
/// walked nine times — no per-dot animation state, and nothing allocated
/// while the clock runs.
struct LatticeDot {
    home_x: Pixels,
    home_y: Pixels,
    gather: (f32, f32),
    release: (f32, f32),
    seeded_phase: f32,
    color: gpui::Hsla,
}

impl RenderOnce for Orbs {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let full = crate::motion::motion_is_full(cx);
        let lattice = MotionTokens::read(cx).orb_lattice();
        let scale = self.diameter.as_f32() / STAGE;
        let dot = px(4.2 * scale);
        // Center-to-center spacing between lattice dots.
        let pitch = px(6.5 * scale);
        let colors = [cx.theme().primary, cx.theme().info, cx.theme().cyan];
        let variant = self.variant;
        let variant_index = OrbVariant::ALL
            .iter()
            .position(|candidate| *candidate == variant)
            .unwrap_or_default() as u64;
        // Lattice geometry: center the N×N grid of dots inside the cluster.
        // Cell pitch keeps a dot-gap-dot rhythm; the whole block is offset so
        // the lattice is centered regardless of diameter.
        let span = px((N - 1) as f32 * pitch.as_f32());
        let origin = (self.diameter - span - dot) / 2.0;
        let radius = tokens.radius.full;

        let dots: Vec<LatticeDot> = (0..N)
            .flat_map(|y| (0..N).map(move |x| (x, y)))
            .map(|(x, y)| {
                let delay = variant.cell_delay(x, y);
                // Negative delays seed partway into the cycle; the shared
                // clock starts at zero, so shift into the positive domain by
                // pre-advancing the phase.
                let seeded_phase = if delay < 0 {
                    (CYCLE_MS as i64 + delay % CYCLE_MS as i64) as f32 / CYCLE_MS as f32
                } else {
                    delay as f32 / CYCLE_MS as f32
                };
                // Swirl-settle: each dot gathers from a position rotated one
                // way around the lattice center and releases to the mirror
                // rotation, so the cycle keeps swirling in one direction
                // instead of rewinding.
                LatticeDot {
                    home_x: origin + pitch * x,
                    home_y: origin + pitch * y,
                    gather: swirl_offset(x, y, -SWIRL_RADIANS, scale),
                    release: swirl_offset(x, y, SWIRL_RADIANS, scale),
                    seeded_phase,
                    color: colors[(x + y) % colors.len()].opacity(0.55),
                }
            })
            .collect();

        div()
            .id(self.id)
            .relative()
            .size(self.diameter)
            .refine_style(&self.style)
            .child(div().relative().size_full().with_visible_animation(
                ElementId::NamedInteger("orb-lattice".into(), variant_index),
                // Frame demand: intentionally ambient. Orbs are the "the
                // model is alive" glyph, so the lattice animates for as long
                // as the cluster is on screen; there is no settled state to
                // reach and treating one as a bug would delete the
                // component's reason to exist. The caller decides when the
                // cluster is mounted. One clock for all nine dots,
                // phase-locked to the shared epoch so side-by-side clusters
                // tick together. Reduced motion holds delta at 0, leaving
                // each dot at its seeded phase — a still choreographed
                // frame, not nine identical dots.
                lattice.looping_synced(),
                move |stage, delta| {
                    let delta = if full { delta } else { 0.0 };
                    stage.children(dots.iter().map(|spec| {
                        let phase = (delta + spec.seeded_phase) % 1.0;
                        // Eased triangle: 0→1→0 across the cycle so each dot
                        // leaves home, swings through the far rotation, and
                        // returns.
                        let swing = if phase < 0.5 {
                            phase * 2.0
                        } else {
                            2.0 - phase * 2.0
                        };
                        let eased = swing * swing * (3.0 - 2.0 * swing);
                        let (ax, ay) = spec.gather;
                        let (bx, by) = spec.release;
                        let dx = ax + (bx - ax) * eased;
                        let dy = ay + (by - ay) * eased;
                        // One-beat swell rides on top of the travel:
                        // brightest mid-swing, dimmest at rest.
                        let swell = (phase * std::f32::consts::TAU).sin();
                        div()
                            .absolute()
                            .left(spec.home_x + px(dx))
                            .top(spec.home_y + px(dy))
                            .size(dot)
                            .rounded(radius)
                            .bg(spec.color)
                            .opacity(0.35 + 0.5 * (0.5 + 0.5 * swell))
                    }))
                },
            ))
    }
}

/// Radians of rotation at each end of the settle cycle (~60°).
const SWIRL_RADIANS: f32 = 1.05;

/// Outward push on top of the rotation, as a fraction of pitch.
const SWIRL_SPREAD: f32 = 1.6;

/// Offset from a cell's own grid slot to its swirled position, scaled to the
/// rendered cluster size.
fn swirl_offset(x: usize, y: usize, angle: f32, scale: f32) -> (f32, f32) {
    let mid = (N - 1) as f32 / 2.0;
    let dx = x as f32 - mid;
    let dy = y as f32 - mid;
    let pitch = PITCH_STAGE * scale;
    let cos = angle.cos();
    let sin = angle.sin();
    (
        ((dx * cos - dy * sin) * SWIRL_SPREAD - dx) * pitch,
        ((dx * sin + dy * cos) * SWIRL_SPREAD - dy) * pitch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, WindowHandle, size};

    struct LatticeProbe {
        variant: OrbVariant,
    }

    impl Render for LatticeProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(Orbs::new().variant(self.variant))
        }
    }

    fn open(variant: OrbVariant, cx: &mut TestAppContext) -> WindowHandle<LatticeProbe> {
        cx.update(crate::init);
        let window = cx.open_window(size(px(80.), px(80.)), |_, _| LatticeProbe { variant });
        cx.run_until_parked();
        next_frame(&window, cx);
        window
    }

    fn next_frame(window: &WindowHandle<LatticeProbe>, cx: &mut TestAppContext) -> usize {
        let callbacks = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the lattice window should remain open");
        cx.run_until_parked();
        callbacks
    }

    #[gpui::test]
    fn every_variant_runs_its_lattice_on_one_clock(cx: &mut TestAppContext) {
        let window = open(OrbVariant::Radial, cx);
        for variant in OrbVariant::ALL {
            window
                .update(cx, |probe, _, cx| {
                    probe.variant = variant;
                    cx.notify();
                })
                .expect("the lattice window should remain open");
            cx.run_until_parked();
            // The switch re-renders while the previous frame's callback is
            // still queued; drain it, then hold the steady state.
            next_frame(&window, cx);
            assert_eq!(
                next_frame(&window, cx),
                1,
                "{variant:?} must derive nine dots from one clock"
            );
        }
    }

    #[gpui::test]
    fn reduced_motion_leaves_a_choreographed_still_lattice(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = open(OrbVariant::Radial, cx);
        assert_eq!(
            next_frame(&window, cx),
            0,
            "a held lattice schedules nothing; seeded phases are the still frame"
        );
    }

    #[test]
    fn radial_wavefront_radiates_from_center() {
        let center = OrbVariant::Radial.cell_delay(1, 1);
        let corner = OrbVariant::Radial.cell_delay(2, 2);
        assert!(center < corner, "corner should lag the center");
        // Center leads by its beat bonus.
        assert_eq!(center, -180);
    }

    #[test]
    fn diagonal_band_orders_by_x_plus_y() {
        let first = OrbVariant::Diagonal.cell_delay(0, 0);
        let middle = OrbVariant::Diagonal.cell_delay(1, 1);
        let last = OrbVariant::Diagonal.cell_delay(2, 2);
        assert!(first < middle && middle < last);
    }

    #[test]
    fn column_travels_left_to_right() {
        let left = OrbVariant::Column.cell_delay(0, 1);
        let right = OrbVariant::Column.cell_delay(2, 1);
        assert!(left < right);
    }

    #[test]
    fn comet_walks_the_perimeter_and_skips_the_center() {
        assert_eq!(OrbVariant::Comet.cell_delay(1, 1), 0);
        // Clockwise walk: (0,0)→(1,0)→(2,0)→(2,1)→(2,2). The head cell (0,0)
        // leads at zero; every follower seeds progressively *later* in the
        // previous cycle (delays rise toward zero), so the visible pulse
        // travels clockwise along the ring.
        let delays: Vec<i64> = [(0, 0), (1, 0), (2, 0), (2, 1), (2, 2)]
            .map(|(x, y)| OrbVariant::Comet.cell_delay(x, y))
            .to_vec();
        assert_eq!(*delays.first().expect("non-empty"), 0);
        assert!(
            delays[1..].windows(2).all(|pair| pair[0] < pair[1]),
            "follower delays must increase clockwise: {delays:?}"
        );
    }

    #[test]
    fn scattered_visits_every_ring_cell_with_shuffled_phases() {
        // Both variants produce the same set of phase magnitudes over the
        // ring (all eight cells), but assign them in a different order.
        let mut comet: Vec<u64> = perimeter_cells()
            .map(|(x, y)| OrbVariant::Comet.cell_delay(x, y).unsigned_abs())
            .collect();
        let mut scattered: Vec<u64> = perimeter_cells()
            .map(|(x, y)| OrbVariant::Scattered.cell_delay(x, y).unsigned_abs())
            .collect();
        comet.sort_unstable();
        scattered.sort_unstable();
        assert_eq!(comet, scattered);

        // The assignment order differs: at least one ring cell has a
        // different phase under Scattered than under Comet.
        let different = perimeter_cells()
            .zip(perimeter_cells())
            .any(|((cx_, cy_), (sx, sy))| {
                OrbVariant::Comet.cell_delay(cx_, cy_) != OrbVariant::Scattered.cell_delay(sx, sy)
            });
        assert!(different, "scattered must not preserve the clockwise order");
    }

    /// Ring cells in clockwise order.
    fn perimeter_cells() -> impl Iterator<Item = (usize, usize)> {
        (0..N)
            .map(|x| (x, 0))
            .chain((1..N).map(move |y| (N - 1, y)))
            .chain((0..N - 1).rev().map(move |x| (x, N - 1)))
            .chain((1..N - 1).rev().map(move |y| (0, y)))
    }

    #[test]
    fn perimeter_covers_all_eight_ring_cells_exactly_once() {
        let mut seen = Vec::new();
        for y in 0..N {
            for x in 0..N {
                if let Some(index) = perimeter_index(x, y) {
                    seen.push(index);
                }
            }
        }
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), perimeter_len());
        assert_eq!(seen, (0..perimeter_len()).collect::<Vec<_>>());
    }
}
