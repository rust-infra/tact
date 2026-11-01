//! FineTuneCard's controls, typed events, and constrained-layout reachability.
//!
//! Three probes share the family: a roomy host for the event contract, a narrow
//! one for horizontal containment, and a short one for the scroll path that
//! keeps Apply reachable below the fold.

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, Modifiers, ParentElement as _,
    Render, ScrollDelta, ScrollWheelEvent, Styled as _, Subscription, TestAppContext,
    VisualTestContext, Window, div, point, px,
};
use gpui_ai::prelude::{FineTuneCard, FineTuneEvent, FineTuneTypeface, FineTuneValues};
use std::{cell::RefCell, rc::Rc};

struct PublicFineTuneProbe {
    card: Entity<FineTuneCard>,
    events: Rc<RefCell<Vec<FineTuneEvent>>>,
    _subscription: Subscription,
}

struct ConstrainedFineTuneProbe {
    card: Entity<FineTuneCard>,
}

struct NarrowFineTuneProbe {
    card: Entity<FineTuneCard>,
}

impl PublicFineTuneProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let card = cx.new(|cx| {
            FineTuneCard::new(
                "public-fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter-regular")
                    .accent(gpui::hsla(0.58, 0.75, 0.52, 1.)),
                [
                    FineTuneTypeface::new("inter-regular", "Inter"),
                    FineTuneTypeface::new("inter-display", "Inter"),
                ],
                window,
                cx,
            )
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let subscription = cx.subscribe(&card, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            card,
            events,
            _subscription: subscription,
        }
    }
}

impl Render for PublicFineTuneProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .debug_selector(|| "public-fine-tune-host".to_owned())
            .w(px(420.))
            .h(px(520.))
            .child(self.card.clone())
    }
}

impl ConstrainedFineTuneProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let card = cx.new(|cx| {
            FineTuneCard::new(
                "constrained-fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter"),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });
        Self { card }
    }
}

impl Render for ConstrainedFineTuneProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .debug_selector(|| "constrained-fine-tune-host".to_owned())
            .w(px(420.))
            .h(px(220.))
            .overflow_hidden()
            .child(self.card.clone())
    }
}

impl NarrowFineTuneProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let card = cx.new(|cx| {
            FineTuneCard::new(
                "narrow-fine-tune",
                FineTuneValues::new(320., 180., 24., 0.72, "inter")
                    .accent(gpui::hsla(0.58, 0.75, 0.52, 1.)),
                [FineTuneTypeface::new("inter", "Inter")],
                window,
                cx,
            )
        });
        Self { card }
    }
}

impl Render for NarrowFineTuneProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .debug_selector(|| "narrow-fine-tune-host".to_owned())
            .w(px(216.))
            .h(px(520.))
            .overflow_hidden()
            .child(self.card.clone())
    }
}

#[gpui::test]
fn public_fine_tune_reset_and_apply_emit_stable_card_identity(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicFineTuneProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let reset = cx
        .debug_bounds("fine-tune-reset")
        .expect("reset should be a real rendered control");
    let apply = cx
        .debug_bounds("fine-tune-apply")
        .expect("apply should be a real rendered control");
    cx.simulate_click(reset.center(), Modifiers::default());
    cx.simulate_click(apply.center(), Modifiers::default());

    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            FineTuneEvent::ResetRequested {
                id: "public-fine-tune".into(),
            },
            FineTuneEvent::ApplyRequested {
                id: "public-fine-tune".into(),
            },
        ]
    );
}

#[gpui::test]
fn fine_tune_presentation_uses_theme_typography_tokens(cx: &mut TestAppContext) {
    use gpui_component::ActiveTheme as _;
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(PublicFineTuneProbe::new);
    for rem in [12., 24.] {
        cx.update(|window, cx| {
            window.set_rem_size(px(rem));
            window.draw(cx).clear(cx);
        });
        let field = cx
            .debug_bounds("fine-tune-width-input")
            .expect("Width field");
        let editor = cx
            .debug_bounds("fine-tune-width-editor")
            .expect("Width editor");
        let (line_height, gap) = cx.update(|_, cx| {
            let tokens = cx.theme().semantic_tokens();
            (tokens.typography.sm.line_height, tokens.spacing.xs)
        });
        assert_eq!(
            field.size.height - editor.size.height - gap,
            line_height,
            "the label must keep its full token line box at rem={rem}"
        );
    }
}

