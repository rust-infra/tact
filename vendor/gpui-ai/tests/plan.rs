//! Plan cards: accessible naming, typed decisions and step activation,
//! keyboard reach, and resolved states without controls.

use gpui::{
    Context, Element as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement as _, Render, RenderOnce as _, Role, Styled as _, TestAppContext,
    VisualTestContext, Window, accesskit, canvas, div, px, size,
};
use gpui_ai::plan::{PlanCard, PlanEvent, PlanState, PlanStep, PlanStepStatus};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

fn steps() -> [PlanStep; 4] {
    [
        PlanStep::new("compare", "Compare unit prices").status(PlanStepStatus::Done),
        PlanStep::new("risk", "Check delivery risk").status(PlanStepStatus::Done),
        PlanStep::new("draft", "Draft the order").status(PlanStepStatus::Running),
        PlanStep::new("send", "Send confirmations").detail("Emails 3 suppliers"),
    ]
}

fn plan(state: PlanState) -> PlanCard {
    PlanCard::new("rollout", "Switch bulk orders")
        .description("Four steps; the last one sends email.")
        .steps(steps())
        .state(state)
        .editable(true)
}

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

struct A11yProbe {
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for A11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                let element = plan(PlanState::Proposed)
                    .on_event(|_, _, _| {})
                    .render(window, cx)
                    .into_element();
                let role = element.a11y_role();
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

#[gpui::test]
fn the_plan_is_a_group_named_by_title_progress_and_state(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let (_, cx) = cx.add_window_view({
        let captured = captured.clone();
        move |_, _| A11yProbe { captured }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let captured = captured
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("probe should capture its element");
    assert_eq!(captured.role, Some(Role::Group));
    assert_eq!(
        captured.node.label(),
        Some("Plan: Switch bulk orders, 2 of 4 steps done, proposed")
    );
    assert_eq!(
        captured.node.description(),
        Some("Four steps; the last one sends email.")
    );
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Proposed,
    Approved,
    Static,
}

struct Probe {
    kind: ProbeKind,
    events: Rc<RefCell<Vec<PlanEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        let handler = move |event: &PlanEvent, _: &mut Window, _: &mut gpui::App| {
            events.borrow_mut().push(event.clone());
        };
        match self.kind {
            ProbeKind::Proposed => div()
                .size_full()
                .child(plan(PlanState::Proposed).on_event(handler)),
            ProbeKind::Approved => div().size_full().child(
                plan(PlanState::Approved)
                    .note("Approved by Oscar")
                    .on_event(handler),
            ),
            ProbeKind::Static => div().size_full().child(plan(PlanState::Completed)),
        }
    }
}

fn harness(
    kind: ProbeKind,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<PlanEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| Probe { kind, events }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(640.), px(560.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (events, cx)
}

fn activate_key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("test key should parse");
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
}

fn click_center(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} should render"));
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn decisions_and_step_activation_report_stable_ids(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Proposed, cx);
    click_center(cx, "plan-step-rollout-send");
    click_center(cx, "plan-edit-rollout");
    click_center(cx, "plan-reject-rollout");
    click_center(cx, "plan-approve-rollout");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            PlanEvent::StepActivated {
                id: "rollout".into(),
                step_id: "send".into()
            },
            PlanEvent::EditRequested {
                id: "rollout".into()
            },
            PlanEvent::Rejected {
                id: "rollout".into()
            },
            PlanEvent::Approved {
                id: "rollout".into()
            },
        ]
    );
}

#[gpui::test]
fn keyboard_reaches_the_first_step(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Proposed, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    activate_key(cx, "enter");
    assert_eq!(
        events.borrow().as_slice(),
        &[PlanEvent::StepActivated {
            id: "rollout".into(),
            step_id: "compare".into()
        }]
    );
}

#[gpui::test]
fn decided_plans_drop_their_controls(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Approved, cx);
    assert!(cx.debug_bounds("plan-approve-rollout").is_none());
    assert!(cx.debug_bounds("plan-reject-rollout").is_none());
    assert!(cx.debug_bounds("plan-edit-rollout").is_none());
    assert!(cx.debug_bounds("plan-step-rollout-draft").is_some());

    let (_, cx) = harness(ProbeKind::Static, cx);
    assert!(cx.debug_bounds("plan-rollout").is_some());
    assert!(cx.debug_bounds("plan-approve-rollout").is_none());
}
