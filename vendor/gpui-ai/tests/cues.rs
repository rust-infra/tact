//! Interaction cues fire at the moments components already report, never
//! on initial load, and stop when the subscription is dropped.

use gpui::{
    AppContext as _, Context, Entity, Modifiers, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_ai::{
    chat::{Chat, ChatMessage, ChatRole, MessageActions},
    cues::{self, Cue, CueSubscription},
    prompt_bar::PromptBar,
    stream::StreamedContent,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct Probe {
    chat: Entity<Chat>,
}

impl Probe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| PromptBar::new("cue-prompt", window, cx));
        let chat = cx.new(|cx| Chat::new("cue-chat", prompt, window, cx));
        Self { chat }
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div().size_full().child(self.chat.clone())
    }
}

struct Harness<'a> {
    view: Entity<Probe>,
    received: Rc<RefCell<Vec<Cue>>>,
    subscription: Option<CueSubscription>,
    cx: &'a mut VisualTestContext,
}

fn harness(cx: &mut TestAppContext) -> Harness<'_> {
    cx.update(gpui_ai::init);
    let received = Rc::new(RefCell::new(Vec::new()));
    let subscription = cx.update({
        let received = received.clone();
        move |cx| cues::observe(cx, move |cue, _| received.borrow_mut().push(cue.clone()))
    });
    let (view, cx) = cx.add_window_view(Probe::new);
    cx.simulate_resize(size(px(640.), px(520.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    Harness {
        view,
        received,
        subscription: Some(subscription),
        cx,
    }
}

impl Harness<'_> {
    fn set_messages(&mut self, messages: Arc<[ChatMessage]>) {
        let view = self.view.clone();
        view.update_in(self.cx, |probe, window, cx| {
            probe
                .chat
                .update(cx, |chat, cx| chat.set_messages(messages, window, cx));
        });
        self.cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    fn take(&self) -> Vec<Cue> {
        std::mem::take(&mut *self.received.borrow_mut())
    }
}

fn history() -> Vec<ChatMessage> {
    vec![
        ChatMessage::new("q-1", ChatRole::User, StreamedContent::done("Hello")),
        ChatMessage::new(
            "a-1",
            ChatRole::Assistant,
            StreamedContent::done("Hi there."),
        )
        .actions(MessageActions::for_role(ChatRole::Assistant)),
    ]
}

fn with_reply(state: StreamedContent) -> Arc<[ChatMessage]> {
    let mut messages = history();
    messages.push(ChatMessage::new("a-2", ChatRole::Assistant, state));
    Arc::from(messages)
}

#[gpui::test]
fn loaded_history_is_silent_and_appended_replies_cue_their_lifecycle(cx: &mut TestAppContext) {
    let mut harness = harness(cx);

    harness.set_messages(Arc::from(history()));
    assert!(
        harness.take().is_empty(),
        "loading a transcript must not cue every historical message"
    );

    harness.set_messages(with_reply(StreamedContent::running("Think".to_owned())));
    assert_eq!(
        harness.take(),
        vec![Cue::MessageArrived {
            message_id: "a-2".into()
        }]
    );

    harness.set_messages(with_reply(StreamedContent::running("Thinking".to_owned())));
    assert!(
        harness.take().is_empty(),
        "a content-only change while streaming is not a cue"
    );

    harness.set_messages(with_reply(StreamedContent::done("Thinking done.")));
    assert_eq!(
        harness.take(),
        vec![Cue::ResponseSettled {
            message_id: "a-2".into(),
            succeeded: true,
        }]
    );
}

#[gpui::test]
fn failed_replies_settle_unsuccessfully(cx: &mut TestAppContext) {
    let mut harness = harness(cx);
    harness.set_messages(Arc::from(history()));
    harness.set_messages(with_reply(StreamedContent::running("Partial".to_owned())));
    harness.take();

    harness.set_messages(with_reply(StreamedContent::failed(
        "Partial".to_owned(),
        "Provider timed out",
    )));
    assert_eq!(
        harness.take(),
        vec![Cue::ResponseSettled {
            message_id: "a-2".into(),
            succeeded: false,
        }]
    );
}

#[gpui::test]
fn copying_a_message_cues_and_the_arrival_reveal_keeps_the_row_interactive(
    cx: &mut TestAppContext,
) {
    let mut harness = harness(cx);
    harness.set_messages(Arc::from(history()));
    // Append a settled reply so it both reveals (arrival) and offers copy.
    let mut messages = history();
    messages.push(
        ChatMessage::new(
            "a-2",
            ChatRole::Assistant,
            StreamedContent::done("Copy me."),
        )
        .actions(MessageActions::for_role(ChatRole::Assistant)),
    );
    harness.set_messages(Arc::from(messages));
    harness.take();

    let bounds = harness
        .cx
        .debug_bounds("chat-action-copy-a-2")
        .expect("copy action renders for the newly arrived reply");
    harness
        .cx
        .simulate_click(bounds.center(), Modifiers::default());
    harness.cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(harness.take(), vec![Cue::Copied]);
    assert_eq!(
        harness
            .cx
            .update(|_, cx| cx.read_from_clipboard())
            .and_then(|item| item.text()),
        Some("Copy me.".to_owned())
    );
}

#[gpui::test]
fn dropping_the_subscription_stops_delivery(cx: &mut TestAppContext) {
    let mut harness = harness(cx);
    harness.set_messages(Arc::from(history()));
    harness.subscription.take();

    harness.set_messages(with_reply(StreamedContent::running("Late".to_owned())));
    assert!(harness.take().is_empty());
}

#[gpui::test]
fn dropping_a_subscription_retires_only_its_own_apps_observer(
    a: &mut TestAppContext,
    b: &mut TestAppContext,
) {
    fn observe_into(cx: &mut TestAppContext, received: &Rc<RefCell<Vec<Cue>>>) -> CueSubscription {
        let received = received.clone();
        cx.update(move |cx| {
            cues::observe(cx, move |cue, _| received.borrow_mut().push(cue.clone()))
        })
    }

    let in_a = Rc::new(RefCell::new(Vec::new()));
    let in_b = Rc::new(RefCell::new(Vec::new()));
    let subscription_a = observe_into(a, &in_a);
    let _subscription_b = observe_into(b, &in_b);

    drop(subscription_a);

    b.update(|cx| cues::emit(cx, Cue::Copied));
    assert_eq!(
        in_b.borrow().as_slice(),
        &[Cue::Copied],
        "retiring an observer in one app must leave another app's observer registered"
    );

    a.update(|cx| cues::emit(cx, Cue::Submitted));
    assert!(
        in_a.borrow().is_empty(),
        "a retired observer must stop receiving cues even after another app emitted first"
    );
}
