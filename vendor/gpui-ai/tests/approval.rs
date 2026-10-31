//! Approval gates: tones, "Always allow", and resolved states.

use gpui::{
    Context, Modifiers, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
    Window, div, px, size,
};
use gpui_ai::approval::{ApprovalCard, ApprovalDecision, ApprovalEvent, ApprovalTone};
use gpui_component::ActiveTheme as _;
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy)]
enum ProbeKind {
    Pending,
    Approved,
    Rejected,
}

struct Probe {
    kind: ProbeKind,
    events: Rc<RefCell<Vec<ApprovalEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        let handler = move |event: &ApprovalEvent, _: &mut Window, _: &mut gpui::App| {
            events.borrow_mut().push(event.clone());
        };
        let card = ApprovalCard::new("purge", "Delete 12 stale records?")
            .description("Removed permanently.")
            .tone(ApprovalTone::Destructive)
            .allow_always(true)
            .note("Decided by Oscar");
        let card = match self.kind {
            ProbeKind::Pending => card,
            ProbeKind::Approved => card.decision(ApprovalDecision::Approved),
            ProbeKind::Rejected => card.decision(ApprovalDecision::Rejected),
        };
        div().size_full().child(card.on_event(handler))
    }
}

fn harness(
    kind: ProbeKind,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<ApprovalEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| Probe { kind, events }
    });
    cx.simulate_resize(size(px(640.), px(400.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (events, cx)
}

/// A control centres its label's cap height over its baseline.
///
/// The rule every desktop toolkit uses, and the one this got wrong twice.
///
/// GPUI centres a line's ascent-to-descent box. A font's ascent reserves room
/// for accents that most words never use, so the first attempt centred the
/// band from the cap height to the descent instead - and the descent is room
/// kept for descenders, which `Allow`, `Accept`, `Export` and `Continue` do
/// not have. Measured, that lifted every one of them four pixels too high,
/// while `Approve` and `Reject` came out level, which is why a test written
/// against those two passed it.
///
/// So this measures both edges of the band the eye actually reads: the top of
/// the caps and the baseline. Neither depends on which word is in the button -
/// the baseline is where the line box puts it - which is what makes the check
/// hold for `Allow` as much as for the two labels it can reach.
#[gpui::test]
fn a_control_centres_its_label_on_the_baseline(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Pending, cx);
    let (ascent, descent, cap, leading) = cx.update(|window, cx| {
        let body = cx.theme().typography_tokens().sm;
        let text = cx.text_system();
        let font = text.resolve_font(&window.text_style().font());
        (
            text.ascent(font, body.size),
            // A depth rather than an offset: GPUI hands back the font file's
            // own sign, which is negative below the baseline.
            -text.descent(font, body.size),
            text.cap_height(font, body.size),
            body.line_height,
        )
    });
    for (button, label) in [
        ("approval-approve-purge", "button-label-Approve"),
        ("approval-reject-purge", "button-label-Reject"),
    ] {
        let button = cx
            .debug_bounds(button)
            .unwrap_or_else(|| panic!("{button} should render"));
        let label = cx
            .debug_bounds(label)
            .unwrap_or_else(|| panic!("{label} should render"));
        // Where GPUI puts the baseline inside the line box, which is centred
        // in the control because nothing offsets it.
        let baseline = label.top() + (leading - ascent - descent) / 2.0 + ascent;
        let above = baseline - cap - button.top();
        let below = button.bottom() - baseline;
        assert!(
            (above - below).abs() <= px(1.0),
            "a control must centre its label's caps over its baseline: {above:?} above the \
             caps, {below:?} below the baseline"
        );
        // A second, different failure: caps level in a control too short to
        // hold the type it carries. One measurement covers both edges, because
        // a centred line box leaves the same room above the ascent as below
        // the descent.
        let clearance = baseline - ascent - button.top();
        assert!(
            clearance >= px(2.0),
            "a control must leave its accents and descenders clear of its frame, not \
             {clearance:?}"
        );
    }
}

fn click_center(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} should render"));
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn pending_gates_offer_every_decision_including_always_allow(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Pending, cx);
    assert!(cx.debug_bounds("approval-decision-purge").is_none());
    click_center(cx, "approval-approve-purge");
    click_center(cx, "approval-always-purge");
    click_center(cx, "approval-reject-purge");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ApprovalEvent::Approved { id: "purge".into() },
            ApprovalEvent::ApprovedAlways { id: "purge".into() },
            ApprovalEvent::Rejected { id: "purge".into() },
        ]
    );
}

#[gpui::test]
fn resolved_gates_show_the_decision_instead_of_buttons(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Approved, cx);
    assert!(cx.debug_bounds("approval-decision-purge").is_some());
    assert!(cx.debug_bounds("approval-approve-purge").is_none());
    assert!(cx.debug_bounds("approval-always-purge").is_none());
    assert!(cx.debug_bounds("approval-reject-purge").is_none());

    let (_, cx) = harness(ProbeKind::Rejected, cx);
    assert!(cx.debug_bounds("approval-decision-purge").is_some());
    assert!(cx.debug_bounds("approval-approve-purge").is_none());
}

#[test]
fn decision_labels_read_as_badges() {
    assert_eq!(ApprovalDecision::Pending.label(), "Pending");
    assert_eq!(ApprovalDecision::Approved.label(), "Approved");
    assert_eq!(ApprovalDecision::Rejected.label(), "Rejected");
}
