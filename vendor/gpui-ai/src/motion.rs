//! Motion primitives shared by gpui-ai components.
//!
//! Every effect here is built on GPUI's animation system, so reduced-motion
//! mode resolves to a useful static frame automatically: one-shot reveals
//! settle at their end state and repeating effects (shimmer, breathing)
//! render at rest. Components opt in explicitly — nothing in this module
//! installs idle redraw on its own.
//!
//! # The motion policy
//!
//! No component spells out a duration. Every animated timing in the crate
//! resolves through [`MotionTokens`], one application-global policy that
//! [`crate::init`] installs with the crate's defaults and an application may
//! replace once, up front, for all components at a stroke.
//!
//! The policy speaks two vocabularies:
//!
//! - **Semantic roles**, the consumer-facing surface: [`MotionTokens::instant`],
//!   [`MotionTokens::quick`], [`MotionTokens::standard`], and
//!   [`MotionTokens::deliberate`] for durations, and the four spring roles —
//!   press, selection, disclosure, reflow — as [`SpringRole`] response/damping
//!   pairs. Components consume roles, never raw numbers.
//! - **Named effects**, the crate-internal table: each shipped repeating
//!   effect (shimmer, grid sweep, image pulse, status spinner, breathing, orb
//!   lattice) keeps its own tempo entry, because collapsing six tuned tempos
//!   into one "ambient" number would be a visual change wearing a refactor's
//!   clothes. These readers stay `pub(crate)`.
//!
//! A progress loop is bound to work that ends: the element carrying it is
//! gone once the work finishes. An ambient loop has no completion to report
//! and runs for as long as its element is on screen. The instant role is
//! zero by conviction, not omission: nothing in the crate delays or eases a
//! direct response to input.
//!
//! Reduced motion outranks the policy. Every effect here routes through
//! GPUI's animation system, whose reduced-motion contract no token value can
//! override — a policy with ten-second durations still renders still frames
//! when the platform asks for stillness.
//!
//! # Example
//!
//! ```no_run
//! # use gpui_ai::prelude::*;
//! # use gpui::ParentElement;
//! # use gpui_component::v_flex;
//! # fn example(is_running: bool, window: &mut gpui::Window, cx: &mut gpui::App) {
//! use gpui_ai::motion::{Shimmer, reveal};
//!
//! // A "Thinking…" label with a travelling highlight while work runs.
//! Shimmer::new("thinking-label", "Thinking…").active(is_running);
//!
//! // A freshly inserted row fades and rises into place once.
//! reveal(v_flex().child("New tool call"), ("tool-call", 3_usize), window, cx);
//! # }
//! ```

mod visibility;
pub(crate) use visibility::VisibleAnimationExt;

use gpui::{
    Animation, AnimationElement, AnimationExt as _, App, ElementId, Global, Hsla, IntoElement,
    ParentElement as _, RenderOnce, SharedString, StyleRefinement, Styled, Window, div,
    ease_in_out, pulsating_between, px, relative,
};
/// The crate's standard enter/exit curve: quint-class ease-out
/// (`cubic-bezier(0.23, 1, 0.32, 1)`-equivalent). Leaves at full speed
/// and spends most of its time settling, which is what makes a quick
/// duration read as instant. The soft cubic stays available for gentle
/// hover-class changes; deliberate entrances use this.
pub(crate) fn ease_out_quint(t: f32) -> f32 {
    let inverse = 1.0 - t.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse * inverse * inverse
}

/// Strong ease-in-out (quart-class,
/// `cubic-bezier(0.77, 0, 0.175, 1)`-equivalent) for something already on
/// screen travelling between two places — fills retargeting, the jump
/// drive's glide.
pub(crate) fn ease_in_out_quart(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        8.0 * t * t * t * t
    } else {
        let inverse = 1.0 - t;
        1.0 - 8.0 * inverse * inverse * inverse * inverse
    }
}
use gpui_base::motion::{Transition, transition};
use gpui_component::{ActiveTheme as _, StyledExt as _};
use std::collections::HashMap;
use std::time::Duration;
#[cfg(not(target_family = "wasm"))]
pub(crate) use std::time::Instant;
#[cfg(target_family = "wasm")]
pub(crate) use web_time::Instant;

/// Width of the travelling shimmer highlight as a fraction of the label
/// width. Highlight geometry rather than tempo, so it stays out of the role
/// specs below.
const SHIMMER_BAND: f32 = 0.45;

/// A one-shot entrance: a newly mounted element settling into place.
///
/// One spec covers every entrance in the crate deliberately. Rows, chips,
/// tool calls, and attachments arrive at one tempo; a second entrance tempo
/// would be a design decision, not a configuration.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EnterSpec {
    /// Mount to settled.
    pub(crate) duration: Duration,
    /// Added per sibling index, so a list ripples rather than snapping.
    pub(crate) stagger: Duration,
    /// Index past which the stagger stops growing, bounding how long the
    /// last item of a long list waits.
    pub(crate) stagger_cap: usize,
    /// Travel from rest, in pixels. An animation distance, not layout.
    pub(crate) rise: f32,
}

impl EnterSpec {
    /// The crate's entrance tempo.
    pub(crate) const REVEAL: Self = Self {
        duration: Duration::from_millis(260),
        stagger: Duration::from_millis(40),
        stagger_cap: 12,
        rise: 6.0,
    };

    /// The per-item delay of a staggered reveal, capped so long lists settle
    /// in a bounded time.
    pub(crate) fn stagger_delay(self, index: usize) -> Duration {
        self.stagger * index.min(self.stagger_cap) as u32
    }
}

/// A repeating loop that means "this work is in flight". It exists only
/// while the work does: the caller stops rendering the element that carries
/// it once the work finishes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ProgressLoopSpec {
    /// One full pass, including any rest beat.
    pub(crate) period: Duration,
    /// Fraction of `period` spent moving. Below `1.0` the remainder is a
    /// rest beat, so consecutive passes read as deliberate rather than
    /// frantic.
    pub(crate) duty: f32,
}

impl ProgressLoopSpec {
    /// Label shimmer — the ecosystem's "something is happening" text
    /// treatment.
    pub(crate) const SHIMMER: Self = Self {
        period: Duration::from_millis(1800),
        duty: 0.72,
    };

    /// Diagonal sweep across the pixel-grid loader.
    pub(crate) const GRID_SWEEP: Self = Self {
        period: Duration::from_millis(1400),
        duty: 1.0,
    };

    /// Placeholder pulse shown until generated pixels arrive.
    pub(crate) const IMAGE_PULSE: Self = Self {
        period: Duration::from_millis(1600),
        duty: 1.0,
    };

    /// One rotation of an in-flight status icon.
    pub(crate) const STATUS_SPINNER: Self = Self {
        period: Duration::from_millis(900),
        duty: 1.0,
    };

    /// This loop as a repeating GPUI animation.
    pub(crate) fn looping(self) -> Animation {
        repeating(self.period)
    }

    /// This loop as a repeating animation phase-locked to the application's
    /// shared epoch, for a region clock whose siblings should tick together.
    pub(crate) fn looping_synced(self) -> Animation {
        repeating_synced(self.period)
    }
}

/// A repeating loop with nothing to complete. It runs for as long as its
/// element is on screen, which is the state it reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct AmbientLoopSpec {
    /// One full cycle. Choreographed phase offsets are fractions of this.
    pub(crate) period: Duration,
}

impl AmbientLoopSpec {
    /// Opacity breathing for "still working" indicators.
    pub(crate) const BREATHING: Self = Self {
        period: Duration::from_millis(1600),
    };

    /// One cycle of the orb lattice.
    pub(crate) const ORB_LATTICE: Self = Self {
        period: Duration::from_millis(1700),
    };

