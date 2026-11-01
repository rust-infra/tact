//! Plan cards: an agent's proposed steps as one reviewable card.
//!
//! A [`PlanCard`] lists ordered [`PlanStep`]s with per-step status glyphs,
//! shows the plan's overall [`PlanState`] as a badge, and, while the plan is
//! still proposed, offers Approve / Reject (and optionally Edit). Decisions
//! and step activations are reported as [`PlanEvent`]s keyed by stable IDs;
//! the application owns execution and feeds statuses back through the
//! snapshot.

use crate::control::ControlMetricsExt as _;
use crate::control::QuietSurfaceExt as _;
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::{
    ButtonLabelExt as _,
    control::composed_button,
    cues::{self, Cue},
    handlers::SharedHandler,
    motion::{ArrivalRoster, MotionTokens, acknowledged_state},
    status::{StatusBadge, StatusTone},
    surface::{card, description, eyebrow, meta, title},
    theme::SemanticStyledExt as _,
};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    spinner::Spinner,
    v_flex,
};
use std::rc::Rc;

/// Where one step stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlanStepStatus {
    /// Not started.
    #[default]
    Pending,
    /// In progress.
    Running,
    /// Finished successfully.
    Done,
    /// Stopped with an error.
    Failed,
    /// Deliberately not executed.
    Skipped,
}

impl PlanStepStatus {
    /// The short status word read to assistive technology.
    pub fn label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
        }
    }
}

/// One ordered step of a plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlanStep {
    id: SharedString,
    title: SharedString,
    detail: Option<SharedString>,
    status: PlanStepStatus,
}

impl PlanStep {
    /// Creates a pending step with a stable identifier.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            status: PlanStepStatus::Pending,
        }
    }

    /// Adds a supporting line (what the step touches, why it is needed).
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Sets the status.
    pub fn status(mut self, status: PlanStepStatus) -> Self {
        self.status = status;
        self
    }

    /// Returns the stable step identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the step title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Returns the supporting line, if any.
    pub fn detail_text(&self) -> Option<&SharedString> {
        self.detail.as_ref()
    }

    /// Returns the status.
    pub fn step_status(&self) -> PlanStepStatus {
        self.status
    }
}

/// The plan's overall lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum PlanState {
    /// Waiting for a decision; Approve / Reject are offered.
    #[default]
    Proposed,
    /// Accepted, not yet started.
    Approved,
    /// Declined.
    Rejected,
    /// Steps are executing.
    Running,
    /// Every step finished.
    Completed,
    /// Execution stopped on a failure.
    Failed,
}

impl PlanState {
    /// The badge label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Proposed => "Proposed",
            Self::Approved => "Approved",
            Self::Rejected => "Rejected",
            Self::Running => "Running",
            Self::Completed => "Completed",
            Self::Failed => "Failed",
        }
    }

    fn tone(self) -> StatusTone {
        match self {
            Self::Proposed | Self::Running => StatusTone::Info,
            Self::Approved | Self::Completed => StatusTone::Success,
            Self::Rejected => StatusTone::Neutral,
            Self::Failed => StatusTone::Danger,
        }
    }
}

/// User-visible copy for a plan card.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct PlanCardLabels {
    pub eyebrow: SharedString,
    pub steps: SharedString,
    pub aria_prefix: SharedString,
    pub aria_of: SharedString,
    pub aria_steps_done: SharedString,
    pub step_prefix: SharedString,
    pub approve: SharedString,
    pub reject: SharedString,
    pub edit: SharedString,
    pub proposed: SharedString,
    pub approved: SharedString,
    pub rejected: SharedString,
    pub running: SharedString,
    pub completed: SharedString,
    pub failed: SharedString,
    pub pending: SharedString,
    pub done: SharedString,
    pub skipped: SharedString,
}

