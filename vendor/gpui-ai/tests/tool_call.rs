use gpui::{
    Context, Element as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement as _, Render, RenderOnce as _, Role, Styled as _, TestAppContext,
    VisualTestContext, Window, accesskit, canvas, px,
};
use gpui_ai::{
    stream::Progressive,
    tool_call::{ToolApproval, ToolCall, ToolCallEvent, ToolGroup, ToolGroupEvent, ToolInvocation},
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Running,
    AwaitingApproval,
    Failed,
    Group,
}

struct A11yProbe {
    kind: ProbeKind,
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

fn invocation() -> ToolInvocation {
    ToolInvocation::new("read-1", "read_file")
        .summary("pricing.md")
        .input("{ \"path\": \"pricing.md\" }")
        .output("Read **214** lines.")
}

impl Render for A11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let kind = self.kind;
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                macro_rules! write_element {
                    ($element:expr) => {{
                        let element = $element.into_element();
                        let role = element.a11y_role();
                        element.write_a11y_info(&mut node);
                        role
                    }};
                }
                let role = match kind {
                    ProbeKind::Running => write_element!(
                        ToolCall::new(&Progressive::running(invocation()))
                            .on_event(|_, _, _| {})
                            .render(window, cx)
                    ),
                    ProbeKind::AwaitingApproval => write_element!(
                        ToolCall::new(&Progressive::pending(
                            ToolInvocation::new("send-1", "send_email")
                                .approval(ToolApproval::Requested),
                        ))
                        .on_event(|_, _, _| {})
                        .render(window, cx)
                    ),
                    ProbeKind::Failed => write_element!(
                        ToolCall::new(&Progressive::failed(invocation(), "Connection timed out"))
                            .render(window, cx)
                    ),
                    ProbeKind::Group => write_element!(
                        ToolGroup::new("burst")
                            .count(2)
                            .active(true)
                            .render(window, cx)
                    ),
                };
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(kind: ProbeKind, cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| A11yProbe { kind, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("component node should be captured")
}

#[gpui::test]
fn tool_call_exposes_name_summary_and_lifecycle_without_color(cx: &mut TestAppContext) {
    let captured = capture(ProbeKind::Running, cx);
    assert_eq!(captured.role, Some(Role::Group));
    assert_eq!(captured.node.label(), Some("read_file pricing.md, Running"));

    let awaiting = capture(ProbeKind::AwaitingApproval, cx);
    assert_eq!(awaiting.node.label(), Some("send_email, awaiting approval"));

    let failed = capture(ProbeKind::Failed, cx);
    assert_eq!(failed.node.label(), Some("read_file pricing.md, Failed"));
    assert_eq!(failed.node.description(), Some("Connection timed out"));

    let group = capture(ProbeKind::Group, cx);
    assert_eq!(group.role, Some(Role::Group));
    assert_eq!(group.node.label(), Some("Running 2 tools…"));
}

struct InteractionProbe {
    approval: ToolApproval,
    open: Option<bool>,
    group_open: Option<bool>,
    events: Rc<RefCell<Vec<ToolCallEvent>>>,
    group_events: Rc<RefCell<Vec<ToolGroupEvent>>>,
}

impl Render for InteractionProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let call = Progressive::pending(
            ToolInvocation::new("send-1", "send_email")
                .summary("3 suppliers")
                .input("{ \"to\": [\"a@example.com\"] }")
                .approval(self.approval),
        );
        let mut card = ToolCall::new(&call).on_event(cx.listener(|this, event, _, cx| {
            match event {
                ToolCallEvent::Toggled { open, .. } => this.open = Some(*open),
                ToolCallEvent::Approved { .. } => this.approval = ToolApproval::Approved,
                ToolCallEvent::Rejected { .. } => this.approval = ToolApproval::Rejected,
            }
            this.events.borrow_mut().push(event.clone());
            cx.notify();
        }));
        if let Some(open) = self.open {
            card = card.open(open);
        }
        let mut group = ToolGroup::new("burst")
            .count(1)
            .active(false)
            .on_event(cx.listener(|this, event: &ToolGroupEvent, _, cx| {
                let ToolGroupEvent::Toggled { open, .. } = event;
                this.group_open = Some(*open);
                this.group_events.borrow_mut().push(event.clone());
                cx.notify();
            }));
        if let Some(open) = self.group_open {
            group = group.open(open);
        }
        gpui::div().size_full().child(group.child(card))
    }
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
}