    /// The cycle in whole milliseconds, for choreography that phases in
    /// integer beats.
    pub(crate) const fn period_millis(self) -> u64 {
        self.period.as_millis() as u64
    }

    /// This loop as a repeating GPUI animation.
    pub(crate) fn looping(self) -> Animation {
        repeating(self.period)
    }

    /// This loop as a repeating animation phase-locked to the application's
    /// shared epoch, for a region clock whose siblings should tick together.
    pub(crate) fn looping_synced(self) -> Animation {
        repeating_synced(self.period)
    }
}

/// A loop over `period`.
///
/// Every repeating effect in the crate is built here, so all of them inherit
/// GPUI's reduced-motion contract by construction: a repeating animation is
/// held at its first frame and schedules nothing. No call site re-implements
/// that check.
fn repeating(period: Duration) -> Animation {
    Animation::new(period).repeat()
}

/// A loop over `period`, phase-locked against `App`'s shared animation epoch
/// rather than the element's own mount instant.
///
/// For region clocks: two clusters mounted at different moments sample the
/// same phase, so side-by-side instances tick together instead of beating
/// against each other. The reduced-motion contract is unchanged — a held
/// animation is held whichever clock it reads.
fn repeating_synced(period: Duration) -> Animation {
    Animation::new(period).repeat_synced()
}

/// How much of the described motion actually runs.
///
/// Resolution follows the crate's customization order: this default → the
/// application's installed policy → per-surface overrides where a
/// component exposes one. The OS reduce-motion signal composes with the
/// policy and can only restrict: under reduce motion the effective
/// preference is at least [`Crossfade`](Self::Crossfade), never less —
/// the policy may choose [`Snap`](Self::Snap), but nothing wins motion
/// back from the OS signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum MotionPreference {
    /// Everything the policy describes: travel, springs, and loops.
    #[default]
    Full,
    /// Comprehension-aiding opacity only, at the quick tempo: no travel,
    /// no springs, no stagger delays, and no repeating loops beyond
    /// progress indication.
    Crossfade,
    /// State changes land in one frame.
    Snap,
}

/// A spring's feel, as the pair the ecosystem tunes springs by: the response
/// period and the damping ratio.
///
/// `response` is the period one full oscillation would take undamped — the
/// scale the motion is felt at, not the moment it stops. `damping` is the
/// ratio ζ: `1.0` approaches without overshoot, below it passes the target
/// and comes back, above it approaches slowly. These are the two numbers
/// `gpui_base::motion::Spring` is built from, so a role converts to a
/// running spring without translation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringRole {
    response: Duration,
    damping: f32,
}

impl SpringRole {
    const fn new(response: Duration, damping: f32) -> Self {
        Self { response, damping }
    }

    /// The response period the role is felt at.
    pub const fn response(self) -> Duration {
        self.response
    }

    /// The damping ratio ζ; `1.0` means no overshoot.
    pub const fn damping(self) -> f32 {
        self.damping
    }
}

/// The application-global motion policy: every duration, stagger beat, and
/// spring response the crate animates with, resolved through one value.
///
/// [`crate::init`] installs [`MotionTokens::DEFAULT`] unless the application
/// already chose. To customize, build from `default()` and [`set`](Self::set)
/// the result — before windows render, so no frame is timed under two
/// policies:
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use std::time::Duration;
/// # fn example(cx: &mut gpui::App) {
/// gpui_ai::init(cx);
/// gpui_ai::motion::MotionTokens::default()
///     .with_quick(Duration::from_millis(120))
///     .set(cx);
/// # }
/// ```
///
/// Components use the quick role for acknowledgments, standard for entrances
/// and compact disclosures, and spring roles for bounded retargets. Named
/// repeating effects keep their individual tempos. The deliberate role is
/// available for application composition without adding motion implicitly.
///
/// What the policy cannot do: override reduced motion. `cx.reduce_motion()`
/// stays authoritative in every consumer, so customization tunes tempo and
/// feel, never whether travel happens for someone who asked for stillness.
#[derive(Debug, Clone, PartialEq)]
pub struct MotionTokens {
    preference: MotionPreference,
    instant: Duration,
    quick: Duration,
    hover_glide: Duration,
    standard: Duration,
    deliberate: Duration,
    stagger_beat: Duration,
    stagger_cap: usize,
    press: SpringRole,
    selection: SpringRole,
    disclosure: SpringRole,
    reflow: SpringRole,
    shimmer: ProgressLoopSpec,
    grid_sweep: ProgressLoopSpec,
    image_pulse: ProgressLoopSpec,
    status_spinner: ProgressLoopSpec,
    breathing: AmbientLoopSpec,
    orb_lattice: AmbientLoopSpec,
}

impl Global for MotionTokens {}

impl Default for MotionTokens {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// How many items may take part in one arrival cascade. The bound is the
/// bounded-choreography rule, not a tunable: a longer list arrives with the
/// last beat rather than queueing behind it.
const STAGGER_PARTICIPANTS: usize = 6;

/// The most aggregate delay one cascade may spread across its participants.
const STAGGER_TOTAL_CAP: Duration = Duration::from_millis(220);

impl MotionTokens {
    /// The crate's default duration, spring, and repeating-effect policy.
    pub const DEFAULT: Self = Self {
        preference: MotionPreference::Full,
        instant: Duration::ZERO,
        quick: Duration::from_millis(150),
        // Bench-tuned: 150ms read as lag behind the pointer; 100ms tracks.
        hover_glide: Duration::from_millis(100),
        standard: EnterSpec::REVEAL.duration,
        deliberate: Duration::from_millis(380),
        stagger_beat: EnterSpec::REVEAL.stagger,
        stagger_cap: EnterSpec::REVEAL.stagger_cap,
        press: SpringRole::new(Duration::from_millis(140), 1.0),
        selection: SpringRole::new(Duration::from_millis(220), 1.0),
        disclosure: SpringRole::new(Duration::from_millis(280), 1.0),
        // The response the shipped FilterTable reorder proved in 0.2.x,
        // adopted as the role's value when that reorder became the role's
        // first consumer.
        reflow: SpringRole::new(Duration::from_millis(180), 1.0),
        shimmer: ProgressLoopSpec::SHIMMER,
        grid_sweep: ProgressLoopSpec::GRID_SWEEP,
        image_pulse: ProgressLoopSpec::IMAGE_PULSE,
        status_spinner: ProgressLoopSpec::STATUS_SPINNER,
        breathing: AmbientLoopSpec::BREATHING,
        orb_lattice: AmbientLoopSpec::ORB_LATTICE,
    };

    /// The policy the application installed, or the crate's default where it
    /// installed none. Never panics: a window rendered before `init` runs is
    /// timed by [`Self::DEFAULT`], the same values `init` would install.
    pub fn read(cx: &App) -> &Self {
        cx.try_global::<Self>().unwrap_or(&Self::DEFAULT)
    }

    /// Makes this policy the application's, for every component at once.
    pub fn set(self, cx: &mut App) {
        cx.set_global(self);
    }

    // -- Duration roles ----------------------------------------------------

    /// Focus, press acknowledgement, pointer capture: zero, because a direct
    /// response to input is not eased.
    pub const fn instant(&self) -> Duration {
        self.instant
    }

    /// Icon or label swap, copy acknowledgement, close.
    pub const fn quick(&self) -> Duration {
        self.quick
    }

    /// One hover highlight gliding between uniform rows, chasing the
    /// pointer on the strong ease-out. Faster than [`quick`](Self::quick)
    /// because the highlight follows input rather than acknowledging it;
    /// any reduced preference snaps the highlight instead.
    pub const fn hover_glide(&self) -> Duration {
        self.hover_glide
    }

    /// Selected indicator, compact disclosure, progress retarget — and the
    /// crate's entrance tempo.
    pub const fn standard(&self) -> Duration {
        self.standard
    }

