//! What a sequence of questions owes: a place stated plainly, no way past a
//! question it still needs, and a typed report of everything a person asked
//! for.

use gpui::{
    Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext,
    VisualTestContext, Window, div, px,
};
use gpui_ai::prelude::{ChoiceOption, Question, QuestionFlow, QuestionFlowEvent};
use std::sync::{Arc, Mutex};

struct Probe {
    step: usize,
    answered: bool,
    events: Arc<Mutex<Vec<QuestionFlowEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let events = self.events.clone();
        let first = Question::new("flavours", "How many flavours should we launch?")
            .options([
                ChoiceOption::new("three", "Three").description("The core line"),
                ChoiceOption::new("five", "Five"),
            ])
            .answered(self.answered.then(|| "three".into()));
        let second = Question::new("market", "Which market do we enter first?")
            .options([ChoiceOption::new("trucks", "Food trucks")]);
        div().w(px(420.)).child(
            QuestionFlow::new("launch", "Before I draft the plan")
                .questions([first, second])
                .step(self.step)
                .on_event(move |event, _, _| {
                    events.lock().expect("events mutex").push(event.clone())
                }),
        )
    }
}

fn probe(cx: &mut TestAppContext, step: usize, answered: bool) -> &mut VisualTestContext {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(move |_, _| Probe {
        step,
        answered,
        events: Arc::new(Mutex::new(Vec::new())),
    });
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx
}

/// The flow says where a person is, because nothing else does.
///
/// A sequence with no count is a sequence of unknown length: a reader cannot
/// tell a second question from a last one, and so cannot tell whether it is
/// worth starting.
#[gpui::test]
fn the_flow_states_the_place_in_the_sequence(cx: &mut TestAppContext) {
    let cx = probe(cx, 0, false);
    assert!(
        cx.debug_bounds("question-flow-counter-launch").is_some(),
        "the counter is part of the flow, not a decoration a caller adds"
    );
}

/// A step past the end shows the last question rather than nothing.
///
/// An application that advances one step too far — an off-by-one, a stale
/// step held across a shorter set of questions — should see the end of the
/// sequence, not an empty card where a question was.
#[gpui::test]
fn a_step_past_the_end_shows_the_last_question(cx: &mut TestAppContext) {
    let cx = probe(cx, 99, false);
    assert!(
        cx.debug_bounds("question-flow-launch").is_some(),
        "the flow still draws"
    );
    assert!(
        cx.debug_bounds("choice-launch-market-trucks").is_some(),
        "and it is the last question that is shown"
    );
}

/// An unanswered question offers a way past it, and no way through it.
///
/// The distinction is the whole point of the pair: skipping records that a
/// question did not apply, and continuing records an answer. A flow that let
/// a person continue without answering would report neither.
#[gpui::test]
fn a_question_is_skippable_until_it_is_answered(cx: &mut TestAppContext) {
    let unanswered = probe(cx, 0, false);
    assert!(
        unanswered
            .debug_bounds("question-flow-skip-launch")
            .is_some(),
        "an unanswered question offers the way past"
    );

    let answered = probe(cx, 0, true);
    assert!(
        answered.debug_bounds("question-flow-skip-launch").is_none(),
        "an answered question has nothing to skip"
    );
    assert!(
        answered
            .debug_bounds("question-flow-advance-launch")
            .is_some(),
        "and offers the way on"
    );
}
