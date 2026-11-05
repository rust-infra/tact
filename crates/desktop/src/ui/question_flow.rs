//! Single- and multi-question clarification cards.

use std::rc::Rc;

use gpui::{
    ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, StyledExt as _, h_flex, v_flex};

type EventHandler<E> = Rc<dyn Fn(&E, &mut Window, &mut gpui::App)>;

/// One choice in a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChoiceOption {
    id: SharedString,
    label: SharedString,
}

impl ChoiceOption {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// One question in a flow.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Question {
    id: SharedString,
    prompt: SharedString,
    optional: bool,
    options: Vec<ChoiceOption>,
    answered: Option<SharedString>,
}

impl Question {
    pub fn new(id: impl Into<SharedString>, prompt: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            prompt: prompt.into(),
            optional: false,
            options: Vec::new(),
            answered: None,
        }
    }

    pub fn optional(mut self, optional: bool) -> Self {
        self.optional = optional;
        self
    }

    pub fn options(mut self, options: impl IntoIterator<Item = ChoiceOption>) -> Self {
        self.options = options.into_iter().collect();
        self
    }

    pub fn answered(mut self, answered: Option<SharedString>) -> Self {
        self.answered = answered;
        self
    }
}

/// An interaction emitted by [`QuestionFlow`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QuestionFlowEvent {
    Answered {
        id: SharedString,
        option: SharedString,
    },
    Completed {
        id: SharedString,
    },
    Skipped {
        id: SharedString,
    },
    Advanced {
        id: SharedString,
    },
}

/// A short sequence of single-select questions.
#[derive(IntoElement)]
pub struct QuestionFlow {
    id: SharedString,
    title: SharedString,
    questions: Vec<Question>,
    question_prefix: SharedString,
    counter_of: SharedString,
    skip_label: SharedString,
    continue_label: SharedString,
    finish_label: SharedString,
    kicker: Option<SharedString>,
    card_options: bool,
    no_selection_label: SharedString,
    style: StyleRefinement,
    on_event: Option<EventHandler<QuestionFlowEvent>>,
}

