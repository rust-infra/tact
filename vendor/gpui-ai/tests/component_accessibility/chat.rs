//! Chat's retry affordance and its composer keyboard path.
//!
//! The transcript is seeded with one failed, retryable message so both halves
//! of the contract are reachable from the same probe: the retry control on the
//! message, and the prompt bar Chat forwards submissions from.

use gpui::{
    AppContext as _, Context, Entity, Modifiers, Render, Subscription, TestAppContext,
    VisualTestContext, Window,
};
use gpui_ai::prelude::{Chat, ChatEvent, ChatMessage, ChatRole};
#[cfg(not(target_os = "macos"))]
use gpui_ai::prompt_bar::PromptBarEvent;
use gpui_ai::{prompt_bar::PromptBar, stream::Progressive};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct PublicChatProbe {
    chat: Entity<Chat>,
    events: Rc<RefCell<Vec<ChatEvent>>>,
    _subscription: Subscription,
}

impl PublicChatProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| PromptBar::new("public-chat-prompt", window, cx));
        let chat = cx.new(|cx| Chat::new("public-chat", prompt, window, cx));
        chat.update(cx, |chat, cx| {
            chat.set_messages(
                Arc::from([ChatMessage::new(
                    "failed-answer",
                    ChatRole::Assistant,
                    Progressive::failed("Partial answer".to_owned(), "Network unavailable"),
                )
                .retryable(true)]),
                window,
                cx,
            );
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let subscription = cx.subscribe(&chat, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            chat,
            events,
            _subscription: subscription,
        }
    }
}

impl Render for PublicChatProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.chat.clone()
    }
}

#[gpui::test]
fn public_chat_keeps_typed_retry_and_keyboard_composer_paths(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicChatProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("chat-transcript").is_some());
    assert!(cx.debug_bounds("chat-message-failed-answer").is_some());
    assert!(cx.debug_bounds("chat-retry-failed-answer").is_some());
    let retry = cx
        .debug_bounds("chat-retry-failed-answer")
        .expect("retry action should remain reachable");
    cx.simulate_click(retry.center(), Modifiers::default());

    #[cfg(not(target_os = "macos"))]
    {
        let chat = probe.read_with(cx, |probe, _| probe.chat.clone());
        let prompt = chat.read_with(cx, |chat, _| chat.prompt_bar().clone());
        cx.update(|window, cx| {
            prompt.update(cx, |prompt, cx| {
                prompt.set_draft("Continue from chat", window, cx);
                prompt.focus(window, cx);
            });
        });
        cx.simulate_keystrokes("enter");
    }

    probe.read_with(cx, |probe, _| {
        let events = probe.events.borrow();
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::RetryRequested { message_id } if message_id == "failed-answer"
        )));
        #[cfg(not(target_os = "macos"))]
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::Prompt(PromptBarEvent::Submit { submission, .. })
                if submission.text() == "Continue from chat"
        )));
    });
}
