use gpui::{
    AppContext as _, Context, Entity, Modifiers, ParentElement as _, Render, Styled as _,
    Subscription, TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_ai::{
    chat::{Chat, ChatEvent, ChatMessage, ChatRole, ChatWelcome, MessageActions},
    prompt_bar::PromptBar,
    stream::StreamedContent,
    suggestions::Suggestion,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct Probe {
    chat: Entity<Chat>,
    _subscription: Subscription,
}

impl Probe {
    fn new(
        events: Rc<RefCell<Vec<ChatEvent>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let prompt = cx.new(|cx| PromptBar::new("probe-prompt", window, cx));
        let chat = cx.new(|cx| {
            let mut chat = Chat::new("probe", prompt, window, cx);
            chat.set_welcome(
                Some(
                    ChatWelcome::new("What should we look into?")
                        .description("Pick a starter or write your own.")
                        .suggestions([
                            Suggestion::new("compare", "Try again"),
                            Suggestion::new("risk", "Try again"),
                        ]),
                ),
                cx,
            );
            chat
        });
        let subscription = cx.subscribe(&chat, {
            let events = events.clone();
            move |_, _, event: &ChatEvent, _| events.borrow_mut().push(event.clone())
        });
        Self {
            chat,
            _subscription: subscription,
        }
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div().size_full().child(self.chat.clone())
    }
}

fn harness(
    cx: &mut TestAppContext,
) -> (
    Entity<Probe>,
    Rc<RefCell<Vec<ChatEvent>>>,
    &mut VisualTestContext,
) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let events = events.clone();
        move |window, cx| Probe::new(events, window, cx)
    });
    cx.simulate_resize(size(px(640.), px(520.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (view, events, cx)
}

fn settled_conversation() -> Arc<[ChatMessage]> {
    Arc::from([
        ChatMessage::new(
            "q-1",
            ChatRole::User,
            StreamedContent::done("What is the answer?"),
        ),
        ChatMessage::new(
            "a-1",
            ChatRole::Assistant,
            StreamedContent::done("Forty-two"),
        ),
    ])
}

fn set_messages(view: &Entity<Probe>, messages: Arc<[ChatMessage]>, cx: &mut VisualTestContext) {
    view.update_in(cx, |probe, window, cx| {
        probe
            .chat
            .update(cx, |chat, cx| chat.set_messages(messages, window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} should render"));
    cx.simulate_click(bounds.center(), Modifiers::default());
}

#[test]
fn role_defaults_match_the_conventional_action_sets() {
    assert!(!MessageActions::for_role(ChatRole::Assistant).is_empty());
    assert!(!MessageActions::for_role(ChatRole::User).is_empty());
    assert!(!MessageActions::for_role(ChatRole::Tool).is_empty());
    assert!(MessageActions::for_role(ChatRole::System).is_empty());
    assert!(MessageActions::none().is_empty());
    assert!(!MessageActions::none().copy(true).is_empty());
    let message = ChatMessage::new("s", ChatRole::Assistant, StreamedContent::done("x"))
        .actions(MessageActions::none());
    assert!(message.message_actions().is_empty());
}

#[gpui::test]
fn empty_conversation_shows_the_welcome_and_suggestions_emit_stable_ids(cx: &mut TestAppContext) {
    let (_, events, cx) = harness(cx);
    assert!(cx.debug_bounds("chat-welcome").is_some());
    click(cx, "suggestion-compare");
    click(cx, "suggestion-risk");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let keystroke = gpui::Keystroke::parse("enter").expect("test key");
    cx.simulate_event(gpui::KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(gpui::KeyUpEvent { keystroke });
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ChatEvent::SuggestionSelected {
                suggestion_id: "compare".into()
            },
            ChatEvent::SuggestionSelected {
                suggestion_id: "risk".into()
            },
            ChatEvent::SuggestionSelected {
                suggestion_id: "risk".into()
            },
        ]
    );
}

#[gpui::test]
fn copy_action_writes_the_clipboard_and_reports_the_message(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx);
    set_messages(&view, settled_conversation(), cx);
    assert!(cx.debug_bounds("chat-welcome").is_none());

    click(cx, "chat-action-copy-a-1");
    assert_eq!(
        events.borrow().as_slice(),
        &[ChatEvent::MessageCopied {
            message_id: "a-1".into()
        }]
    );
    let copied = cx
        .update(|_, cx| cx.read_from_clipboard())
        .and_then(|item| item.text());
    assert_eq!(copied.as_deref(), Some("Forty-two"));
}

#[gpui::test]
fn regenerate_and_feedback_report_typed_intent_by_stable_id(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx);
    set_messages(&view, settled_conversation(), cx);

    click(cx, "chat-action-regenerate-a-1");
    click(cx, "chat-action-helpful-a-1");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ChatEvent::RegenerateRequested {
                message_id: "a-1".into()
            },
            ChatEvent::FeedbackSubmitted {
                message_id: "a-1".into(),
                positive: true
            },
        ]
    );
    // The edit action opens the in-place editor; `tests/chat_branching.rs`
    // covers it, since focusing the editor needs a platform window.
    assert!(cx.debug_bounds("chat-action-edit-q-1").is_some());
    // User prompts never offer regeneration or ratings; assistant replies never offer editing.
    assert!(cx.debug_bounds("chat-action-regenerate-q-1").is_none());
    assert!(cx.debug_bounds("chat-action-helpful-q-1").is_none());
    assert!(cx.debug_bounds("chat-action-edit-a-1").is_none());
}

#[gpui::test]
fn actions_wait_until_a_message_settles(cx: &mut TestAppContext) {
    let (view, _, cx) = harness(cx);
    set_messages(
        &view,
        Arc::from([ChatMessage::new(
            "a-1",
            ChatRole::Assistant,
            StreamedContent::running("Thinking about".to_owned()),
        )]),
        cx,
    );
    assert!(cx.debug_bounds("chat-actions-a-1").is_none());

    set_messages(
        &view,
        Arc::from([ChatMessage::new(
            "a-1",
            ChatRole::Assistant,
            StreamedContent::done("Thinking about it: forty-two."),
        )]),
        cx,
    );
    assert!(cx.debug_bounds("chat-actions-a-1").is_some());
}