impl QuestionFlow {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            questions: Vec::new(),
            question_prefix: "Question".into(),
            counter_of: "of".into(),
            skip_label: "Cancel".into(),
            continue_label: "Confirm".into(),
            finish_label: "Confirm".into(),
            kicker: None,
            card_options: false,
            no_selection_label: "No selection".into(),
            style: StyleRefinement::default(),
            on_event: None,
        }
    }

    pub fn questions(mut self, questions: impl IntoIterator<Item = Question>) -> Self {
        self.questions = questions.into_iter().collect();
        self
    }

    pub fn question_prefix(mut self, label: impl Into<SharedString>) -> Self {
        self.question_prefix = label.into();
        self
    }

    pub fn counter_of(mut self, label: impl Into<SharedString>) -> Self {
        self.counter_of = label.into();
        self
    }

    pub fn skip_label(mut self, label: impl Into<SharedString>) -> Self {
        self.skip_label = label.into();
        self
    }

    pub fn continue_label(mut self, label: impl Into<SharedString>) -> Self {
        self.continue_label = label.into();
        self
    }

    pub fn finish_label(mut self, label: impl Into<SharedString>) -> Self {
        self.finish_label = label.into();
        self
    }

    pub fn kicker(mut self, kicker: impl Into<SharedString>) -> Self {
        self.kicker = Some(kicker.into());
        self
    }

    pub fn card_options(mut self, card_options: bool) -> Self {
        self.card_options = card_options;
        self
    }

    pub fn no_selection_label(mut self, label: impl Into<SharedString>) -> Self {
        self.no_selection_label = label.into();
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&QuestionFlowEvent, &mut Window, &mut gpui::App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for QuestionFlow {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for QuestionFlow {
    fn render(self, _: &mut Window, cx: &mut gpui::App) -> impl IntoElement {
        let question = self.questions.first().cloned();
        let selected = question
            .as_ref()
            .and_then(|question| question.answered.clone());
        let handler = self.on_event;
        let id = self.id.clone();
        let option_rows = question
            .as_ref()
            .map(|question| {
                question
                    .options
                    .iter()
                    .enumerate()
                    .map(|(index, option)| {
                        let selected_option = selected.as_ref() == Some(option.id());
                        let event = QuestionFlowEvent::Answered {
                            id: id.clone(),
                            option: option.id().clone(),
                        };
                        let handler = handler.clone();
                        let mut row = h_flex()
                            .id(ElementId::Name(
                                format!("question-option-{id}-{index}").into(),
                            ))
                            .role(Role::RadioButton)
                            .aria_label(option.label().clone())
                            .aria_selected(selected_option)
                            .w_full()
                            .items_center()
                            .gap_2()
                            .rounded(px(7.))
                            .border_1()
                            .border_color(if selected_option {
                                cx.theme().primary
                            } else {
                                cx.theme().border
                            })
                            .bg(if selected_option {
                                cx.theme().accent
                            } else {
                                cx.theme().popover
                            })
                            .px_3()
                            .py_2()
                            .cursor_pointer()
                            .hover(|style| style.bg(cx.theme().muted))
                            .child(
                                div()
                                    .size(px(8.))
                                    .rounded_full()
                                    .border_1()
                                    .border_color(if selected_option {
                                        cx.theme().primary
                                    } else {
                                        cx.theme().muted_foreground
                                    })
                                    .when(selected_option, |dot| dot.bg(cx.theme().primary)),
                            )
                            .child(
                                div()
                                    .min_w_0()
                                    .flex_1()
                                    .truncate()
                                    .text_sm()
                                    .child(option.label().clone()),
                            );
                        if let Some(handler) = handler {
                            row = row.on_click(move |_: &ClickEvent, window, cx| {
                                handler(&event, window, cx)
                            });
                        }
                        row.into_any_element()
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let skip_event = QuestionFlowEvent::Skipped { id: id.clone() };
        let confirm_event = QuestionFlowEvent::Completed { id: id.clone() };
        let skip_handler = handler.clone();
        let confirm_handler = handler;
        let skip = div()
            .id(ElementId::Name(format!("question-skip-{id}").into()))
            .debug_selector({
                let id = id.clone();
                move || format!("question-skip-{id}")
            })
            .role(Role::Button)
            .aria_label(self.skip_label.clone())
            .cursor_pointer()
            .rounded(px(6.))
            .border_1()
            .border_color(cx.theme().border)
            .px_3()
            .py_1()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(self.skip_label.clone())
            .when_some(skip_handler, |button, handler| {
                button.on_click(move |_: &ClickEvent, window, cx| handler(&skip_event, window, cx))
            });
        let confirm_enabled = selected.is_some();
        let mut confirm = div()
            .id(ElementId::Name(format!("question-confirm-{id}").into()))
            .debug_selector({
                let id = id.clone();
                move || format!("question-confirm-{id}")
            })
            .role(Role::Button)
            .aria_label(self.finish_label.clone())
            .rounded(px(6.))
            .px_3()
            .py_1()
            .text_xs()
            .child(self.finish_label.clone());
        if confirm_enabled {
            confirm = confirm
                .cursor_pointer()
                .bg(cx.theme().primary)
                .text_color(cx.theme().primary_foreground)
                .when_some(confirm_handler, |button, handler| {
                    button.on_click(move |_: &ClickEvent, window, cx| {
                        handler(&confirm_event, window, cx)
                    })
                });
        } else {
            confirm = confirm
                .bg(cx.theme().muted)
                .text_color(cx.theme().muted_foreground)
                .opacity(0.6);
        }
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Group)
            .aria_label(self.title.clone())
            .w_full()
            .gap_2()
            .when_some(self.kicker, |card, kicker| {
                card.child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(10.))
                        .text_color(cx.theme().muted_foreground)
                        .child(kicker),
                )
            })
            .child(div().text_sm().font_semibold().child(self.title))
            .when_some(question.as_ref(), |card, question| {
                card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(question.prompt.clone()),
                )
                .when(!question.options.is_empty(), |card| {
                    card.child(v_flex().gap_1().children(option_rows))
                })
                .when(question.options.is_empty(), |card| {
                    card.child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.no_selection_label.clone()),
                    )
                })
            })
            .child(h_flex().justify_end().gap_2().child(skip).child(confirm))
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    use gpui::{Context, Render, TestAppContext, div, px, size};

    use super::*;

    struct QuestionProbe {
        events: Rc<RefCell<Vec<QuestionFlowEvent>>>,
        answered: Rc<Cell<bool>>,
    }

    impl Render for QuestionProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let events = self.events.clone();
            let answered = self.answered.get();
            div().w(px(600.)).child(
                QuestionFlow::new("flow-test", "Choose")
                    .questions([Question::new("question", "Pick one")
                        .optional(true)
                        .answered(answered.then(|| "one".into()))
                        .options([
                            ChoiceOption::new("one", "One"),
                            ChoiceOption::new("two", "Two"),
                        ])])
                    .on_event(move |event, _, _| events.borrow_mut().push(event.clone())),
            )
        }
    }

    #[gpui::test]
    fn confirm_is_disabled_until_an_option_is_selected(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let events = Rc::new(RefCell::new(Vec::new()));
        let answered = Rc::new(Cell::new(false));
        let (probe, window) = cx.add_window_view({
            let events = events.clone();
            let answered = answered.clone();
            move |_, _| QuestionProbe { events, answered }
        });
        window.simulate_resize(size(px(800.), px(500.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let confirm = window
            .debug_bounds("question-confirm-flow-test")
            .expect("confirm control");
        window.simulate_click(confirm.center(), gpui::Modifiers::default());
        window.run_until_parked();
        assert!(
            events.borrow().is_empty(),
            "disabled Confirm must not emit a completion"
        );
        window.update(|_, cx| {
            answered.set(true);
            probe.update(cx, |_, cx| cx.notify());
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let confirm = window
            .debug_bounds("question-confirm-flow-test")
            .expect("confirm control");
        window.simulate_click(confirm.center(), gpui::Modifiers::default());
        window.run_until_parked();
        assert!(
            events
                .borrow()
                .iter()
                .any(|event| matches!(event, QuestionFlowEvent::Completed { .. })),
            "an answered question must complete"
        );
    }
}