#[gpui::test]
fn approval_controls_emit_typed_decisions_and_resolve_the_card(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let group_events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let (view, cx) = cx.add_window_view({
        let events = events.clone();
        let group_events = group_events.clone();
        move |_, _| InteractionProbe {
            approval: ToolApproval::Requested,
            open: None,
            group_open: Some(true),
            events,
            group_events,
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Awaiting approval opens the card automatically, so Allow is reachable.
    assert!(cx.debug_bounds("tool-call-body-send-1").is_some());
    click_center(cx, "tool-call-allow-send-1");
    assert_eq!(
        captured.borrow().as_slice(),
        &[ToolCallEvent::Approved {
            id: "send-1".into()
        }]
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // A closed disclosure must not retain focusable or semantic descendants,
    // even during the header's visual transition.
    assert!(cx.debug_bounds("tool-call-allow-send-1").is_none());
    assert!(cx.debug_bounds("tool-call-body-send-1").is_none());
    assert_eq!(
        view.read_with(cx, |probe, _| probe.approval),
        ToolApproval::Approved
    );
}

/// Opening is geometry, not just opacity. A body that appears at its full
/// height and merely fades reads as a snap however long the fade lasts —
/// the first finding of the 0.4.0 feel review. The body grows into its own
/// height, so mid-flight it stands shorter than it finally will.
#[gpui::test]
fn opening_a_tool_call_grows_the_body_into_its_height(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let group_events = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let events = events.clone();
        let group_events = group_events.clone();
        move |_, _| InteractionProbe {
            approval: ToolApproval::NotRequired,
            open: Some(true),
            group_open: Some(true),
            events,
            group_events,
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Settle open once so the body's natural height is measured.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let settled = cx
        .debug_bounds("tool-call-body-send-1")
        .expect("an open card shows its body");

    // Close, settle, then reopen and look one frame in.
    cx.update(|_, cx| {
        view.update(cx, |probe, cx| {
            probe.open = Some(false);
            cx.notify();
        })
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(1));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.update(|_, cx| {
        view.update(cx, |probe, cx| {
            probe.open = Some(true);
            cx.notify();
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(40));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let opening = cx
        .debug_bounds("tool-call-disclosure-clip-send-1")
        .expect("the opening body is clipped to a fraction of its height");
    assert!(
        opening.size.height < settled.size.height,
        "mid-flight the body must stand shorter than its settled height:          {opening:?} vs {settled:?}"
    );
    assert!(
        opening.size.height > gpui::Pixels::ZERO,
        "and taller than nothing: {opening:?}"
    );
}

#[gpui::test]
fn pointer_toggle_emits_the_proposed_state_and_reveals_the_body(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let group_events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        let group_events = group_events.clone();
        move |_, _| InteractionProbe {
            approval: ToolApproval::NotRequired,
            open: None,
            group_open: Some(true),
            events,
            group_events,
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("tool-call-body-send-1").is_none());
    click_center(cx, "tool-call-toggle-send-1");
    assert_eq!(
        captured.borrow().as_slice(),
        &[ToolCallEvent::Toggled {
            id: "send-1".into(),
            open: true
        }]
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("tool-call-body-send-1").is_some());
}

#[gpui::test]
fn keyboard_toggle_on_the_group_collapses_its_calls(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let group_events = Rc::new(RefCell::new(Vec::new()));
    let captured_group = group_events.clone();
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        let group_events = group_events.clone();
        move |_, _| InteractionProbe {
            approval: ToolApproval::NotRequired,
            open: None,
            group_open: Some(true),
            events,
            group_events,
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("tool-call-toggle-send-1").is_some());

    // The group toggle is the first focusable control in the tree.
    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    assert_eq!(
        captured_group.borrow().as_slice(),
        &[ToolGroupEvent::Toggled {
            id: "burst".into(),
            open: false
        }]
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Hidden descendants leave the interaction tree on the closing frame.
    assert!(cx.debug_bounds("tool-call-toggle-send-1").is_none());
}

#[gpui::test]
fn constrained_card_keeps_its_output_reachable(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    // Geometry, not motion, is under test: settle the one-shot reveal.
    cx.update(|cx| cx.set_reduce_motion(true));
    struct Constrained;
    impl Render for Constrained {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let call = Progressive::complete(
                ToolInvocation::new("wide-1", "read_file")
                    .summary("a very long summary that should truncate rather than overflow the card frame")
                    .input("{ \"path\": \"pricing.md\" }")
                    .output("Done."),
            );
            gpui::div()
                .w(px(220.))
                .h(px(400.))
                .child(ToolCall::new(&call).open(true))
        }
    }
    let (_, cx) = cx.add_window_view(|_, _| Constrained);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let card = cx
        .debug_bounds("tool-call-card-wide-1")
        .expect("card should render");
    let body = cx
        .debug_bounds("tool-call-body-wide-1")
        .expect("body should render");
    assert!(card.size.width <= px(220.), "card must not exceed its host");
    assert!(body.right() <= card.right() + px(1.));
    assert!(body.bottom() <= card.bottom() + px(1.));
}

/// The failure glyph rides a first-line slot: however far the reason
/// wraps, the triangle stays centered on the first text line instead of
/// floating against the block. The slot's own geometry is the proof — its
/// box is exactly one line tall and shares the row's top edge with the
/// text, so its center and the first line's center coincide by
/// construction.
#[gpui::test]
fn the_failure_glyph_holds_to_the_first_line_of_a_wrapping_reason(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    struct WrapProbe;
    impl Render for WrapProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            // Narrow enough that the long reason must wrap.
            gpui::div().w(gpui::px(260.)).h(gpui::px(300.)).child(ToolCall::new(
                &Progressive::failed(
                    invocation(),
                    "Connection timed out after 2s while waiting for the prices replica;                      the pool retried twice before giving up on the read",
                ),
            ))
        }
    }
    let (_, cx) = cx.add_window_view(|_, _| WrapProbe);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let slot = cx
        .debug_bounds("tool-call-failure-glyph")
        .expect("the failure glyph slot should render");
    let reason = cx
        .debug_bounds("tool-call-failure-reason")
        .expect("the failure reason should render");
    let expected = cx.update(|_, cx| gpui_ai::sizing::SizeTokens::read(cx).slot_md());
    assert_eq!(
        slot.size.height, expected,
        "the slot is exactly one text line tall"
    );
    assert_eq!(
        slot.top(),
        reason.top(),
        "items_start keeps the slot on the first line"
    );
    assert!(
        reason.size.height > expected,
        "the probe must actually wrap ({:?} tall)",
        reason.size.height
    );
}
