//! One status vocabulary for every lifecycle the library displays.
//!
//! Tool chips, task rows, tool-call cards, and plan steps all describe work
//! as pending, running, completed, or failed. [`StatusTone`] maps those
//! meanings onto the theme's semantic colors once, and [`StatusBadge`] is the
//! single compact pill that renders them, so status looks identical wherever
//! it appears.

use crate::motion::swap_progress;
use crate::stream::ProgressState;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, Hsla, InteractiveElement as _, IntoElement,
    ParentElement as _, Pixels, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, spinner::Spinner, v_flex,
};

/// The meaning a status carries, independent of any one component.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusTone {
    /// Not started, idle, or informational-neutral.
    #[default]
    Neutral,
    /// Actively in progress.
    Info,
    /// Finished successfully.
    Success,
    /// Needs attention but is not an error.
    Warning,
    /// Failed or destructive.
    Danger,
}

impl StatusTone {
    /// Resolves the tone to the active theme's semantic color.
    pub fn color(self, cx: &App) -> Hsla {
        match self {
            Self::Neutral => cx.theme().muted_foreground,
            Self::Info => cx.theme().info,
            Self::Success => cx.theme().success,
            Self::Warning => cx.theme().warning,
            Self::Danger => cx.theme().danger,
        }
    }

    /// Maps the shared progressive lifecycle onto a tone.
    pub fn from_progress(state: &ProgressState) -> Self {
        match state {
            ProgressState::Pending => Self::Neutral,
            ProgressState::Running => Self::Info,
            ProgressState::Complete => Self::Success,
            ProgressState::Failed(_) => Self::Danger,
        }
    }
}

/// How strongly a chip speaks.
///
/// Two strengths are the whole chip vocabulary: a neutral inset pill for
/// plain values, and the tinted status chip — solid status-colour text on
/// the same hue at low alpha, borderless — for lifecycle and tags. Each
/// carries its own padding, because a neutral pill holds a bare word
/// while a tinted one holds a word beside a glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ChipStrength {
    /// Inset pill: secondary surface, muted ink.
    Neutral,
    /// Solid status-colour text on the same hue at low alpha. The default.
    #[default]
    Tinted,
}

/// The chip surface all three strengths share: extra-small medium-weight
/// text, chip paddings, full radius, and no border — a tinted pill inside
/// a bordered row never doubles its lines.
pub(crate) fn chip_frame(color: Hsla, strength: ChipStrength, cx: &App) -> Div {
    let tokens = cx.theme().semantic_tokens();
    let (background, foreground, padding) = match strength {
        ChipStrength::Neutral => (
            cx.theme().secondary,
            cx.theme().muted_foreground,
            tokens.spacing.xs,
        ),
        ChipStrength::Tinted => (color.opacity(0.12), color, tokens.spacing.sm),
    };
    div()
        .flex_none()
        .px(padding)
        .py(tokens.spacing.xxs)
        .rounded(tokens.radius.full)
        .bg(background)
        .text_token(tokens.typography.xs)
        .font_weight(FontWeight::MEDIUM)
        .text_color(foreground)
}

