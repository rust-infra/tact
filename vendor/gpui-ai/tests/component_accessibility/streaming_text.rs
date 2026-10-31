//! StreamingText's citation and web-source companions.
//!
//! These probes exercise the reachable, activatable surface of a streamed
//! answer: which references become controls, what typed event each activation
//! emits by pointer and by keyboard, and whether the last one stays inside a
//! root too narrow to hold the row.

use gpui::{
    Context, InteractiveElement as _, Modifiers, ParentElement as _, Render, ScrollDelta,
    ScrollWheelEvent, Styled as _, TestAppContext, VisualTestContext, Window, div, point, px,
};
use gpui_ai::{
    stream::Progressive,
    streaming_text::{CitationRef, SourceRef, StreamingText, StreamingTextEvent},
};
use std::{cell::RefCell, rc::Rc};

use crate::harness::activate_key;

struct PublicCitationProbe {
    events: Rc<RefCell<Vec<StreamingTextEvent>>>,
}

struct BoundedCitationProbe;

impl Render for BoundedCitationProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let citations = [
            CitationRef::new("first", "One", "Open first", "app://first"),
            CitationRef::new("second", "Two", "Open second", "app://second"),
            CitationRef::new("third", "Three", "Open third", "app://third"),
            CitationRef::new("fourth", "Four", "Open fourth", "app://fourth"),
            CitationRef::new("final", "Final", "Open final", "app://final"),
        ];
        div()
            .debug_selector(|| "bounded-citation-host".to_owned())
            .w(px(184.))
            .h(px(160.))
            .overflow_hidden()
            .child(
                StreamingText::new(
                    "bounded-citations",
                    &Progressive::complete(
                        "[[cite:first]] [[cite:second]] [[cite:third]] [[cite:fourth]] [[cite:final]]"
                            .into(),
                    ),
                )
                .citations(citations)
                .on_event(|_, _, _| {}),
            )
    }
}

impl Render for PublicCitationProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        StreamingText::new(
            "citation-answer",
            &Progressive::complete(
                "[[cite:pricing]] changed while supply held [[cite:supply]].".into(),
            ),
        )
        .citations([
            CitationRef::new(
                "pricing",
                "Pricing report",
                "Open the pricing report",
                "app://reports/pricing",
            ),
            CitationRef::new(
                "supply",
                "Supply report",
                "Open the supply report",
                "app://reports/supply",
            ),
        ])
        .on_event(move |event, _, _| events.borrow_mut().push(event.clone()))
    }
}

struct PublicSourceProbe {
    events: Rc<RefCell<Vec<StreamingTextEvent>>>,
}

impl Render for PublicSourceProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        div().size_full().child(
            StreamingText::new("sourced", &Progressive::complete("Forty-two".into()))
                .source_refs([
                    SourceRef::new("pricing.md"),
                    SourceRef::with_id("dairy-index", "Dairy index")
                        .url("https://www.dairyreport.org/index"),
                ])
                .on_event(move |event, _, _| events.borrow_mut().push(event.clone())),
        )
    }
}

#[gpui::test]
fn public_web_sources_activate_typed_events_while_files_stay_static(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let (_, cx) = cx.add_window_view(move |_, _| PublicSourceProbe { events });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("streaming-source-pricing.md").is_none(),
        "a source without a location is a static chip, never a dead button"
    );
    let chip = cx
        .debug_bounds("streaming-source-dairy-index")
        .expect("web source should be an activatable chip");
    cx.simulate_click(chip.center(), Modifiers::default());
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::SourceActivated {
            id: "dairy-index".into(),
            url: "https://www.dairyreport.org/index".into(),
        }]
    );
    captured.borrow_mut().clear();

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::SourceActivated {
            id: "dairy-index".into(),
            url: "https://www.dairyreport.org/index".into(),
        }]
    );
}

#[gpui::test]
fn public_streaming_citation_companions_activate_typed_events(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let (_, cx) = cx.add_window_view(move |_, _| PublicCitationProbe { events });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let body = cx
        .debug_bounds("streaming-text-body")
        .expect("selectable Markdown body should render");
    cx.simulate_click(
        point(body.left() + px(3.), body.top() + px(8.)),
        Modifiers::default(),
    );
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::CitationActivated {
            id: "pricing".into(),
            destination: "app://reports/pricing".into(),
        }]
    );
    captured.borrow_mut().clear();

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::CitationActivated {
            id: "pricing".into(),
            destination: "app://reports/pricing".into(),
        }]
    );
    captured.borrow_mut().clear();

    let pricing = cx
        .debug_bounds("streaming-citation-pricing")
        .expect("resolved citation companion should render");
    cx.simulate_click(pricing.center(), Modifiers::default());
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::CitationActivated {
            id: "pricing".into(),
            destination: "app://reports/pricing".into(),
        }]
    );

    captured.borrow_mut().clear();
    activate_key(cx, "enter");
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::CitationActivated {
            id: "pricing".into(),
            destination: "app://reports/pricing".into(),
        }]
    );
    assert!(cx.debug_bounds("streaming-citation-supply").is_some());
}

#[gpui::test]
fn streaming_citation_companions_keep_the_final_link_reachable_in_a_narrow_root(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|_, _| BoundedCitationProbe);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let host = cx
        .debug_bounds("bounded-citation-host")
        .expect("bounded citation host should render");
    let scroller = cx
        .debug_bounds("streaming-citation-scroll")
        .expect("citation scroller should render");
    cx.simulate_mouse_move(scroller.center(), None, Modifiers::default());

    for _ in 0..12 {
        cx.simulate_event(ScrollWheelEvent {
            position: scroller.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            ..Default::default()
        });
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let final_link = cx
        .debug_bounds("streaming-citation-final")
        .expect("final citation should remain rendered after scrolling");
    assert!(
        final_link.left() >= host.left() && final_link.right() <= host.right(),
        "{final_link:?} vs {host:?}"
    );
}