    /// Larger bounded panel or success choreography.
    ///
    /// Restricted by contract to modal- and sheet-class surfaces and
    /// explanatory composition: nothing smaller — tooltips, popovers,
    /// dropdowns, toggles, row state — may bind to it, because 380ms on
    /// an element the reader is waiting on reads as lag, not weight.
    /// Popover-class entrances bind to [`quick`](Self::quick).
    pub const fn deliberate(&self) -> Duration {
        self.deliberate
    }

    // -- Spring roles ------------------------------------------------------

    /// Reversible control compression: fast and fully damped.
    pub const fn press_spring(&self) -> SpringRole {
        self.press
    }

    /// An active pill or marker gliding between targets.
    pub const fn selection_spring(&self) -> SpringRole {
        self.selection
    }

    /// Bounded panel geometry; no bounce, so it also suits destructive and
    /// error surfaces.
    pub const fn disclosure_spring(&self) -> SpringRole {
        self.disclosure
    }

    /// Visible row displacement: critically damped, velocity-preserving.
    pub const fn reflow_spring(&self) -> SpringRole {
        self.reflow
    }

    // -- Builders ----------------------------------------------------------

    /// The policy's own motion preference, before the OS signal composes.
    pub const fn preference(&self) -> MotionPreference {
        self.preference
    }

    /// The preference actually in effect: the policy's, floored at
    /// [`MotionPreference::Crossfade`] while the OS asks for reduced
    /// motion. Every clock, spring, and loop in the crate resolves
    /// through this.
    pub fn effective_preference(cx: &App) -> MotionPreference {
        let policy = Self::read(cx).preference;
        if cx.reduce_motion() {
            policy.max(MotionPreference::Crossfade)
        } else {
            policy
        }
    }

    /// Replaces the [`preference`](Self::preference).
    pub const fn with_preference(mut self, preference: MotionPreference) -> Self {
        self.preference = preference;
        self
    }

    /// Replaces the [`instant`](Self::instant) duration.
    pub const fn with_instant(mut self, duration: Duration) -> Self {
        self.instant = duration;
        self
    }

    /// Replaces the [`quick`](Self::quick) duration.
    pub const fn with_quick(mut self, duration: Duration) -> Self {
        self.quick = duration;
        self
    }

    /// Replaces the [`hover_glide`](Self::hover_glide) duration.
    pub const fn with_hover_glide(mut self, duration: Duration) -> Self {
        self.hover_glide = duration;
        self
    }

    /// Replaces the [`standard`](Self::standard) duration, which is also the
    /// entrance tempo every reveal in the crate settles at.
    pub const fn with_standard(mut self, duration: Duration) -> Self {
        self.standard = duration;
        self
    }

    /// Replaces the [`deliberate`](Self::deliberate) duration.
    pub const fn with_deliberate(mut self, duration: Duration) -> Self {
        self.deliberate = duration;
        self
    }

    /// Replaces the per-item stagger beat. The participation and aggregate
    /// caps are policy, not configuration, and stay where they are.
    pub const fn with_stagger_beat(mut self, beat: Duration) -> Self {
        self.stagger_beat = beat;
        self
    }

    /// Replaces the press spring's feel.
    pub const fn with_press_spring(mut self, response: Duration, damping: f32) -> Self {
        self.press = SpringRole::new(response, damping);
        self
    }

    /// Replaces the selection spring's feel.
    pub const fn with_selection_spring(mut self, response: Duration, damping: f32) -> Self {
        self.selection = SpringRole::new(response, damping);
        self
    }

    /// Replaces the disclosure spring's feel.
    pub const fn with_disclosure_spring(mut self, response: Duration, damping: f32) -> Self {
        self.disclosure = SpringRole::new(response, damping);
        self
    }

    /// Replaces the reflow spring's feel.
    pub const fn with_reflow_spring(mut self, response: Duration, damping: f32) -> Self {
        self.reflow = SpringRole::new(response, damping);
        self
    }

    // -- Crate-internal readers --------------------------------------------

    /// The entrance spec every reveal resolves through: the standard tempo,
    /// the stagger beat and cap, and the crate's fixed travel distance.
    pub(crate) const fn reveal(&self) -> EnterSpec {
        EnterSpec {
            duration: self.standard,
            stagger: self.stagger_beat,
            stagger_cap: self.stagger_cap,
            rise: EnterSpec::REVEAL.rise,
        }
    }

    /// Label shimmer tempo.
    pub(crate) const fn shimmer(&self) -> ProgressLoopSpec {
        self.shimmer
    }

    /// Pixel-grid loader sweep tempo.
    pub(crate) const fn grid_sweep(&self) -> ProgressLoopSpec {
        self.grid_sweep
    }

    /// Image placeholder pulse tempo.
    pub(crate) const fn image_pulse(&self) -> ProgressLoopSpec {
        self.image_pulse
    }

    /// In-flight status icon rotation tempo.
    pub(crate) const fn status_spinner(&self) -> ProgressLoopSpec {
        self.status_spinner
    }

    /// Opacity-breathing tempo.
    pub(crate) const fn breathing(&self) -> AmbientLoopSpec {
        self.breathing
    }

    /// Orb lattice cycle tempo.
    pub(crate) const fn orb_lattice(&self) -> AmbientLoopSpec {
        self.orb_lattice
    }