/// One lifecycle glyph for the whole library, seated in a slot from the
/// size policy: hollow dot pending, spinner running, check complete,
/// cross failed. Settled faces fade in once through the shared
/// acknowledgment (the state a surface mounts with is exempt); the
/// spinner stays untouched, since fading an animation in would stack two
/// motions on one slot. Glyph weight follows the slot — the medium slot
/// seats small icons, anything tighter seats extra-small — so the same
/// lifecycle reads identically across cards, rows, and tables.
pub(crate) fn progress_glyph(
    state: &ProgressState,
    ack_slot: ElementId,
    slot: Pixels,
    window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> Div {
    let tokens = cx.theme().semantic_tokens();
    let ordinal = match state {
        ProgressState::Pending => 0,
        ProgressState::Running => 1,
        ProgressState::Complete => 2,
        ProgressState::Failed(_) => 3,
    };
    let acknowledged = crate::motion::acknowledged_state(ack_slot, ordinal, window, cx);
    let roomy = slot >= crate::sizing::slot_md(cx);
    let glyph: AnyElement = match state {
        ProgressState::Pending => div()
            .when(roomy, |dot| dot.size_2())
            .when(!roomy, |dot| dot.size_1p5())
            .rounded(tokens.radius.full)
            .border_1()
            .border_color(cx.theme().muted_foreground)
            .into_any_element(),
        ProgressState::Running => {
            let spinner = Spinner::new().color(cx.theme().info);
            if roomy {
                spinner.small()
            } else {
                spinner.xsmall()
            }
            .into_any_element()
        }
        ProgressState::Complete => {
            let icon = Icon::new(IconName::CircleCheck).text_color(cx.theme().success);
            if roomy { icon.small() } else { icon.xsmall() }
                .opacity(acknowledged)
                .into_any_element()
        }
        ProgressState::Failed(_) => {
            let icon = Icon::new(IconName::CircleX).text_color(cx.theme().danger);
            if roomy { icon.small() } else { icon.xsmall() }
                .opacity(acknowledged)
                .into_any_element()
        }
    };
    crate::surface::leading_glyph_slot(slot, glyph)
}

/// Human-readable label for a progressive lifecycle state.
pub fn progress_label(state: &ProgressState) -> &'static str {
    match state {
        ProgressState::Pending => "Pending",
        ProgressState::Running => "Running",
        ProgressState::Complete => "Completed",
        ProgressState::Failed(_) => "Failed",
    }
}

/// A compact status pill: a tone-colored dot (or spinner while active) and a
/// short label, exposed as a named status region.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(task: Progressive<()>) {
/// StatusBadge::for_progress("call-status", task.state());
/// StatusBadge::new("review", "Needs review").tone(StatusTone::Warning);
/// # }
/// ```
#[derive(IntoElement)]
pub struct StatusBadge {
    id: ElementId,
    style: StyleRefinement,
    label: SharedString,
    tone: StatusTone,
    active: bool,
    reserve_lifecycle_width: bool,
}

/// What the badge showed last, held in keyed window state so a change can
/// stage the outgoing status while the incoming one settles into the slot.
struct StatusSwap {
    label: SharedString,
    tone: StatusTone,
    active: bool,
    /// Bumped per change; keys the swap clock so each change plays once.
    generation: u64,
    /// The status being faded out, present only while a swap is running.
    /// An outgoing spinner is frozen to its tone's dot: a settling copy of
    /// an animation would be a second active-work signal.
    outgoing: Option<(SharedString, StatusTone)>,
}

impl StatusBadge {
    /// Creates a neutral badge.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            label: label.into(),
            tone: StatusTone::Neutral,
            active: false,
            reserve_lifecycle_width: false,
        }
    }

    /// Creates a badge describing a progressive lifecycle state.
    ///
    /// The badge is as wide as the word it says, like every other chip in
    /// the library: a pill padded out to some other label's width leaves
    /// its own text adrift, which is what the 0.4.0 feel review saw in
    /// "Failed" sitting in a "Completed"-sized pill. Callers who need the
    /// lifecycle to hold one width — a badge with layout to its right that
    /// must not shuffle — ask for it with
    /// [`reserve_lifecycle_width`](Self::reserve_lifecycle_width).
    pub fn for_progress(id: impl Into<ElementId>, state: &ProgressState) -> Self {
        Self::new(id, progress_label(state))
            .tone(StatusTone::from_progress(state))
            .active(matches!(state, ProgressState::Running))
    }

    /// Holds the width of the widest lifecycle label, so
    /// pending→running→completed/failed never moves what sits beside it.
    ///
    /// Only for a badge with layout to its right: a trailing badge grows
    /// into its own margin and needs no reservation. The label centres in
    /// the reserved slot rather than hugging its leading edge.
    pub fn reserve_lifecycle_width(mut self, reserve: bool) -> Self {
        self.reserve_lifecycle_width = reserve;
        self
    }

    /// Sets the badge tone.
    pub fn tone(mut self, tone: StatusTone) -> Self {
        self.tone = tone;
        self
    }

    /// Shows a spinner instead of the dot while work is in progress.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
}

impl Styled for StatusBadge {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for StatusBadge {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let color = self.tone.color(cx);