impl Default for PlanCardLabels {
    fn default() -> Self {
        Self {
            eyebrow: "Plan".into(),
            steps: "Steps".into(),
            aria_prefix: "Plan".into(),
            aria_of: "of".into(),
            aria_steps_done: "steps done".into(),
            step_prefix: "Step".into(),
            approve: "Approve plan".into(),
            reject: "Reject".into(),
            edit: "Edit plan".into(),
            proposed: PlanState::Proposed.label().into(),
            approved: PlanState::Approved.label().into(),
            rejected: PlanState::Rejected.label().into(),
            running: PlanState::Running.label().into(),
            completed: PlanState::Completed.label().into(),
            failed: PlanState::Failed.label().into(),
            pending: PlanStepStatus::Pending.label().into(),
            done: PlanStepStatus::Done.label().into(),
            skipped: PlanStepStatus::Skipped.label().into(),
        }
    }
}

impl PlanCardLabels {
    fn state(&self, state: PlanState) -> &SharedString {
        match state {
            PlanState::Proposed => &self.proposed,
            PlanState::Approved => &self.approved,
            PlanState::Rejected => &self.rejected,
            PlanState::Running => &self.running,
            PlanState::Completed => &self.completed,
            PlanState::Failed => &self.failed,
        }
    }

    fn step_status(&self, status: PlanStepStatus) -> &SharedString {
        match status {
            PlanStepStatus::Pending => &self.pending,
            PlanStepStatus::Running => &self.running,
            PlanStepStatus::Done => &self.done,
            PlanStepStatus::Failed => &self.failed,
            PlanStepStatus::Skipped => &self.skipped,
        }
    }
}

/// An interaction emitted by [`PlanCard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanEvent {
    /// The plan was approved.
    Approved {
        /// Stable plan identifier.
        id: SharedString,
    },
    /// The plan was rejected.
    Rejected {
        /// Stable plan identifier.
        id: SharedString,
    },
    /// The user asked to change the plan before deciding.
    EditRequested {
        /// Stable plan identifier.
        id: SharedString,
    },
    /// The user activated one step (to inspect or jump to it).
    StepActivated {
        /// Stable plan identifier.
        id: SharedString,
        /// Stable step identifier.
        step_id: SharedString,
    },
}

/// A proposed plan with ordered steps, a state badge, and decision controls.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// PlanCard::new("rollout", "Switch bulk orders to Alpenrose")
///     .description("Three steps; the last one sends email.")
///     .steps([
///         PlanStep::new("compare", "Compare unit prices").status(PlanStepStatus::Done),
///         PlanStep::new("draft", "Draft the new order"),
///         PlanStep::new("send", "Send confirmations").detail("Emails 3 suppliers"),
///     ])
///     .editable(true)
///     .on_event(|event, _, _| { /* PlanEvent::Approved { id } … */ });
/// ```
#[derive(IntoElement)]
pub struct PlanCard {
    id: SharedString,
    style: StyleRefinement,
    decoration: Decoration,
    title: SharedString,
    description: Option<SharedString>,
    steps: Vec<PlanStep>,
    state: PlanState,
    note: Option<SharedString>,
    editable: bool,
    on_event: Option<SharedHandler<PlanEvent>>,
    labels: PlanCardLabels,
}