    /// The delay item `index` of `arriving` newly visible items waits before
    /// its arrival plays — the stagger role.
    ///
    /// The cascade decelerates: delays are spread with ease-out spacing, so
    /// later items land closer together and the group resolves quickly
    /// instead of trailing. At most six items take part, the whole cascade
    /// spends at most 220 ms, and items past the participation bound arrive
    /// with the last beat rather than after it — the bounded-choreography
    /// caps, which are policy rather than configuration. The per-item beat
    /// scales with [`with_stagger_beat`](Self::with_stagger_beat).
    ///
    /// Provisional like the other unconsumed roles: the motion lab is its
    /// first consumer and its validator, and the shipped `reveal_staggered`
    /// keeps its documented linear beats regardless.
    pub fn arrival_stagger(&self, index: usize, arriving: usize) -> Duration {
        let last = arriving.min(STAGGER_PARTICIPANTS).saturating_sub(1);
        if last == 0 {
            return Duration::ZERO;
        }
        let natural = self.stagger_beat * last as u32;
        let total = natural.min(STAGGER_TOTAL_CAP);
        let position = index.min(last) as f32 / last as f32;
        // Ease-out spacing: the derivative shrinks as position grows, so
        // each gap is smaller than the one before it.
        let eased = position * (2.0 - position);
        total.mul_f32(eased)
    }
}

/// Installs [`MotionTokens::DEFAULT`] unless the application already chose a
/// policy — so `init` never overwrites a customization made before it ran,
/// and a customization made after simply replaces the default.
pub(crate) fn install(cx: &mut App) {
    if !cx.has_global::<MotionTokens>() {
        cx.set_global(MotionTokens::DEFAULT);
    }
}

/// The open/close channel every animated disclosure in the crate samples.
///
/// One retargetable transition from closed (`0.0`) to open (`1.0`) on the
/// standard role: rapid toggling resumes from the current sample rather than
/// restarting, content changes leave a settled channel untouched, and
/// reduced motion snaps to the target — GPUI's transition contract, not
/// per-component policy. Callers cross-fade their body on the returned
/// progress (opacity plus a small token-derived lift) while open. Closed
/// bodies leave the tree immediately: opacity does not suppress focus,
/// input, or accessibility. Headers can retain the closing transition.
/// Callers never animate the body's height,
/// because rich content re-measured per frame is the layout loop the motion
/// plan forbids.
///
/// Frame demand: only while the channel is travelling. Settled open it
/// requests nothing; settled closed the caller unmounts the body.
pub(crate) fn disclosure_progress(
    id: impl Into<gpui_base::motion::TransitionId>,
    open: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let target = if open { 1.0f32 } else { 0.0 };
    if !motion_is_full(cx) {
        // Geometry is travel: a reduced preference lands the surface in
        // one frame and leaves the fade to `disclosure_fade`.
        return target;
    }
    let standard = MotionTokens::read(cx).standard();
    transition(
        id,
        target,
        Transition::new(standard).ease(ease_out_quint),
        window,
        cx,
    )
}

/// Reveals a disclosure body by easing its height, not just its opacity.
///
/// A body that appears at full height and merely fades reads as a snap
/// however long the fade takes — the 0.4.0 feel review's first finding.
/// This clips the body to a fraction of its own measured height, so the
/// surface grows into the space it is about to occupy.
///
/// The body's natural height is measured from the content itself, which
/// keeps its full size inside the clip; the measurement writes no
/// notification, so a settled disclosure schedules nothing. Until a first
/// measurement exists the body renders unclipped, which is what a
/// reduced preference wants anyway: `progress` is already 1.0 there.
pub(crate) fn disclosure_clip(
    id: impl Into<ElementId>,
    progress: f32,
    content: impl gpui::IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> gpui::Div {
    use gpui::prelude::FluentBuilder as _;
    use gpui::{ParentElement as _, Styled as _};
    use gpui_base::ElementExt as _;

    let id = id.into();
    let measured = window.use_keyed_state((id.clone(), "disclosure-height"), cx, |_, _| {
        None::<gpui::Pixels>
    });
    let height = *measured.read(cx);
    let clipping = progress < 1.0;
    // A body that unmounts while closed is measured again on the way back,
    // and the measuring pass happens during prepaint — after this render
    // has already sized the clip. Until a height is known, a clipping body
    // is held closed rather than drawn at its full size: opening would
    // otherwise flash the whole body for one frame and then collapse it,
    // which is a worse pop than the one this replaced.
    let clipped = match (height, clipping) {
        (Some(height), true) => Some(height * progress.max(0.0)),
        (None, true) => Some(gpui::Pixels::ZERO),
        _ => None,
    };
    let recorder = measured;
    gpui::div()
        .w_full()
        .overflow_hidden()
        .when_some(clipped, |body, height| body.h(height))
        .child(
            gpui::div()
                .w_full()
                .flex_none()
                .on_prepaint(move |bounds, _, cx| {
                    // The content keeps its natural height inside the clip,
                    // so this reads the same number whether the body is
                    // mid-reveal or settled — and a body that unmounts
                    // while closed measures itself again on the way back.
                    // Geometry bookkeeping only: no notify, so a settled
                    // disclosure schedules nothing for having been measured.
                    recorder.update(cx, |measured, _| *measured = Some(bounds.size.height));
                })
                .child(content),
        )
}

/// The opacity half of a disclosure: identical to
/// [`disclosure_progress`] under full motion (same transition key, so the
/// two never disagree), a quick crossfade on its own clock under the
/// crossfade preference — the upstream transition would snap it whenever
/// the OS flag is set — and the target under snap.
pub(crate) fn disclosure_fade(
    id: impl Into<gpui_base::motion::TransitionId>,
    open: bool,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let target = if open { 1.0f32 } else { 0.0 };
    match MotionTokens::effective_preference(cx) {
        MotionPreference::Snap => target,
        MotionPreference::Crossfade => {
            let id = id.into();
            crossfade_toward(
                ElementId::Name(format!("{id:?}-disclosure-fade").into()),
                target,
                window,
                cx,
            )
        }
        MotionPreference::Full => disclosure_progress(id, open, window, cx),
    }
}

/// Arrival bookkeeping for one owner. History starts at rest; later batches
/// animate at most six current identities. The owner retains the clocks, so
/// temporarily unmounting a child cannot restart its arrival. Removed items
/// and settled clocks are retired instead of accumulating lifetime history.
pub(crate) struct ArrivalRoster {
    primed: bool,
    epoch: bool,
    seen: HashMap<ElementId, bool>,
    delays: HashMap<ElementId, ArrivalTiming>,
}

struct ArrivalTiming {
    started_at: Instant,
    delay: Duration,
    duration: Duration,
}

impl ArrivalTiming {
    fn progress(&self, now: Instant) -> f32 {
        let elapsed = now.saturating_duration_since(self.started_at);
        if self.duration.is_zero() {
            1.0
        } else if elapsed < self.delay {
            0.0
        } else {
            (elapsed.saturating_sub(self.delay).as_secs_f32() / self.duration.as_secs_f32())
                .min(1.0)
        }
    }
}

impl ArrivalRoster {
    pub(crate) fn new() -> Self {
        Self {
            primed: false,
            epoch: false,
            seen: HashMap::new(),
            delays: HashMap::new(),
        }
    }

    /// Takes the roll call of this render's identities, assigning one
    /// decelerating cascade to those not seen before when `assign` holds.
    pub(crate) fn note(
        &mut self,
        keys: impl Iterator<Item = ElementId>,
        assign: bool,
        tokens: &MotionTokens,
        now: Instant,
    ) {
        self.epoch = !self.epoch;
        let mut fresh = Vec::new();
        for key in keys {
            if self.seen.insert(key.clone(), self.epoch).is_none()
                && fresh.len() < STAGGER_PARTICIPANTS
            {
                fresh.push(key);
            }
        }
        self.seen.retain(|_, epoch| *epoch == self.epoch);
        self.delays.retain(|key, timing| {
            assign && self.seen.contains_key(key) && timing.progress(now) < 1.0
        });
        let primed = std::mem::replace(&mut self.primed, true);
        let cascade = primed && assign;
        fresh.truncate(STAGGER_PARTICIPANTS.saturating_sub(self.delays.len()));
        let batch = fresh.len();
        for (position, key) in fresh.into_iter().enumerate() {
            if cascade {
                self.delays.insert(
                    key,
                    ArrivalTiming {
                        started_at: now,
                        delay: tokens.arrival_stagger(position, batch),
                        duration: tokens.standard(),
                    },
                );
            }
        }
    }

    /// Samples the owner's clock, retiring it on completion or reduced
    /// motion. Only a live sample asks for another frame.
    pub(crate) fn progress(
        &mut self,
        key: &ElementId,
        window: &mut Window,
        cx: &App,
    ) -> Option<f32> {
        let progress = self
            .delays
            .get(key)?
            .progress(cx.background_executor().now());
        if MotionTokens::effective_preference(cx) == MotionPreference::Snap || progress >= 1.0 {
            self.delays.remove(key);
            return None;
        }
        note_reveal_frame_request();
        window.request_animation_frame();
        Some(ease_out_quint(progress))
    }

    #[cfg(test)]
    pub(crate) fn delay(&self, key: &ElementId) -> Option<Duration> {
        self.delays.get(key).map(|timing| timing.delay)
    }
}

/// Text with a soft highlight travelling across it — the ecosystem's
/// "something is happening" label treatment for thinking, running tool
/// groups, and streaming plans.
///
/// The base layer is muted text; a clipped bright copy sweeps over it. When
/// [`Shimmer::active`] is false (or reduced motion is on) the label renders
/// as plain muted text, so meaning never depends on the motion.
#[derive(IntoElement)]
pub struct Shimmer {
    id: ElementId,
    style: StyleRefinement,
    text: SharedString,
    active: bool,
    base: Option<Hsla>,
    highlight: Option<Hsla>,
}

impl Shimmer {
    /// Creates an active shimmer label.
    pub fn new(id: impl Into<ElementId>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            text: text.into(),
            active: true,
            base: None,
            highlight: None,
        }
    }

    /// Sets whether the highlight travels. Inactive labels are plain text.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Overrides the resting and highlight colors (defaults: muted foreground
    /// and foreground).
    pub fn colors(mut self, base: Hsla, highlight: Hsla) -> Self {
        self.base = Some(base);
        self.highlight = Some(highlight);
        self
    }
}

impl Styled for Shimmer {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

// Travel during the first part of the cycle, then rest fully off the trailing
// edge. The start is relative to the label width, as is SHIMMER_BAND.
fn shimmer_band_start(delta: f32, duty: f32) -> f32 {
    let progress = ease_in_out((delta / duty).min(1.0));
    -SHIMMER_BAND + progress * (1.0 + SHIMMER_BAND)
}

impl RenderOnce for Shimmer {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let base = self.base.unwrap_or(cx.theme().muted_foreground);
        let highlight = self.highlight.unwrap_or(cx.theme().foreground);
        if !self.active || !motion_is_full(cx) {
            return div()
                .whitespace_nowrap()
                .text_color(base)
                .child(self.text)
                .refine_style(&self.style)
                .into_any_element();
        }

