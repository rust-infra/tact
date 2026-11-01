//! Attachment previews: accessible names, typed remove/open events, keyboard
//! reach, bounded width, and read-only tiles inside chat messages.

use gpui::{
    AppContext as _, Context, Element as _, Entity, IntoElement as _, KeyDownEvent, KeyUpEvent,
    Keystroke, Modifiers, ParentElement as _, Render, RenderOnce as _, Role, Styled as _,
    Subscription, TestAppContext, VisualTestContext, Window, accesskit, canvas, div, px, size,
};
use gpui_ai::{
    attachment::{Attachment, AttachmentEvent, AttachmentStrip},
    chat::{Chat, ChatEvent, ChatMessage, ChatRole},
    prompt_bar::PromptBar,
    stream::StreamedContent,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

fn pricing() -> Attachment {
    Attachment::new("pricing", "pricing.md")
        .size_bytes(12_300)
        .detail("12 pages")
}

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

/// Captures the strip's own accessibility node. Tiles render through
/// `AnyElement`, which does not surface `a11y_role` to this probe, so tile
/// semantics are proven by behavior below (a remove button reachable by
/// keyboard, a read-only tile that activates like a button) and the label
/// text by `Attachment::accessibility_label`.
struct A11yProbe {
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for A11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                let element = AttachmentStrip::new("strip")
                    .label("Prompt attachments")
                    .items([pricing()])
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

fn capture_strip(cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let (_, cx) = cx.add_window_view({
        let captured = captured.clone();
        move |_, _| A11yProbe { captured }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    captured
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("probe should capture its element")
}

#[gpui::test]
fn the_strip_is_a_named_group_and_tiles_read_name_plus_summary(cx: &mut TestAppContext) {
    let strip = capture_strip(cx);
    assert_eq!(strip.role, Some(Role::Group));
    assert_eq!(strip.node.label(), Some("Prompt attachments"));
    assert_eq!(
        pricing().accessibility_label(),
        "pricing.md, Document · 12 KB · 12 pages"
    );
}

#[derive(Clone, Copy)]
enum StripKind {
    Composer,
    Message,
    Narrow,
}

struct StripProbe {
    kind: StripKind,
    events: Rc<RefCell<Vec<AttachmentEvent>>>,
}

impl Render for StripProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        let handler = move |event: &AttachmentEvent, _: &mut Window, _: &mut gpui::App| {
            events.borrow_mut().push(event.clone());
        };
        let _ = cx;
        match self.kind {
            StripKind::Composer => div().size_full().child(
                AttachmentStrip::new("composer")
                    .items([pricing(), Attachment::new("sales", "sales.csv")])
                    .removable(true)
                    .compact(true)
                    .on_event(handler),
            ),
            StripKind::Message => div().size_full().child(
                AttachmentStrip::new("message")
                    .items([pricing()])
                    .on_event(handler),
            ),
            StripKind::Narrow => div().size_full().child(
                div().w(px(220.)).child(
                    AttachmentStrip::new("narrow").items([Attachment::new(
                        "long",
                        "a-very-long-file-name-that-keeps-going-and-going-for-quite-a-while.md",
                    )
                    .size_bytes(1_200)]),
                ),
            ),
        }
    }
}

fn strip_harness(
    kind: StripKind,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<AttachmentEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| StripProbe { kind, events }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(640.), px(320.)));
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
}

#[gpui::test]
fn remove_controls_report_the_attachment_id(cx: &mut TestAppContext) {
    let (events, cx) = strip_harness(StripKind::Composer, cx);
    click_center(cx, "attachment-remove-sales");
    assert_eq!(
        events.borrow().as_slice(),
        &[AttachmentEvent::Removed { id: "sales".into() }]
    );
}

#[gpui::test]
fn read_only_tiles_open_on_click(cx: &mut TestAppContext) {
    let (events, cx) = strip_harness(StripKind::Message, cx);
    click_center(cx, "attachment-pricing");
    assert_eq!(
        events.borrow().as_slice(),
        &[AttachmentEvent::Opened {
            id: "pricing".into()
        }]
    );
}

#[gpui::test]
fn keyboard_reaches_the_remove_control(cx: &mut TestAppContext) {
    let (events, cx) = strip_harness(StripKind::Composer, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    activate_key(cx, "enter");
    assert_eq!(
        events.borrow().as_slice(),
        &[AttachmentEvent::Removed {
            id: "pricing".into()
        }]
    );
}

#[gpui::test]
fn long_names_stay_inside_a_narrow_host(cx: &mut TestAppContext) {
    let (_, cx) = strip_harness(StripKind::Narrow, cx);
    let tile = cx
        .debug_bounds("attachment-long")
        .expect("tile should render");
    assert!(
        tile.size.width <= px(220.),
        "tile width {:?} should not exceed its 220px host",
        tile.size.width
    );
    assert!(
        tile.size.width > px(120.),
        "tile should still use the width it has"
    );
}

struct ChatProbe {
    chat: Entity<Chat>,
    _subscription: Subscription,
}

impl Render for ChatProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div().size_full().child(self.chat.clone())
    }
}

#[gpui::test]
fn chat_messages_render_attachments_and_report_activation(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events: Rc<RefCell<Vec<ChatEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let events = events.clone();
        move |window, cx| {
            let prompt = cx.new(|cx| PromptBar::new("probe-prompt", window, cx));
            let chat = cx.new(|cx| Chat::new("probe", prompt, window, cx));
            let subscription = cx.subscribe(&chat, move |_, _, event: &ChatEvent, _| {
                events.borrow_mut().push(event.clone())
            });
            ChatProbe {
                chat,
                _subscription: subscription,
            }
        }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(640.), px(520.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let messages: Arc<[ChatMessage]> = Arc::from([
        ChatMessage::new(
            "q-1",
            ChatRole::User,
            StreamedContent::done("Compare these suppliers."),
        )
        .attachments([pricing()]),
        ChatMessage::new(
            "a-1",
            ChatRole::Assistant,
            StreamedContent::done("Alpenrose is cheapest."),
        ),
    ]);
    view.update_in(cx, |probe, window, cx| {
        probe
            .chat
            .update(cx, |chat, cx| chat.set_messages(messages, window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        view.read_with(cx, |probe, cx| probe.chat.read(cx).messages()[0]
            .attachment_refs()
            .len()),
        1
    );
    click_center(cx, "attachment-pricing");
    assert!(
        events.borrow().iter().any(|event| matches!(
            event,
            ChatEvent::AttachmentActivated { message_id, attachment_id }
                if message_id.as_ref() == "q-1" && attachment_id.as_ref() == "pricing"
        )),
        "expected AttachmentActivated, got {:?}",
        events.borrow()
    );
}