        // The swap is staged from keyed state: the first render is not a
        // transition, and every later change fades the outgoing status away
        // while the incoming one lifts into the same fixed slot. Semantics
        // never wait — aria_label and the tone below are the incoming state
        // from the first frame of the change.
        let swap =
            window.use_keyed_state((self.id.clone(), "status-swap"), cx, |_, _| StatusSwap {
                label: self.label.clone(),
                tone: self.tone,
                active: self.active,
                generation: 0,
                outgoing: None,
            });
        let (generation, outgoing) = swap.update(cx, |swap, _| {
            if swap.label != self.label || swap.tone != self.tone || swap.active != self.active {
                // A label or tone change stages the old face; an
                // active-only flip (spinner to dot on the same status) just
                // swaps the indicator without staging a ghost label.
                if swap.label != self.label || swap.tone != self.tone {
                    swap.outgoing = Some((swap.label.clone(), swap.tone));
                }
                swap.generation += 1;
                swap.label = self.label.clone();
                swap.tone = self.tone;
                swap.active = self.active;
            }
            (swap.generation, swap.outgoing.clone())
        });
        let progress = if generation == 0 {
            1.0
        } else {
            swap_progress(
                ElementId::NamedInteger(
                    SharedString::from(format!("{:?}-status-swap", self.id)),
                    generation,
                ),
                window,
                cx,
            )
        };
        if progress >= 1.0 && outgoing.is_some() {
            swap.update(cx, |swap, _| swap.outgoing = None);
        }
        let exit = tokens.spacing.xxs * progress * crate::motion::travel(cx);
        let entry = tokens.spacing.xxs * (1.0 - progress) * crate::motion::travel(cx);

        // The indicator slot is fixed — sized for the spinner, centering the
        // smaller dot — so running↔settled never nudges the label sideways.
        let indicator_label = self.label.clone();
        let indicator = div()
            .debug_selector(move || format!("status-badge-indicator-{indicator_label}"))
            .flex_none()
            .size(crate::sizing::slot_xs(cx))
            .flex()
            .items_center()
            .justify_center()
            .child(if self.active {
                Spinner::new().xsmall().color(color).into_any_element()
            } else {
                div()
                    .debug_selector({
                        let dot_label = self.label.clone();
                        move || format!("status-badge-dot-{dot_label}")
                    })
                    .size_1p5()
                    .rounded(tokens.radius.full)
                    .bg(color)
                    .opacity(progress)
                    .into_any_element()
            });

        let label_slot = v_flex()
            .relative()
            .items_center()
            .when(self.reserve_lifecycle_width, |slot| {
                // Zero-height ghosts of every lifecycle label make the slot
                // as wide as the widest one, in whatever face and rem scale
                // the theme resolves — so the whole pending→failed journey
                // fits one width with nothing measured. The pill's
                // aria_label is the accessible name; these never paint.
                slot.children(LIFECYCLE_LABELS.iter().map(|label| {
                    div()
                        .h(rems(0.))
                        .overflow_hidden()
                        .opacity(0.)
                        .child(*label)
                }))
            })
            .child({
                let label_name = self.label.clone();
                div()
                    .debug_selector(move || format!("status-badge-label-{label_name}"))
                    .opacity(progress)
                    .top(entry)
                    .child(self.label.clone())
            })
            .when_some(outgoing, |slot, (label, tone)| {
                // The outgoing face drifts down as it fades, layered over
                // the slot so it never contributes width of its own.
                slot.child(
                    div()
                        .absolute()
                        .left(rems(0.))
                        .top(exit)
                        .opacity(1.0 - progress)
                        .text_color(tone.color(cx))
                        .child(label),
                )
            });

        // The dot rides a slot sized for the spinner that replaces it, so
        // the slot carries slack the dot does not fill. Left outside that
        // slack, the chip's own leading padding put visibly more room
        // before the dot than after the label — the optical asymmetry the
        // 0.4.0 feel review saw. The leading inset absorbs the slack so
        // both ends read the same, and the slot still holds the label
        // still when the spinner takes the dot's place.
        let slot_slack = (crate::sizing::slot_xs(cx) - INDICATOR_DOT) / 2.;
        chip_frame(color, ChipStrength::Tinted, cx)
            .id(self.id)
            .role(Role::Status)
            .aria_label(self.label.clone())
            .flex()
            .items_center()
            .pl((tokens.spacing.sm - slot_slack).max(gpui::Pixels::ZERO))
            .gap(tokens.spacing.xs)
            .child(indicator)
            .child(label_slot)
            .refine_style(&self.style)
    }
}