#[gpui::test]
fn identical_typeface_labels_activate_the_selected_stable_id(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicFineTuneProbe::new);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let trigger = cx
        .debug_bounds("fine-tune-typeface")
        .expect("typeface trigger");
    cx.simulate_click(trigger.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("down down enter");
    assert!(matches!(
        probe.read_with(cx, |probe, _| probe.events.borrow().last().cloned()),
        Some(FineTuneEvent::TypefaceChanged { id, typeface_id })
            if id == "public-fine-tune" && typeface_id == "inter-display"
    ));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_click(trigger.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_keystrokes("down enter");
    assert!(matches!(
        probe.read_with(cx, |probe, _| probe.events.borrow().last().cloned()),
        Some(FineTuneEvent::TypefaceChanged { id, typeface_id })
            if id == "public-fine-tune" && typeface_id == "inter-regular"
    ));
}

#[gpui::test]
fn public_fine_tune_rendered_clear_accent_and_slider_keyboard_paths_emit_typed_events(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicFineTuneProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("fine-tune-typeface").is_some());
    let clear_accent = cx
        .debug_bounds("fine-tune-clear-accent")
        .expect("the populated accent should expose a named clear control");
    cx.simulate_click(clear_accent.center(), Modifiers::default());
    assert!(matches!(
        probe.read_with(cx, |probe, _| probe.events.borrow().last().cloned()),
        Some(FineTuneEvent::AccentChanged { id, accent: None })
            if id == "public-fine-tune"
    ));

    let slider = cx
        .debug_bounds("fine-tune-opacity-slider")
        .expect("the named opacity slider should render");
    cx.simulate_click(slider.center(), Modifiers::default());
    probe.update(cx, |probe, _| probe.events.borrow_mut().clear());
    cx.simulate_keystrokes("right");
    cx.run_until_parked();

    let events = probe.read_with(cx, |probe, _| probe.events.borrow().clone());
    assert_eq!(events.len(), 1);
    assert!(matches!(
        &events[0],
        FineTuneEvent::OpacityChanged { id, opacity }
            if id == "public-fine-tune" && *opacity > 0.5
    ));
}

#[gpui::test]
fn public_fine_tune_empty_typeface_catalog_cannot_open_a_popup(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicFineTuneProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| {
        probe.update(cx, |probe, cx| {
            probe.card.update(cx, |card, cx| card.set_typefaces([], cx));
        });
        window.draw(cx).clear(cx);
    });

    let typeface = cx
        .debug_bounds("fine-tune-typeface")
        .expect("the empty typeface state should remain visible");
    assert!(cx.debug_bounds("popup-content").is_none());
    cx.simulate_click(typeface.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("popup-content").is_none());
    assert!(probe.read_with(cx, |probe, _| probe.events.borrow().is_empty()));
}

#[gpui::test]
fn public_fine_tune_keeps_controls_inside_a_narrow_card(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(NarrowFineTuneProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let host = cx
        .debug_bounds("narrow-fine-tune-host")
        .expect("the narrow Fine-tune host should render");
    for selector in [
        "fine-tune-clear-accent",
        "fine-tune-reset",
        "fine-tune-apply",
    ] {
        let control = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("{selector} should render"));
        assert!(control.left() >= host.left(), "{selector}: {control:?}");
        assert!(control.right() <= host.right(), "{selector}: {control:?}");
    }
}

#[gpui::test]
fn public_fine_tune_keeps_apply_reachable_in_a_constrained_viewport(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(ConstrainedFineTuneProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let host = cx
        .debug_bounds("constrained-fine-tune-host")
        .expect("constrained FineTune host should render");
    let initial_apply = cx
        .debug_bounds("fine-tune-apply")
        .expect("Apply should remain laid out below the fold");
    assert!(initial_apply.bottom() > host.bottom());

    for _ in 0..8 {
        cx.simulate_event(ScrollWheelEvent {
            position: host.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
            ..Default::default()
        });
    }
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let final_apply = cx
        .debug_bounds("fine-tune-apply")
        .expect("Apply should remain rendered after scrolling");
    assert!(
        final_apply.top() >= host.top(),
        "{final_apply:?} vs {host:?}"
    );
    assert!(
        final_apply.bottom() <= host.bottom(),
        "{final_apply:?} vs {host:?}"
    );
}