        let text = self.text;
        let spec = MotionTokens::read(cx).shimmer();
        div()
            .relative()
            .overflow_hidden()
            .whitespace_nowrap()
            .text_color(base)
            .child(text.clone())
            .refine_style(&self.style)
            .with_visible_animation(
                (self.id, "shimmer"),
                // Frame demand: active while the caller's work is in flight,
                // which is what `active` reports; the caller stops passing
                // `active(true)` when the work ends, so the loop never runs
                // over a settled surface. Reduced motion holds delta at 0 —
                // the band sits fully off the leading edge and the label is
                // plain muted text.
                spec.looping(),
                move |container, delta| {
                    let start = shimmer_band_start(delta, spec.duty);
                    container.child(
                        div()
                            .absolute()
                            .top_0()
                            .bottom_0()
                            .left(relative(start))
                            .w(relative(SHIMMER_BAND))
                            .overflow_hidden()
                            .child(
                                div()
                                    .absolute()
                                    .top_0()
                                    // Counter-offset so the bright copy stays
                                    // aligned with the base text underneath.
                                    .left(relative(-start / SHIMMER_BAND))
                                    .whitespace_nowrap()
                                    .text_color(highlight)
                                    .child(text.clone()),
                            ),
                    )
                },
            )
            .into_any_element()
    }
}

/// Progress of a one-shot reveal keyed by `id`, from `0.0` (just mounted) to
/// `1.0` (settled), starting the clock the first time the element renders.
///
/// The start instant lives in keyed element state, so a row that keeps its
/// stable identity across renders plays the reveal only once; it requests
/// animation frames until it settles and returns `1.0` immediately under
/// reduced motion.
pub fn reveal_progress(
    id: impl Into<ElementId>,
    delay: Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let duration = MotionTokens::read(cx).reveal().duration;
    timed_progress(id, delay, duration, window, cx)
}

/// One-shot decay clock for a released press, over the press spring's
/// response. Runs 0→1 from the first frame its key exists; callers paint
/// the press tint at `1.0 - progress`, so a fresh key replays the release
/// once and re-renders replay nothing. Pressing itself never rides this —
/// the pressed style applies on pointer-down, instantly, and only the way
/// back out is staged.
pub(crate) fn press_release_progress(
    id: impl Into<ElementId>,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let response = MotionTokens::read(cx).press_spring().response();
    timed_progress(id, Duration::ZERO, response, window, cx)
}

/// Progress of a one-shot status swap keyed by `id`, at the quick role's
/// tempo — the acknowledgment an icon or label change settles in on.
pub(crate) fn swap_progress(id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> f32 {
    let duration = MotionTokens::read(cx).quick();
    timed_progress(id, Duration::ZERO, duration, window, cx)
}

/// One-shot acknowledgment of the state a fixed slot has settled into.
///
/// Observe the slot on every render, including states that do not apply the
/// returned opacity. The initial state is settled; each subsequent change
/// starts one quick acknowledgment, including a return to the initial state.
/// One retained slot owns the clock rather than allocating a key per value.
/// Running faces may ignore the sample so their spinner remains unmodified.
pub(crate) fn acknowledged_state(
    slot: ElementId,
    ordinal: u64,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let now = cx.background_executor().now();
    let duration = MotionTokens::read(cx).quick();
    let reduced =
        MotionTokens::effective_preference(cx) == MotionPreference::Snap || duration.is_zero();
    let state =
        window.use_keyed_state((slot, "acknowledged-state"), cx, |_, _| AcknowledgedState {
            ordinal,
            started_at: None,
        });
    let progress = state.update(cx, |state, _| {
        if state.ordinal != ordinal {
            state.ordinal = ordinal;
            state.started_at = Some(now);
        }
        if reduced {
            state.started_at = None;
        }
        let progress = state.started_at.map_or(1.0, |started| {
            (now.saturating_duration_since(started).as_secs_f32() / duration.as_secs_f32()).min(1.0)
        });
        if progress >= 1.0 {
            state.started_at = None;
        }
        progress
    });
    if progress < 1.0 {
        note_reveal_frame_request();
        window.request_animation_frame();
    }
    ease_out_quint(progress)
}

struct AcknowledgedState {
    ordinal: u64,
    started_at: Option<Instant>,
}

/// `1.0` while travel is allowed, `0.0` under crossfade and snap.
///
/// Multiplied into every offset a progress value drives, so the same
/// sample fades under a reduced preference instead of travelling — the
/// "gentler, not zero" reading both review standards prescribe.
pub(crate) fn travel(cx: &App) -> f32 {
    if MotionTokens::effective_preference(cx) == MotionPreference::Full {
        1.0
    } else {
        0.0
    }
}

/// Whether springs, reorders, drives, and decorative loops may run at
/// all. Progress indication (an indeterminate spinner) is exempt from
/// crossfade by contract and follows only snap and the OS signal.
pub(crate) fn motion_is_full(cx: &App) -> bool {
    MotionTokens::effective_preference(cx) == MotionPreference::Full
}

/// The duration a geometry retarget (fills, widths, disclosure travel)
/// actually animates over: unchanged under full motion, zero otherwise —
/// geometry is travel, and gpui_base's transition snaps on a zero
/// duration.
pub(crate) fn retarget_duration(cx: &App, duration: Duration) -> Duration {
    if motion_is_full(cx) {
        duration
    } else {
        Duration::ZERO
    }
}

/// A crossfade's own retargeting clock: steps `value` toward `target` at
/// the quick tempo, carrying the current value through interruptions.
///
/// The upstream transition facility snaps whenever the OS asks for
/// reduced motion — correct for travel, wrong for the opacity fades the
/// crossfade preference exists to keep. This clock ignores the OS flag
/// on purpose; callers reach it only through a preference match that has
/// already resolved to crossfade.
fn crossfade_toward(
    id: impl Into<ElementId>,
    target: f32,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    let quick = MotionTokens::read(cx)
        .quick()
        .as_secs_f32()
        .max(f32::EPSILON);
    let now = cx.background_executor().now();
    let state = window.use_keyed_state((id.into(), "crossfade"), cx, |_, _| CrossfadeState {
        value: target,
        at: now,
    });
    let value = state.update(cx, |state, _| {
        let step = now.saturating_duration_since(state.at).as_secs_f32() / quick;
        state.at = now;
        state.value = if state.value < target {
            (state.value + step).min(target)
        } else {
            (state.value - step).max(target)
        };
        state.value
    });
    if (value - target).abs() > f32::EPSILON {
        note_reveal_frame_request();
        window.request_animation_frame();
    }
    value
}

struct CrossfadeState {
    value: f32,
    at: Instant,
}

/// The shared clock behind [`reveal_progress`] and [`swap_progress`]: `0.0`
/// at the keyed instant the element first rendered, `1.0` once `duration`
/// (after `delay`) has passed, eased on the way.
fn timed_progress(
    id: impl Into<ElementId>,
    delay: Duration,
    duration: Duration,
    window: &mut Window,
    cx: &mut App,
) -> f32 {
    // Frame demand: the only hand-scheduled effects in the crate, because
    // these read a clock rather than a GPUI animation. Active while
    // `progress < 1.0`; a settled clock asks for nothing. Snap returns
    // the end state; crossfade keeps the fade — clamped to the quick
    // tempo, stagger delay dropped — because a one-shot opacity change is
    // exactly the comprehension aid a reduced preference retains.
    let (delay, duration) = match MotionTokens::effective_preference(cx) {
        MotionPreference::Snap => return 1.0,
        MotionPreference::Crossfade => {
            (Duration::ZERO, duration.min(MotionTokens::read(cx).quick()))
        }
        MotionPreference::Full => (delay, duration),
    };
    let now = cx.background_executor().now();
    let started = *window.use_keyed_state(id, cx, |_, _| now).read(cx);
    let elapsed = now.saturating_duration_since(started);
    let progress = if elapsed <= delay {
        0.0
    } else {
        (elapsed.saturating_sub(delay).as_secs_f32() / duration.as_secs_f32()).min(1.0)
    };
    if progress < 1.0 {
        note_reveal_frame_request();
        window.request_animation_frame();
    }
    ease_out_quint(progress)
}

#[cfg(test)]
thread_local! {
    /// Animation frames reveals have asked for on this thread.
    ///
    /// Thread-local rather than global: the harness runs each test on its own
    /// thread, and a shared counter would report another test's frames.
    static REVEAL_FRAME_REQUESTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Records one animation frame requested by a reveal.
///
/// Frame demand is the property under audit, so it is counted at the single
/// site that creates it instead of inferred from progress values. Nothing
/// outside tests compiles the counter.
#[inline]
fn note_reveal_frame_request() {
    #[cfg(test)]
    REVEAL_FRAME_REQUESTS.with(|count| count.set(count.get().saturating_add(1)));
}

/// Reveal frames requested since the last call, and resets the counter.
///
/// `pub(crate)` so component tests can audit their own reveal demand — a
/// loaded trace proving it revealed nothing reads this, not a proxy.
#[cfg(test)]
pub(crate) fn take_reveal_frame_requests() -> usize {
    REVEAL_FRAME_REQUESTS.with(|count| count.replace(0))
}

/// Fades and lifts an element into place once when it first mounts.
///
/// Keyed by `id`: a row that keeps its stable identity across renders plays
/// the reveal only on its first frames, never on every content update.
pub fn reveal<E>(element: E, id: impl Into<ElementId>, window: &mut Window, cx: &mut App) -> E
where
    E: Styled,
{
    apply_reveal(
        element,
        reveal_progress(id, Duration::ZERO, window, cx),
        travel(cx),
    )
}

/// Like [`reveal`], but item `index` waits `index` stagger beats before it
/// starts, so a list of chips or rows ripples into place.
pub fn reveal_staggered<E>(
    element: E,
    id: impl Into<ElementId>,
    index: usize,
    window: &mut Window,
    cx: &mut App,
) -> E
where
    E: Styled,
{
    let delay = MotionTokens::read(cx).reveal().stagger_delay(index);
    apply_reveal(element, reveal_progress(id, delay, window, cx), travel(cx))
}

/// Reveal travel from rest, in pixels.
///
/// A derived alias carrying no number of its own: the pixel-discipline gate
/// pins the displacement call site below by its exact expression, so the
/// distance stays a named local rather than an inline field access.
const REVEAL_RISE: f32 = EnterSpec::REVEAL.rise;

fn apply_reveal<E: Styled>(element: E, progress: f32, travel: f32) -> E {
    element
        .opacity(progress)
        .top(px(REVEAL_RISE * (1.0 - progress) * travel))
}

/// Slowly breathes an element's opacity — for indicators that mean "still
/// working" without a measurable progress value.
///
/// Breathes at the crate's default ambient tempo: this signature predates
/// [`MotionTokens`] and carries no `App`, so it has no way to see a replaced
/// policy. Components inside the crate route through the policy instead;
/// under the default tokens the two are identical.
///
/// This low-level compatibility helper returns GPUI's `AnimationElement`;
/// callers must unmount it while offscreen. The library's composed looping
/// components additionally suspend when clipped by a scroll container.
pub fn breathing<E>(element: E, id: impl Into<ElementId>) -> AnimationElement<E>
where
    E: IntoElement + Styled + 'static,
{
    breathing_with(MotionTokens::DEFAULT.breathing(), element, id)
}

/// [`breathing`], at the tempo the given policy entry sets.
pub(crate) fn breathing_with<E>(
    spec: AmbientLoopSpec,
    element: E,
    id: impl Into<ElementId>,
) -> AnimationElement<E>
where
    E: IntoElement + Styled + 'static,
{
    element.with_animation(
        id,
        // Frame demand: ambient — active for as long as the caller keeps the
        // element mounted, which is the "still working, nothing to report"
        // state itself, so there is no settled frame to reach. Reduced
        // motion holds delta at 0, the middle of the opacity range.
        spec.looping().with_easing(pulsating_between(0.35, 1.0)),
        |element, alpha| element.opacity(alpha),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, WindowHandle, px, size};
    use std::cell::Cell;
    use std::rc::Rc;

    #[test]
    fn shimmer_band_travels_fully_across_and_rests_off_the_trailing_edge() {
        let default_duty = ProgressLoopSpec::SHIMMER.duty;
        assert!(default_duty > 0.0 && default_duty < 1.0);
        // Exercise the renderer's geometry with both the default timing and a
        // shorter sweep. Expectations describe position, not the easing formula.
        for duty in [default_duty, 0.4] {
            let start = shimmer_band_start(0.0, duty);
            let quarter = shimmer_band_start(duty * 0.25, duty);
            let middle = shimmer_band_start(duty * 0.5, duty);
            let three_quarters = shimmer_band_start(duty * 0.75, duty);
            let end = shimmer_band_start(duty, duty);

            assert!(start + SHIMMER_BAND <= 0.0, "starts before the label");
            assert!(start < quarter && quarter < middle);
            assert!(middle < three_quarters && three_quarters < end);
            assert!(
                (middle + SHIMMER_BAND / 2.0 - 0.5).abs() < 1e-6,
                "the band crosses the label's center halfway through travel"
            );
            assert!(end >= 1.0, "clears the trailing edge");
            for delta in [duty + (1.0 - duty) * 0.5, 1.0] {
                assert_eq!(shimmer_band_start(delta, duty), end, "rests offscreen");
            }
        }
    }

    #[test]
    fn stagger_is_bounded_so_long_lists_do_not_wait_forever() {
        let spec = MotionTokens::DEFAULT.reveal();
        let far = spec.stagger_delay(12);
        let capped = spec.stagger_delay(100);
        assert_eq!(far, capped);
        assert!(spec.stagger_delay(1) < far);
    }

    #[test]
    fn default_policy_uses_the_documented_role_specs() {
        let tokens = MotionTokens::DEFAULT;
        assert_eq!(tokens.reveal(), EnterSpec::REVEAL);
        assert_eq!(tokens.shimmer(), ProgressLoopSpec::SHIMMER);
        assert_eq!(tokens.grid_sweep(), ProgressLoopSpec::GRID_SWEEP);
        assert_eq!(tokens.image_pulse(), ProgressLoopSpec::IMAGE_PULSE);
        assert_eq!(tokens.status_spinner(), ProgressLoopSpec::STATUS_SPINNER);
        assert_eq!(tokens.breathing(), AmbientLoopSpec::BREATHING);
        assert_eq!(tokens.orb_lattice(), AmbientLoopSpec::ORB_LATTICE);
        assert_eq!(tokens.standard(), EnterSpec::REVEAL.duration);
        assert_eq!(tokens.instant(), Duration::ZERO, "immediate is immediate");
        assert_eq!(MotionTokens::default(), tokens);
    }

    #[test]
    fn default_roles_have_ordered_tempos_and_safe_springs() {
        let tokens = MotionTokens::DEFAULT;
        assert!(tokens.quick() > Duration::ZERO);
        assert!(tokens.quick() < tokens.standard());
        assert!(tokens.standard() < tokens.deliberate());
        for spring in [
            tokens.press_spring(),
            tokens.selection_spring(),
            tokens.disclosure_spring(),
            tokens.reflow_spring(),
        ] {
            assert!(spring.response() > Duration::ZERO);
            assert!(spring.damping() > 0.0);
        }
        // Destructive-adjacent roles must not overshoot.
        assert!(tokens.press_spring().damping() >= 1.0);
        assert!(tokens.disclosure_spring().damping() >= 1.0);
        assert!(tokens.reflow_spring().damping() >= 1.0);
    }

    #[test]
    fn arrival_stagger_decelerates_within_its_caps() {
        let tokens = MotionTokens::DEFAULT;
        let delays: Vec<_> = (0..STAGGER_PARTICIPANTS)
            .map(|index| tokens.arrival_stagger(index, STAGGER_PARTICIPANTS))
            .collect();
        assert_eq!(delays[0], Duration::ZERO, "the first item leads at once");
        let gaps: Vec<_> = delays.windows(2).map(|pair| pair[1] - pair[0]).collect();
        for pair in gaps.windows(2) {
            assert!(
                pair[1] < pair[0],
                "the cascade must decelerate: gaps {gaps:?}"
            );
        }
        let last = *delays.last().expect("participants exist");
        assert!(last <= STAGGER_TOTAL_CAP);
        // Items past the participation bound arrive with the last beat, not
        // after it — a hundred-item load does not queue a hundred delays.
        assert_eq!(tokens.arrival_stagger(99, 100), last);
        // A single arrival has no cascade to join.
        assert_eq!(tokens.arrival_stagger(0, 1), Duration::ZERO);
    }

    #[test]
    fn arrival_roster_retires_removed_identities() {
        let mut roster = ArrivalRoster::new();
        for id in 0..1_000 {
            roster.note(
                std::iter::once(ElementId::Integer(id)),
                true,
                &MotionTokens::DEFAULT,
                Instant::now(),
            );
            assert!(
                roster.seen.len() <= 1 && roster.delays.len() <= 1,
                "a one-item surface must not retain the history of removed items"
            );
        }
    }

    #[test]
    fn arrival_roster_caps_overlapping_batches_and_retires_settled_clocks() {
        let mut roster = ArrivalRoster::new();
        let now = Instant::now();
        let tokens = MotionTokens::DEFAULT;
        roster.note(std::iter::empty(), true, &tokens, now);
        roster.note((0..1_000).map(ElementId::Integer), true, &tokens, now);
        assert_eq!(roster.delays.len(), STAGGER_PARTICIPANTS);
        roster.note((0..2_000).map(ElementId::Integer), true, &tokens, now);
        assert_eq!(roster.delays.len(), STAGGER_PARTICIPANTS);
        roster.note(
            (0..2_000).map(ElementId::Integer),
            true,
            &tokens,
            now + tokens.standard() + STAGGER_TOTAL_CAP,
        );
        assert!(roster.delays.is_empty(), "settled clocks must be retired");
    }

    struct AcknowledgmentProbe {
        ordinal: u64,
        sample: Rc<Cell<f32>>,
    }

    impl Render for AcknowledgmentProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.sample.set(acknowledged_state(
                "ack-probe".into(),
                self.ordinal,
                window,
                cx,
            ));
            div()
        }
    }

    #[gpui::test]
    fn acknowledgments_observe_every_change_without_replaying_snapshots(cx: &mut TestAppContext) {
        let sample = Rc::new(Cell::new(f32::NAN));
        let (probe, cx) = cx.add_window_view({
            let sample = sample.clone();
            move |_, _| AcknowledgmentProbe { ordinal: 0, sample }
        });
        draw(cx);
        assert_eq!(sample.get(), 1.0, "the first state is already presented");

        for ordinal in [1, 0, 1] {
            probe.update(cx, |probe, cx| {
                probe.ordinal = ordinal;
                cx.notify();
            });
            draw(cx);
            assert_eq!(sample.get(), 0.0, "every controlled change acknowledges");
            cx.executor().advance_clock(MotionTokens::DEFAULT.quick());
            assert_eq!(draw(cx), 0);
            assert_eq!(sample.get(), 1.0);
            assert_eq!(draw(cx), 0, "unchanged snapshots stay settled");
        }

        // The OS flag resolves to the crossfade preference: the
        // acknowledgment still fades — comprehension-aiding opacity is
        // kept — and settles at the quick tempo.
        cx.update(|_, cx| cx.set_reduce_motion(true));
        probe.update(cx, |probe, cx| {
            probe.ordinal = 2;
            cx.notify();
        });
        assert!(draw(cx) > 0, "the crossfade acknowledgment runs");
        cx.executor().advance_clock(MotionTokens::DEFAULT.quick());
        draw(cx);
        assert_eq!(sample.get(), 1.0);
        assert_eq!(draw(cx), 0, "and settles");

        // The snap preference is the true zero: end state, no frames.
        cx.update(|_, cx| {
            MotionTokens::default()
                .with_preference(MotionPreference::Snap)
                .set(cx)
        });
        probe.update(cx, |probe, cx| {
            probe.ordinal = 3;
            cx.notify();
        });
        assert_eq!(draw(cx), 0, "snap acknowledges in one frame");
        assert_eq!(sample.get(), 1.0);
        cx.update(|_, cx| {
            MotionTokens::default().set(cx);
            cx.set_reduce_motion(false);
        });
        assert_eq!(
            draw(cx),
            0,
            "leaving reduced motion must not replay history"
        );
    }

    #[gpui::test]
    fn install_respects_a_policy_the_application_chose_first(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let chosen = Duration::from_millis(10);
            MotionTokens::default().with_standard(chosen).set(cx);
            install(cx);
            assert_eq!(
                MotionTokens::read(cx).standard(),
                chosen,
                "init must not overwrite an earlier application choice"
            );
        });
    }

    #[gpui::test]
    fn install_provides_the_default_policy_when_none_was_chosen(cx: &mut TestAppContext) {
        cx.update(|cx| {
            assert_eq!(
                MotionTokens::read(cx),
                &MotionTokens::DEFAULT,
                "reads before install fall back to the same values"
            );
            install(cx);
            assert_eq!(MotionTokens::read(cx), &MotionTokens::DEFAULT);
        });
    }

    #[derive(Debug, Clone, Copy)]
    enum LoopLifecycle {
        Active,
        Inactive,
        Complete,
        Offscreen,
        Dropped,
    }

    struct LoopLifecycleProbe {
        lifecycle: LoopLifecycle,
    }

    impl Render for LoopLifecycleProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let animated = || {
                div().with_animation(
                    "lifecycle-loop",
                    ProgressLoopSpec::SHIMMER.looping(),
                    |element, _| element,
                )
            };
            match self.lifecycle {
                LoopLifecycle::Active => div().child(animated()),
                // GPUI schedules an animation during layout, before clipping.
                // Offscreen suspension therefore means the virtualized row or
                // host does not build the animated element at all.
                LoopLifecycle::Inactive
                | LoopLifecycle::Complete
                | LoopLifecycle::Offscreen
                | LoopLifecycle::Dropped => div().child(div()),
            }
        }
    }

    fn next_frame(window: &WindowHandle<LoopLifecycleProbe>, cx: &mut TestAppContext) -> usize {
        let callbacks = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the motion audit window should remain open");
        cx.run_until_parked();
        callbacks
    }

    fn set_lifecycle(
        lifecycle: LoopLifecycle,
        window: &WindowHandle<LoopLifecycleProbe>,
        cx: &mut TestAppContext,
    ) {
        window
            .update(cx, |probe, _, cx| {
                probe.lifecycle = lifecycle;
                cx.notify();
            })
            .expect("the motion audit window should remain open");
        cx.run_until_parked();
        // A callback already queued by the previous active frame may fire once,
        // but the new static tree must not replace it.
        next_frame(window, cx);
    }

    #[gpui::test]
    fn repeating_loops_request_frames_only_for_visible_active_work(cx: &mut TestAppContext) {
        let window = cx.open_window(size(px(120.), px(80.)), |_, _| LoopLifecycleProbe {
            lifecycle: LoopLifecycle::Active,
        });
        cx.run_until_parked();
        assert_eq!(next_frame(&window, cx), 1, "active work must keep moving");

        for lifecycle in [
            LoopLifecycle::Inactive,
            LoopLifecycle::Complete,
            LoopLifecycle::Offscreen,
            LoopLifecycle::Dropped,
        ] {
            set_lifecycle(lifecycle, &window, cx);
            assert_eq!(
                next_frame(&window, cx),
                0,
                "{lifecycle:?} content must settle without idle frame demand"
            );
        }

        set_lifecycle(LoopLifecycle::Active, &window, cx);
        assert_eq!(next_frame(&window, cx), 1);
    }

    #[gpui::test]
    fn reduced_motion_holds_a_repeating_loop_on_its_static_frame(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = cx.open_window(size(px(120.), px(80.)), |_, _| LoopLifecycleProbe {
            lifecycle: LoopLifecycle::Active,
        });
        cx.run_until_parked();
        assert_eq!(next_frame(&window, cx), 0);
    }

    /// Runs one reveal per draw and remembers what it returned.
    struct RevealProbe {
        delay: Duration,
        progress: Rc<Cell<f32>>,
    }

    impl Render for RevealProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            self.progress
                .set(reveal_progress("reveal-probe", self.delay, window, cx));
            div()
        }
    }

    fn reveal_probe(
        reduce_motion: bool,
        cx: &mut TestAppContext,
    ) -> (Rc<Cell<f32>>, &mut VisualTestContext) {
        let progress = Rc::new(Cell::new(f32::NAN));
        let (_, cx) = cx.add_window_view({
            let progress = progress.clone();
            move |_, _| RevealProbe {
                delay: Duration::ZERO,
                progress,
            }
        });
        cx.update(|_, cx| cx.set_reduce_motion(reduce_motion));
        // Opening the window may already have drawn; the audit starts here.
        take_reveal_frame_requests();
        (progress, cx)
    }

    fn draw(cx: &mut VisualTestContext) -> usize {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        take_reveal_frame_requests()
    }

    #[gpui::test]
    fn an_active_reveal_requests_a_frame_per_draw_until_it_settles(cx: &mut TestAppContext) {
        let (progress, cx) = reveal_probe(false, cx);

        assert_eq!(draw(cx), 1, "a reveal at zero progress must keep drawing");
        assert_eq!(progress.get(), 0.0);

        cx.executor().advance_clock(EnterSpec::REVEAL.duration / 2);
        assert_eq!(draw(cx), 1, "a half-played reveal must keep drawing");
        assert!(
            (0.0..1.0).contains(&progress.get()),
            "progress={}",
            progress.get()
        );
    }

    #[gpui::test]
    fn a_settled_reveal_requests_no_further_frames(cx: &mut TestAppContext) {
        let (progress, cx) = reveal_probe(false, cx);
        draw(cx);

        cx.executor().advance_clock(EnterSpec::REVEAL.duration);
        assert_eq!(draw(cx), 0, "the settling draw must be the last one");
        assert_eq!(progress.get(), 1.0);

        // Redraws for unrelated reasons must not restart the demand.
        assert_eq!(draw(cx), 0);
        assert_eq!(draw(cx), 0);
        assert_eq!(progress.get(), 1.0);
    }

    #[gpui::test]
    fn reduced_motion_crossfades_at_the_quick_tempo(cx: &mut TestAppContext) {
        // The OS flag no longer hard-snaps: it resolves to the crossfade
        // preference, which keeps the opacity fade — clamped to the quick
        // tempo, travel zeroed by `travel` — and settles quiet.
        let (progress, cx) = reveal_probe(true, cx);

        assert!(draw(cx) > 0, "the crossfade fade is live");
        assert!(progress.get() < 1.0);
        cx.update(|_, cx| assert_eq!(travel(cx), 0.0, "travel is zero under the flag"));

        cx.executor().advance_clock(MotionTokens::DEFAULT.quick());
        draw(cx);
        assert_eq!(progress.get(), 1.0);
        assert_eq!(draw(cx), 0, "a settled crossfade asks for nothing");
    }

    #[gpui::test]
    fn the_snap_preference_returns_the_end_state_without_frames(cx: &mut TestAppContext) {
        let (progress, cx) = reveal_probe(false, cx);
        cx.update(|_, cx| {
            MotionTokens::default()
                .with_preference(MotionPreference::Snap)
                .set(cx)
        });

        assert_eq!(draw(cx), 0, "snap must not animate");
        assert_eq!(progress.get(), 1.0);

        cx.executor().advance_clock(EnterSpec::REVEAL.duration);
        assert_eq!(draw(cx), 0);
        assert_eq!(progress.get(), 1.0);
    }

    #[gpui::test]
    fn a_replaced_policy_drives_a_running_reveal(cx: &mut TestAppContext) {
        let quickened = Duration::from_millis(10);
        let (progress, cx) = reveal_probe(false, cx);
        cx.update(|_, cx| MotionTokens::default().with_standard(quickened).set(cx));

        assert_eq!(draw(cx), 1, "the reveal is live under the new policy");
        cx.executor().advance_clock(quickened);
        draw(cx);
        assert_eq!(
            progress.get(),
            1.0,
            "ten milliseconds settles a ten-millisecond policy"
        );
        assert_eq!(draw(cx), 0, "and a settled reveal stays settled");
    }

    #[gpui::test]
    fn the_default_policy_is_still_travelling_at_that_moment(cx: &mut TestAppContext) {
        // The differential half of the test above: the same clock advance
        // under the default policy must not have settled, or the assertion
        // there would pass for reasons other than the replacement working.
        let (progress, cx) = reveal_probe(false, cx);
        draw(cx);
        cx.executor().advance_clock(Duration::from_millis(10));
        draw(cx);
        assert!(
            progress.get() < 1.0,
            "progress={} — the default tempo settled implausibly fast",
            progress.get()
        );
    }

    #[gpui::test]
    fn no_policy_value_can_override_reduced_motion(cx: &mut TestAppContext) {
        // The OS signal floors the effective preference at crossfade: a
        // ten-second policy still settles at the quick tempo with zero
        // travel, and a policy asking for full motion cannot win it back.
        let (progress, cx) = reveal_probe(true, cx);
        cx.update(|_, cx| {
            MotionTokens::default()
                .with_standard(Duration::from_secs(10))
                .with_preference(MotionPreference::Full)
                .set(cx);
            assert_eq!(
                MotionTokens::effective_preference(cx),
                MotionPreference::Crossfade,
                "full cannot outrank the OS signal"
            );
            assert_eq!(travel(cx), 0.0);
        });

        cx.executor().advance_clock(MotionTokens::DEFAULT.quick());
        draw(cx);
        assert_eq!(progress.get(), 1.0, "settled at quick, not at ten seconds");

        // The policy may restrict further than the signal, never less.
        cx.update(|_, cx| {
            MotionTokens::default()
                .with_preference(MotionPreference::Snap)
                .set(cx);
            assert_eq!(
                MotionTokens::effective_preference(cx),
                MotionPreference::Snap
            );
        });
    }
}
