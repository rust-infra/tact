//! A short sequence of questions an agent asks before it acts.
//!
//! One question at a time, each a single choice, with the place in the
//! sequence stated plainly and a way past a question that does not apply.
//! Where [`crate::approval::ApprovalCard`] asks for one decision about
//! something already proposed, this gathers what the agent needs in order to
//! propose anything at all.
//!
//! Controlled, like the rest of the library: the application owns the step
//! and every answer, and this reports what a person asked for. Nothing here
//! advances on its own — an application that wants to record an answer and
//! move on does both when the answer arrives.

use crate::ButtonLabelExt as _;
use crate::control::ControlMetricsExt as _;
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::form::{ChoiceEvent, ChoiceGroup, ChoiceOption};
use crate::handlers::SharedHandler;
use crate::surface::{CardFrameExt as _, description, meta, title};
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use std::rc::Rc;

/// One question, and the answers it will accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    id: SharedString,
    prompt: SharedString,
    note: Option<SharedString>,
    options: Vec<ChoiceOption>,
    answer: Option<SharedString>,
    optional: bool,
}

impl Question {
    /// Creates a question with a stable identifier and what it asks.
    pub fn new(id: impl Into<SharedString>, prompt: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            prompt: prompt.into(),
            note: None,
            options: Vec::new(),
            answer: None,
            optional: false,
        }
    }

    /// A line under the question, for why it is being asked.
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Sets the answers on offer, in order.
    pub fn options(mut self, options: impl IntoIterator<Item = ChoiceOption>) -> Self {
        self.options = options.into_iter().collect();
        self
    }

    /// Records the answer this question already has.
    pub fn answer(mut self, option: impl Into<SharedString>) -> Self {
        self.answer = Some(option.into());
        self
    }

    /// Records the answer, or the absence of one.
    pub fn answered(mut self, option: Option<SharedString>) -> Self {
        self.answer = option;
        self
    }

    /// Lets the sequence move past this question unanswered.
    pub fn optional(mut self, optional: bool) -> Self {
        self.optional = optional;
        self
    }
}

/// What a question flow reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QuestionFlowEvent {
    /// A person answered the question on screen.
    Answered {
        /// The flow's identifier.
        flow: SharedString,
        /// The question answered.
        question: SharedString,
        /// The option chosen.
        option: SharedString,
    },
    /// A person asked to move past the question on screen without answering.
    Skipped {
        /// The flow's identifier.
        flow: SharedString,
        /// The question skipped.
        question: SharedString,
    },
    /// A person asked to move on from an answered question.
    Advanced {
        /// The flow's identifier.
        flow: SharedString,
        /// The step being asked for.
        step: usize,
    },
    /// A person answered the last question and asked to finish.
    Completed {
        /// The flow's identifier.
        flow: SharedString,
    },
}

/// A sequence of single-choice questions, asked one at a time.
///
/// # Example
///
/// ```
/// use gpui_ai::prelude::{ChoiceOption, Question, QuestionFlow};
///
/// let flow = QuestionFlow::new("launch", "Before I draft the plan")
///     .questions([
///         Question::new("flavours", "How many flavours should we launch?").options([
///             ChoiceOption::new("three", "Three").description("The core line"),
///             ChoiceOption::new("five", "Five"),
///         ]),
///         Question::new("market", "Which market do we enter first?")
///             .options([ChoiceOption::new("trucks", "Food trucks")]),
///     ])
///     .step(0);
/// ```
#[derive(IntoElement)]
pub struct QuestionFlow {
    id: SharedString,
    title: Option<SharedString>,
    questions: Vec<Question>,
    step: usize,
    skip_label: SharedString,
    continue_label: SharedString,
    finish_label: SharedString,
    question_prefix: SharedString,
    counter_of: SharedString,
    on_event: Option<SharedHandler<QuestionFlowEvent>>,
    style: StyleRefinement,
    decoration: Decoration,
}

