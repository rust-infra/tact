//! Message queue: accessible list naming, typed reorder / send / remove /
//! clear events with disabled ends, keyboard reach, and bounded width.

use gpui::{
    Context, Element as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement as _, Render, RenderOnce as _, Role, Styled as _, TestAppContext,
    VisualTestContext, Window, accesskit, canvas, div, px, size,
};
use gpui_ai::queue::{MessageQueue, QueueEvent, QueuedMessage};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

fn items() -> [QueuedMessage; 3] {
    [
        QueuedMessage::new("a", "Compare the three suppliers").note("after the current step"),
        QueuedMessage::new("b", "Draft the order confirmations"),
        QueuedMessage::new(
            "c",
            "Summarize every risk we found in the cold-chain review and propose mitigations for each one",
        ),
    ]
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
                let element = MessageQueue::new("queue")
                    .items(items())
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
fn the_queue_is_a_list_that_counts_its_items(cx: &mut TestAppContext) {
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
    assert_eq!(captured.role, Some(Role::List));
    assert_eq!(captured.node.label(), Some("Queued messages, 3 waiting"));
}

struct Probe {
    narrow: bool,
    events: Rc<RefCell<Vec<QueueEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        let queue = MessageQueue::new("queue")
            .items(items())
            .editable(true)
            .on_event(move |event, _, _| events.borrow_mut().push(event.clone()));
        if self.narrow {
            div().size_full().child(div().w(px(300.)).child(queue))
        } else {
            div().size_full().child(queue)
        }
    }
}

fn harness(
    narrow: bool,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<QueueEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| Probe { narrow, events }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(640.), px(400.)));
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
fn controls_report_stable_ids_and_respect_the_ends(cx: &mut TestAppContext) {
    let (events, cx) = harness(false, cx);
    click_center(cx, "queue-up-queue-a");
    click_center(cx, "queue-down-queue-c");
    click_center(cx, "queue-down-queue-a");
    click_center(cx, "queue-up-queue-c");
    click_center(cx, "queue-edit-queue-b");
    click_center(cx, "queue-send-queue-b");
    click_center(cx, "queue-remove-queue-c");
    click_center(cx, "queue-clear-queue");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            QueueEvent::MovedDown { id: "a".into() },
            QueueEvent::MovedUp { id: "c".into() },
            QueueEvent::EditRequested { id: "b".into() },
            QueueEvent::SentNow { id: "b".into() },
            QueueEvent::Removed { id: "c".into() },
            QueueEvent::Cleared,
        ]
    );
}

#[gpui::test]
fn keyboard_reaches_the_clear_control_first(cx: &mut TestAppContext) {
    let (events, cx) = harness(false, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    activate_key(cx, "enter");
    assert_eq!(events.borrow().as_slice(), &[QueueEvent::Cleared]);
}

#[gpui::test]
fn long_prompts_truncate_inside_a_narrow_host(cx: &mut TestAppContext) {
    let (_, cx) = harness(true, cx);
    let row = cx.debug_bounds("queue-item-queue-c").expect("row renders");
    assert!(row.size.width <= px(300.), "row width {:?}", row.size.width);
    assert!(cx.debug_bounds("queue-remove-queue-c").is_some());
}

#[gpui::test]
fn an_empty_queue_renders_nothing_visible(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|_, _| EmptyProbe);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("queue-queue").is_none());
}

struct EmptyProbe;

impl Render for EmptyProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div().size_full().child(MessageQueue::new("queue"))
    }
}
