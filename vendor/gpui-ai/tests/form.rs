//! What a form control owes: the right thing announced, the right thing
//! reported, and an indicator that lines up with the text beside it.

use gpui::{
    App, Context, Element as _, IntoElement, ParentElement as _, Render, RenderOnce as _, Role,
    Styled as _, TestAppContext, VisualTestContext, Window, accesskit, canvas, div, px,
};
use gpui_ai::prelude::{ChoiceEvent, ChoiceGroup, ChoiceOption};
use std::sync::{Arc, Mutex};

/// A control's role and its accessibility node.
///
/// Read from the element the builder renders, not from the builder: these
/// are `RenderOnce`, and the role belongs to what comes out of `render`.
/// Type-erasing that into an `AnyElement` first would drop it.
struct Captured {
    role: Option<Role>,
    node: accesskit::Node,
}

/// Reads the role and node out of one rendered control.
fn read<E: IntoElement>(element: E) -> Captured {
    let element = element.into_element();
    let role = element.a11y_role();
    let mut node = accesskit::Node::new(Role::Unknown);
    element.write_a11y_info(&mut node);
    Captured { role, node }
}

type Capture = std::rc::Rc<dyn Fn(&mut Window, &mut App) -> Captured>;

struct Probe {
    capture: Capture,
    captured: Arc<Mutex<Option<Captured>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let captured = self.captured.clone();
        let capture = self.capture.clone();
        canvas(
            move |_, window, cx| {
                *captured.lock().expect("capture mutex") = Some(capture(window, cx));
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(cx: &mut TestAppContext, capture: Capture) -> Captured {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Probe { capture, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex")
        .take()
        .expect("the control writes an accessibility node")
}

/// A group of choices is a radio group, and says how many it offers.
///
/// Announced rather than merely drawn: a set of options a screen reader
/// cannot count is a set nobody can navigate.
#[gpui::test]
fn a_choice_group_announces_itself_and_its_size(cx: &mut TestAppContext) {
    let captured = capture(
        cx,
        std::rc::Rc::new(|window, cx| {
            read(
                ChoiceGroup::new("flavours", "How many flavours?")
                    .options([
                        ChoiceOption::new("three", "Three"),
                        ChoiceOption::new("five", "Five"),
                        ChoiceOption::new("one", "Just one"),
                    ])
                    .selected("five")
                    .render(window, cx),
            )
        }),
    );

    assert_eq!(captured.role, Some(Role::RadioGroup));
    assert_eq!(captured.node.label(), Some("How many flavours?"));
    assert_eq!(captured.node.size_of_set(), Some(3));
}

struct ChoiceProbe {
    chosen: Arc<Mutex<Vec<String>>>,
}

impl Render for ChoiceProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let chosen = self.chosen.clone();
        div().w(px(320.)).child(
            ChoiceGroup::new("flavours", "How many flavours?")
                .options([
                    ChoiceOption::new("three", "Three").description("The core line"),
                    ChoiceOption::new("five", "Five"),
                ])
                .selected("three")
                .on_event(move |event, _, _| {
                    let ChoiceEvent::Chosen { option, .. } = event;
                    chosen
                        .lock()
                        .expect("chosen mutex")
                        .push(option.to_string());
                }),
        )
    }
}

/// An option's indicator sits on the label's first line, not the middle of
/// a block, and every option shares one text inset.
///
/// The ragged left edge this prevents is the misalignment class the 0.4.0
/// audit found across eight components: an option with a second line would
/// otherwise centre its ring against both lines and step its label in.
#[gpui::test]
fn every_option_keeps_one_text_inset(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|_, _| ChoiceProbe {
        chosen: Arc::new(Mutex::new(Vec::new())),
    });
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let with_description = cx
        .debug_bounds("choice-flavours-three")
        .expect("the described option renders");
    let without = cx
        .debug_bounds("choice-flavours-five")
        .expect("the plain option renders");

    assert_eq!(
        with_description.origin.x, without.origin.x,
        "options must share a left edge"
    );
    assert!(
        with_description.size.height > without.size.height,
        "the described option is the taller one, so this compares the two cases"
    );
}

/// An indicator sits on the middle of the label's first line.
///
/// The test above checks the left edge, which is what the 0.4.0 audit was
/// about, and it passed while every indicator in this file sat two pixels
/// above the words beside it. The seat was a square the size of the *control*
/// - sixteen pixels - where it should have been the height of the *line* the
/// control belongs to, which is twenty. So this measures the one thing the
/// other does not: the middle of the seat against the middle of the first
/// line, for an option with a second line and for an option without, because
/// a seat centred on the whole option drifts only when there is one.
#[gpui::test]
fn an_indicator_sits_on_the_middle_of_the_first_line(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|_, _| ChoiceProbe {
        chosen: Arc::new(Mutex::new(Vec::new())),
    });
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    for (option, seat_name, label_name) in [
        (
            "three",
            "choice-seat-flavours-three",
            "choice-label-flavours-three",
        ),
        (
            "five",
            "choice-seat-flavours-five",
            "choice-label-flavours-five",
        ),
    ] {
        let seat = cx
            .debug_bounds(seat_name)
            .unwrap_or_else(|| panic!("{option} renders a seat"));
        let label = cx
            .debug_bounds(label_name)
            .unwrap_or_else(|| panic!("{option} renders a label"));

        assert!(
            (seat.center().y - label.center().y).abs() <= px(0.5),
            "{option}'s indicator must sit on its first line: seat centred at {:?}, the line \
             at {:?}",
            seat.center().y,
            label.center().y
        );
        assert_eq!(
            seat.size.height, label.size.height,
            "{option}'s seat must be the line it sits on, not the control it holds"
        );
    }
}