impl PlanCard {
    /// Layers painted into this card's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a proposed plan.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            title: title.into(),
            description: None,
            steps: Vec::new(),
            state: PlanState::Proposed,
            note: None,
            editable: false,
            on_event: None,
            labels: PlanCardLabels::default(),
        }
    }

    /// Sets supporting detail under the title.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the steps, in order.
    pub fn steps(mut self, steps: impl IntoIterator<Item = PlanStep>) -> Self {
        self.steps = steps.into_iter().collect();
        self
    }

    /// Sets the lifecycle state (default [`PlanState::Proposed`]).
    pub fn state(mut self, state: PlanState) -> Self {
        self.state = state;
        self
    }

    /// Adds a footnote shown once the plan is decided ("Approved by Oscar").
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Offers an Edit control while the plan is proposed.
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Replaces the card's user-visible and accessible labels.
    pub fn labels(mut self, labels: PlanCardLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Handles typed interactions. Without a handler the card is static.
    pub fn on_event(
        mut self,
        handler: impl Fn(&PlanEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    /// Counts finished steps.
    fn done_count(&self) -> usize {
        self.steps
            .iter()
            .filter(|step| step.status == PlanStepStatus::Done)
            .count()
    }
}

impl Styled for PlanCard {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for PlanCard {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let done = self.done_count();
        let handler = self.on_event;
        let labels = self.labels.clone();
        let plan_id = self.id.clone();
        let total = self.steps.len();
        let label: SharedString = format!(
            "{}: {}, {done} {} {total} {}, {}",
            labels.aria_prefix,
            self.title,
            labels.aria_of,
            labels.aria_steps_done,
            labels.state(self.state)
        )
        .into();
        let root_id = ElementId::from(self.id.clone());
        let debug_id = self.id.to_string();

        // Steps the card has already shown never re-animate: the roster
        // persists in keyed window state, the initial plan joins at rest,
        // and only steps appended to a mounted card settle in — on the
        // capped cascade, so a long plan accumulates neither delay nor
        // offscreen work.
        let motion = MotionTokens::read(cx).clone();
        let roster = window.use_keyed_state((root_id.clone(), "arrivals"), cx, |_, _| {
            ArrivalRoster::new()
        });
        roster.update(cx, |roster, cx| {
            roster.note(
                self.steps.iter().map(|step| {
                    ElementId::Name(SharedString::from(format!("plan-step-{}", step.id)))
                }),
                true,
                &motion,
                cx.background_executor().now(),
            );
        });
        let mut steps = Vec::with_capacity(total);
        for (index, step) in self.steps.iter().enumerate() {
            let arrival = roster.update(cx, |roster, cx| {
                roster.progress(
                    &ElementId::Name(SharedString::from(format!("plan-step-{}", step.id))),
                    window,
                    cx,
                )
            });
            steps.push(render_step(
                &root_id,
                &plan_id,
                index,
                total,
                step,
                arrival,
                handler.clone(),
                &labels,
                window,
                cx,
            ));
        }

        let footer: Option<AnyElement> = match (self.state, handler) {
            (PlanState::Proposed, Some(handler)) => {
                let approve_id = plan_id.clone();
                let reject_id = plan_id.clone();
                let edit_id = plan_id.clone();
                let approve_handler = handler.clone();
                let reject_handler = handler.clone();
                let approve_debug = debug_id.clone();
                let reject_debug = debug_id.clone();
                let edit_debug = debug_id.clone();
                Some(
                    h_flex()
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(
                            div()
                                .debug_selector(move || format!("plan-approve-{approve_debug}"))
                                .child(
                                    Button::new((root_id.clone(), "approve"))
                                        .primary()
                                        .small()
                                        .control_metrics(cx)
                                        .accessibility_id(format!("{plan_id}-approve"))
                                        .text_label(labels.approve.clone())
                                        .on_click(move |_: &ClickEvent, window, cx| {
                                            cues::emit(cx, Cue::Decided { approved: true });
                                            approve_handler(
                                                &PlanEvent::Approved {
                                                    id: approve_id.clone(),
                                                },
                                                window,
                                                cx,
                                            )
                                        }),
                                ),
                        )
                        .child(
                            div()
                                .debug_selector(move || format!("plan-reject-{reject_debug}"))
                                .child(
                                    Button::new((root_id.clone(), "reject"))
                                        .outline()
                                        .small()
                                        .control_metrics(cx)
                                        .accessibility_id(format!("{plan_id}-reject"))
                                        .text_label(labels.reject.clone())
                                        .on_click(move |_: &ClickEvent, window, cx| {
                                            cues::emit(cx, Cue::Decided { approved: false });
                                            reject_handler(
                                                &PlanEvent::Rejected {
                                                    id: reject_id.clone(),
                                                },
                                                window,
                                                cx,
                                            )
                                        }),
                                ),
                        )
                        .when(self.editable, |this| {
                            this.child(
                                div()
                                    .debug_selector(move || format!("plan-edit-{edit_debug}"))
                                    .child(
                                        Button::new((root_id.clone(), "edit"))
                                            .ghost()
                                            .small()
                                            .control_metrics(cx)
                                            .accessibility_id(format!("{plan_id}-edit"))
                                            .text_label(labels.edit.clone())
                                            .on_click(move |_: &ClickEvent, window, cx| {
                                                handler(
                                                    &PlanEvent::EditRequested {
                                                        id: edit_id.clone(),
                                                    },
                                                    window,
                                                    cx,
                                                )
                                            }),
                                    ),
                            )
                        })
                        .into_any_element(),
                )
            }
            _ => self
                .note
                .clone()
                .map(|note| meta(note, cx).into_any_element()),
        };

        card(self.id.clone(), cx)
            .decoration_under(&mut decoration)
            .role(Role::Group)
            .aria_label(label)
            .when_some(self.description.clone(), |this, text| {
                this.aria_description(text)
            })
            .debug_selector(move || format!("plan-{debug_id}"))
            .when(self.state == PlanState::Proposed, |this| {
                this.border_color(cx.theme().warning)
            })
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(tokens.spacing.sm)
                    .child(eyebrow(labels.eyebrow.clone(), cx))
                    .child(
                        h_flex()
                            .items_center()
                            .gap(tokens.spacing.sm)
                            .child(meta(format!("{done} / {total}"), cx))
                            .child(
                                StatusBadge::new(
                                    (ElementId::from(self.id.clone()), "state"),
                                    labels.state(self.state).clone(),
                                )
                                .tone(self.state.tone())
                                .active(self.state == PlanState::Running),
                            ),
                    ),
            )
            .child(title(self.title, cx))
            .when_some(self.description, |this, text| {
                this.child(description(text, cx))
            })
            .child(
                v_flex()
                    .id((ElementId::from(self.id.clone()), "steps"))
                    .role(Role::List)
                    .aria_label(labels.steps.clone())
                    .w_full()
                    .min_w_0()
                    .children(steps),
            )
            .children(footer)
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
    }
}

#[allow(clippy::too_many_arguments)]
fn render_step(
    root_id: &ElementId,
    plan_id: &SharedString,
    index: usize,
    total: usize,
    step: &PlanStep,
    arrival: Option<f32>,
    handler: Option<SharedHandler<PlanEvent>>,
    labels: &PlanCardLabels,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    let step_id = ElementId::from((root_id.clone(), format!("step-{}", step.id)));
    let plan_debug = plan_id.clone();
    let step_debug = step.id.clone();
    let label: SharedString = format!(
        "{} {}: {}, {}",
        labels.step_prefix,
        index + 1,
        step.title,
        labels.step_status(step.status)
    )
    .into();
    let is_last = index + 1 == total;

    let glyph_size = tokens.spacing.lg;
    // Terminal marks settle into the fixed glyph ring once, after the
    // controlled status changes; the status a step mounts with is exempt,
    // and error paths share the same quick, overshoot-free acknowledgment.
    let acknowledged = acknowledged_state(
        ElementId::from((step_id.clone(), "glyph")),
        step.status as u64,
        window,
        cx,
    );
    let (glyph, ring, fill): (AnyElement, gpui::Hsla, gpui::Hsla) = match step.status {
        PlanStepStatus::Pending => (
            div()
                .text_token(tokens.typography.xs)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().muted_foreground)
                .child((index + 1).to_string())
                .into_any_element(),
            cx.theme().border,
            cx.theme().transparent,
        ),
        PlanStepStatus::Running => (
            Spinner::new().xsmall().into_any_element(),
            cx.theme().primary,
            cx.theme().primary.opacity(0.1),
        ),
        PlanStepStatus::Done => (
            Icon::new(IconName::Check)
                .xsmall()
                .text_color(cx.theme().success)
                .opacity(acknowledged)
                .into_any_element(),
            cx.theme().success.opacity(0.5),
            cx.theme().success.opacity(0.12),
        ),
        PlanStepStatus::Failed => (
            Icon::new(IconName::CircleX)
                .xsmall()
                .text_color(cx.theme().danger)
                .opacity(acknowledged)
                .into_any_element(),
            cx.theme().danger.opacity(0.5),
            cx.theme().danger.opacity(0.12),
        ),
        PlanStepStatus::Skipped => (
            Icon::new(IconName::Dash)
                .xsmall()
                .text_color(cx.theme().muted_foreground)
                .opacity(acknowledged)
                .into_any_element(),
            cx.theme().border,
            cx.theme().muted.opacity(0.5),
        ),
    };
    let rail = v_flex()
        .flex_none()
        .items_center()
        .w(glyph_size)
        .child(
            div()
                .flex_none()
                .size(glyph_size)
                .flex()
                .items_center()
                .justify_center()
                .rounded(tokens.radius.full)
                .border_1()
                .border_color(ring)
                .bg(fill)
                .child(glyph),
        )
        .when(!is_last, |this| {
            this.child(
                div()
                    .flex_1()
                    .min_h(tokens.spacing.sm)
                    .border_l_1()
                    .border_color(cx.theme().border),
            )
        });
    let body = div()
        .flex()
        .flex_col()
        .min_w_0()
        .flex_1()
        .gap(tokens.spacing.xxs)
        .pb(if is_last {
            tokens.spacing.xxs
        } else {
            tokens.spacing.sm
        })
        .child(
            div()
                .text_token(tokens.typography.sm)
                .text_color(match step.status {
                    PlanStepStatus::Skipped => cx.theme().muted_foreground,
                    _ => cx.theme().foreground,
                })
                .child(step.title.clone()),
        )
        .when_some(step.detail.clone(), |this, detail| {
            this.child(crate::surface::hint(detail, cx))
        });

    let row = match handler {
        Some(handler) => {
            let event = PlanEvent::StepActivated {
                id: plan_id.clone(),
                step_id: step.id.clone(),
            };
            composed_button(step_id.clone(), label)
                .debug_selector(move || format!("plan-step-{plan_debug}-{step_debug}"))
                .flex()
                .items_stretch()
                .w_full()
                .min_w_0()
                .gap(tokens.spacing.sm)
                .quiet_press_surface(
                    ElementId::from((step_id, "press")),
                    tokens.radius.sm,
                    window,
                    cx,
                )
                .child(rail)
                .child(body)
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                .into_any_element()
        }
        None => h_flex()
            .id(step_id)
            .role(Role::ListItem)
            .aria_label(label)
            .debug_selector(move || format!("plan-step-{plan_debug}-{step_debug}"))
            .items_stretch()
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.sm)
            .child(rail)
            .child(body)
            .into_any_element(),
    };
    let wrapped = div().w_full().min_w_0().child(row);
    match arrival {
        // A freshly appended step settles in on its assigned beat; every
        // other step — the whole initial plan included — renders at rest.
        Some(progress) => wrapped
            .opacity(progress)
            .top(tokens.spacing.xxs * (1.0 - progress) * crate::motion::travel(cx))
            .into_any_element(),
        None => wrapped.into_any_element(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, px};

    struct PlanProbe {
        count: usize,
    }

    impl Render for PlanProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(360.)).h(px(480.)).child(
                PlanCard::new("probe-plan", "Rollout")
                    .steps((0..self.count).map(|ix| PlanStep::new(format!("step-{ix}"), "Step"))),
            )
        }
    }

    #[gpui::test]
    fn a_proposed_plan_mounts_at_rest_and_new_steps_settle_in(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| PlanProbe { count: 3 });
        let cx: &mut VisualTestContext = cx;
        cx.update(|window, cx| window.draw(cx).clear(cx));
        crate::motion::take_reveal_frame_requests();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a plan's initial steps are a proposal to read, not a cascade"
        );

        probe.update(cx, |probe, cx| {
            probe.count = 4;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "a step appended to a mounted plan must settle in"
        );
    }

    #[test]
    fn plan_state_labels_and_tones_follow_the_lifecycle() {
        assert_eq!(PlanState::Proposed.label(), "Proposed");
        assert_eq!(PlanState::Completed.tone(), StatusTone::Success);
        assert_eq!(PlanState::Failed.tone(), StatusTone::Danger);
        assert_eq!(PlanState::Rejected.tone(), StatusTone::Neutral);
    }

    #[test]
    fn done_steps_are_counted_for_the_accessible_name() {
        let card = PlanCard::new("p", "Plan").steps([
            PlanStep::new("a", "A").status(PlanStepStatus::Done),
            PlanStep::new("b", "B").status(PlanStepStatus::Running),
            PlanStep::new("c", "C"),
        ]);
        assert_eq!(card.done_count(), 1);
    }
}