impl QuestionFlow {
    /// Layers painted into this flow's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a flow with a stable identifier and what it is gathering for.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: Some(title.into()),
            questions: Vec::new(),
            step: 0,
            skip_label: "Skip".into(),
            continue_label: "Continue".into(),
            finish_label: "Done".into(),
            question_prefix: "Question".into(),
            counter_of: "of".into(),
            on_event: None,
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
        }
    }

    /// Sets the questions, in the order they are asked.
    pub fn questions(mut self, questions: impl IntoIterator<Item = Question>) -> Self {
        self.questions = questions.into_iter().collect();
        self
    }

    /// Shows the question at `step`. Out of range shows the last one.
    pub fn step(mut self, step: usize) -> Self {
        self.step = step;
        self
    }

    /// Renames the control that moves past a question.
    pub fn skip_label(mut self, label: impl Into<SharedString>) -> Self {
        self.skip_label = label.into();
        self
    }

    /// Renames the control that moves on from an answered question.
    pub fn continue_label(mut self, label: impl Into<SharedString>) -> Self {
        self.continue_label = label.into();
        self
    }

    /// Renames the control that closes the last question.
    pub fn finish_label(mut self, label: impl Into<SharedString>) -> Self {
        self.finish_label = label.into();
        self
    }

    /// Renames the accessible prefix for the question counter.
    pub fn question_prefix(mut self, label: impl Into<SharedString>) -> Self {
        self.question_prefix = label.into();
        self
    }

    /// Sets the separator between the current question and the total.
    pub fn counter_of(mut self, label: impl Into<SharedString>) -> Self {
        self.counter_of = label.into();
        self
    }

    /// Handles the typed request.
    pub fn on_event(
        mut self,
        handler: impl Fn(&QuestionFlowEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    /// The step actually shown, which is the last question once `step` runs
    /// past the end.
    fn shown(&self) -> usize {
        self.step.min(self.questions.len().saturating_sub(1))
    }
}

impl Styled for QuestionFlow {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for QuestionFlow {
    fn render(mut self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let total = self.questions.len();
        let shown = self.shown();
        let root_id = ElementId::from(self.id.clone());
        let flow = self.id.clone();
        let handler = self.on_event;
        let card_title = self.title.clone();
        let debug_id = self.id.clone();

        let Some(question) = self.questions.into_iter().nth(shown) else {
            // A flow with nothing to ask draws nothing rather than an empty
            // frame with a "0 of 0" under it.
            return div().into_any_element();
        };

        let answered = question.answer.is_some();
        let last = shown + 1 == total;
        let question_id = question.id.clone();
        let group = ChoiceGroup::unlabelled(format!("{flow}-{question_id}"))
            .options(question.options.clone())
            .selection(question.answer.clone())
            .on_event({
                let handler = handler.clone();
                let flow = flow.clone();
                let question_id = question_id.clone();
                move |event, window, cx| {
                    let ChoiceEvent::Chosen { option, .. } = event;
                    if let Some(handler) = handler.as_ref() {
                        handler(
                            &QuestionFlowEvent::Answered {
                                flow: flow.clone(),
                                question: question_id.clone(),
                                option: option.clone(),
                            },
                            window,
                            cx,
                        );
                    }
                }
            });

        let skip_event = QuestionFlowEvent::Skipped {
            flow: flow.clone(),
            question: question_id.clone(),
        };
        let advance_event = if last {
            QuestionFlowEvent::Completed { flow: flow.clone() }
        } else {
            QuestionFlowEvent::Advanced {
                flow: flow.clone(),
                step: shown + 1,
            }
        };
        let advance_label = if last {
            self.finish_label.clone()
        } else {
            self.continue_label.clone()
        };
        let counter = format!("{} {} {total}", shown + 1, self.counter_of);
        let counter_debug = debug_id.clone();
        let skip_handler = handler.clone();
        let advance_handler = handler;

        v_flex()
            .id(root_id.clone())
            .debug_selector(move || format!("question-flow-{debug_id}"))
            .card_frame(cx)
            .decoration_under(&mut decoration)
            .p(tokens.spacing.lg)
            .gap(tokens.spacing.md)
            .role(Role::Group)
            .aria_label(
                card_title
                    .clone()
                    .unwrap_or_else(|| question.prompt.clone()),
            )
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
            .when_some(card_title, |card, text| card.child(title(text, cx)))
            .child(
                v_flex()
                    .gap(tokens.spacing.xs)
                    .child(description(question.prompt.clone(), cx))
                    .when_some(question.note.clone(), |body, note| {
                        body.child(crate::surface::hint(note, cx))
                    }),
            )
            .child(group)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .justify_between()
                    .gap(tokens.spacing.sm)
                    .child(
                        // The place in the sequence is a status, not a
                        // decoration: it is the only thing that says how much
                        // is left to answer.
                        div()
                            .id(ElementId::from((root_id.clone(), "counter")))
                            .debug_selector(move || {
                                format!("question-flow-counter-{counter_debug}")
                            })
                            .role(Role::Status)
                            .aria_label(format!("{} {counter}", self.question_prefix))
                            .child(meta(counter, cx)),
                    )
                    .child(
                        h_flex()
                            .gap(tokens.spacing.sm)
                            .when(question.optional || !answered, |row| {
                                row.child(
                                    Button::new(ElementId::from((root_id.clone(), "skip")))
                                        .ghost()
                                        .small()
                                        .control_metrics(cx)
                                        .accessibility_id(format!("{flow}-skip"))
                                        .debug_selector({
                                            let flow = flow.clone();
                                            move || format!("question-flow-skip-{flow}")
                                        })
                                        .text_label(self.skip_label.clone())
                                        .when_some(skip_handler, |button, handler| {
                                            button.on_click(move |_: &ClickEvent, window, cx| {
                                                handler(&skip_event, window, cx)
                                            })
                                        }),
                                )
                            })
                            .child(
                                Button::new(ElementId::from((root_id, "advance")))
                                    .primary()
                                    .small()
                                    .control_metrics(cx)
                                    .accessibility_id(format!("{flow}-advance"))
                                    .debug_selector({
                                        let flow = flow.clone();
                                        move || format!("question-flow-advance-{flow}")
                                    })
                                    .text_label(advance_label)
                                    .disabled(!answered)
                                    .when_some(
                                        advance_handler.filter(|_| answered),
                                        |button, handler| {
                                            button.on_click(move |_: &ClickEvent, window, cx| {
                                                handler(&advance_event, window, cx)
                                            })
                                        },
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }
}