/// Every label [`progress_label`] can produce, for the lifecycle slot's
/// width reservation. A new lifecycle label joins both or the reservation
/// silently under-measures.
const LIFECYCLE_LABELS: [&str; 4] = ["Pending", "Running", "Completed", "Failed"];

/// The settled dot's diameter, which is `size_1p5`. Named because the
/// badge's leading inset is measured against the slack the dot leaves in
/// the spinner-sized slot around it.
const INDICATOR_DOT: gpui::Pixels = gpui::px(6.);

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Entity, Render, TestAppContext, VisualTestContext, px};
    use gpui_component::h_flex;

    struct BadgeProbe {
        state: ProgressState,
    }

    impl Render for BadgeProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(320.)).h(px(80.)).child(
                h_flex()
                    .debug_selector(|| "badge-hug".into())
                    .flex_none()
                    .child(StatusBadge::for_progress("probe-badge", &self.state)),
            )
        }
    }

    fn open(cx: &mut TestAppContext) -> (Entity<BadgeProbe>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| BadgeProbe {
            state: ProgressState::Pending,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (probe, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn set_state(probe: &Entity<BadgeProbe>, cx: &mut VisualTestContext, state: ProgressState) {
        probe.update(cx, |probe, cx| {
            probe.state = state;
            cx.notify();
        });
        draw(cx);
    }

    fn settle_swap(cx: &mut VisualTestContext) {
        cx.executor()
            .advance_clock(crate::motion::MotionTokens::DEFAULT.quick() * 2);
        draw(cx);
        draw(cx);
    }

    #[gpui::test]
    fn a_status_change_stages_a_swap_and_settles_quiet(cx: &mut TestAppContext) {
        let (probe, cx) = open(cx);
        crate::motion::take_reveal_frame_requests();

        set_state(&probe, cx, ProgressState::Complete);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "a status change must acknowledge itself"
        );

        settle_swap(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a settled swap must schedule nothing"
        );
    }

    #[gpui::test]
    fn the_first_render_is_not_a_transition(cx: &mut TestAppContext) {
        let (_, cx) = open(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a badge mounts at rest, whatever state it mounts in"
        );
    }

    #[gpui::test]
    fn reduced_motion_swaps_crossfade_and_snap_is_instant(cx: &mut TestAppContext) {
        // The OS flag resolves to crossfade: the swap's opacity fade runs
        // at the quick tempo and settles quiet.
        cx.update(|cx| cx.set_reduce_motion(true));
        let (probe, cx) = open(cx);
        crate::motion::take_reveal_frame_requests();

        set_state(&probe, cx, ProgressState::Failed("offline".into()));
        draw(cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "the crossfade swap runs"
        );
        settle_swap(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(crate::motion::take_reveal_frame_requests(), 0);

        // The snap preference is the true zero.
        cx.update(|_, cx| {
            crate::motion::MotionTokens::default()
                .with_preference(crate::motion::MotionPreference::Snap)
                .set(cx)
        });
        set_state(&probe, cx, ProgressState::Complete);
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "snap resolves the swap instantly"
        );
    }

    #[test]
    fn every_lifecycle_label_is_reserved() {
        for state in [
            ProgressState::Pending,
            ProgressState::Running,
            ProgressState::Complete,
            ProgressState::Failed("offline".into()),
        ] {
            assert!(
                LIFECYCLE_LABELS.contains(&progress_label(&state)),
                "the width reservation must know every label for_progress can show"
            );
        }
    }

    #[test]
    fn progress_maps_onto_one_tone_and_label_each() {
        let cases = [
            (ProgressState::Pending, StatusTone::Neutral, "Pending"),
            (ProgressState::Running, StatusTone::Info, "Running"),
            (ProgressState::Complete, StatusTone::Success, "Completed"),
            (
                ProgressState::Failed("offline".into()),
                StatusTone::Danger,
                "Failed",
            ),
        ];
        for (state, tone, label) in cases {
            assert_eq!(StatusTone::from_progress(&state), tone);
            assert_eq!(progress_label(&state), label);
            let badge = StatusBadge::for_progress("badge", &state);
            assert_eq!(badge.tone, tone);
            assert_eq!(badge.label.as_ref(), label);
            assert_eq!(badge.active, matches!(state, ProgressState::Running));
        }
    }
}
