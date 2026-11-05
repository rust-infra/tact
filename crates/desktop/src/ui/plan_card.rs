//! Compact plan cards for goals and transcript plans.

use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};

/// The overall state of a plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanState {
    #[default]
    Proposed,
    Approved,
    Rejected,
    Running,
    Completed,
    Failed,
}

/// One step in a plan.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum PlanStepStatus {
    #[default]
    Pending,
    Done,
    Failed,
    Skipped,
}

/// One plan step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanStep {
    id: SharedString,
    label: SharedString,
    detail: Option<SharedString>,
    status: PlanStepStatus,
}

impl PlanStep {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            status: PlanStepStatus::default(),
        }
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn status(mut self, status: PlanStepStatus) -> Self {
        self.status = status;
        self
    }
}

/// User-visible copy for a plan card.
#[derive(Clone, Debug, PartialEq, Eq)]
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
            proposed: "Proposed".into(),
            approved: "Approved".into(),
            rejected: "Rejected".into(),
            running: "Running".into(),
            completed: "Completed".into(),
            failed: "Failed".into(),
            pending: "Pending".into(),
            done: "Done".into(),
            skipped: "Skipped".into(),
        }
    }
}

/// A titled group of plan steps.
#[derive(IntoElement)]
pub struct PlanCard {
    id: SharedString,
    title: SharedString,
    labels: PlanCardLabels,
    steps: Vec<PlanStep>,
    state: PlanState,
}

impl PlanCard {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            labels: PlanCardLabels::default(),
            steps: Vec::new(),
            state: PlanState::Proposed,
        }
    }

    pub fn labels(mut self, labels: PlanCardLabels) -> Self {
        self.labels = labels;
        self
    }

    pub fn steps(mut self, steps: impl IntoIterator<Item = PlanStep>) -> Self {
        self.steps = steps.into_iter().collect();
        self
    }

    pub fn state(mut self, state: PlanState) -> Self {
        self.state = state;
        self
    }

    fn state_label(&self) -> &SharedString {
        match self.state {
            PlanState::Proposed => &self.labels.proposed,
            PlanState::Approved => &self.labels.approved,
            PlanState::Rejected => &self.labels.rejected,
            PlanState::Running => &self.labels.running,
            PlanState::Completed => &self.labels.completed,
            PlanState::Failed => &self.labels.failed,
        }
    }

    fn state_color(&self, cx: &App) -> gpui::Hsla {
        match self.state {
            PlanState::Proposed | PlanState::Approved => cx.theme().info,
            PlanState::Rejected | PlanState::Failed => cx.theme().danger,
            PlanState::Running => cx.theme().warning,
            PlanState::Completed => cx.theme().success,
        }
    }
}

impl RenderOnce for PlanCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let done = self
            .steps
            .iter()
            .filter(|step| step.status == PlanStepStatus::Done)
            .count();
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Group)
            .aria_label(format!(
                "{} {} {}{} {} {}",
                self.labels.aria_prefix,
                self.title,
                done,
                self.labels.aria_of,
                self.steps.len(),
                self.labels.aria_steps_done
            ))
            .w_full()
            .gap_2()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .p_3()
            .child(
                h_flex()
                    .justify_between()
                    .items_center()
                    .child(
                        v_flex()
                            .gap(px(1.))
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(px(10.))
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.labels.eyebrow.clone()),
                            )
                            .child(div().text_sm().font_semibold().child(self.title.clone())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .rounded_full()
                            .bg(self.state_color(cx).opacity(0.14))
                            .px_2()
                            .py(px(2.))
                            .text_size(px(10.))
                            .text_color(self.state_color(cx))
                            .child(self.state_label().clone()),
                    ),
            )
            .child(
                v_flex()
                    .gap(px(3.))
                    .children(self.steps.iter().enumerate().map(|(index, step)| {
                        let (mark, color) = match step.status {
                            PlanStepStatus::Pending => ("○", cx.theme().muted_foreground),
                            PlanStepStatus::Done => ("●", cx.theme().success),
                            PlanStepStatus::Failed => ("×", cx.theme().danger),
                            PlanStepStatus::Skipped => ("–", cx.theme().muted_foreground),
                        };
                        h_flex()
                            .id(ElementId::Name(
                                format!("plan-step-{}-{index}", step.id).into(),
                            ))
                            .items_start()
                            .gap_2()
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(10.))
                                    .text_color(color)
                                    .child(mark),
                            )
                            .child(
                                v_flex()
                                    .flex_1()
                                    .min_w_0()
                                    .child(div().text_xs().child(step.label.clone()))
                                    .when_some(step.detail.clone(), |step, detail| {
                                        step.child(
                                            div()
                                                .text_size(px(10.))
                                                .text_color(cx.theme().muted_foreground)
                                                .child(detail),
                                        )
                                    }),
                            )
                    })),
            )
    }
}
