//! Chat behavior, accessibility, and virtualization regression coverage.
#![cfg(test)]

use super::*;
use super::{render::*, transcript::*};
#[cfg(not(target_os = "macos"))]
use crate::prompt_bar::PromptBarEvent;
use crate::{
    prompt_bar::PromptBar,
    stream::Progressive,
    streaming_text::{CitationRef, FollowUp},
};
use gpui::{
    Context, Element as _, Entity, KeyDownEvent, KeyUpEvent, Keystroke, ListOffset, Modifiers,
    Render, RenderOnce as _, Role, SharedString, Subscription, TestAppContext, VisualTestContext,
    Window, accesskit, canvas, px, size,
};
use gpui_component::theme::Theme;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

struct ChatHarness {
    chat: Entity<Chat>,
    events: Rc<RefCell<Vec<ChatEvent>>>,
    _subscription: Subscription,
}

impl ChatHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let prompt = cx.new(|cx| PromptBar::new("chat-prompt", window, cx));
        let chat = cx.new(|cx| Chat::new("test-chat", prompt, window, cx));
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

impl Render for ChatHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.chat.clone()
    }
}

fn message(index: usize) -> ChatMessage {
    ChatMessage::new(
        format!("m{index:04}"),
        if index.is_multiple_of(2) {
            ChatRole::Assistant
        } else {
            ChatRole::User
        },
        Progressive::complete(format!("Message {index}: selectable conversation prose.")),
    )
}

fn messages(range: std::ops::Range<usize>) -> Arc<[ChatMessage]> {
    Arc::from(range.map(message).collect::<Vec<_>>())
}

fn harness(cx: &mut TestAppContext) -> (Entity<ChatHarness>, &mut VisualTestContext) {
    cx.update(crate::init);
    let (harness, cx) = cx.add_window_view(ChatHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(420.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (harness, cx)
}

fn set_messages(
    harness: &Entity<ChatHarness>,
    messages: Arc<[ChatMessage]>,
    cx: &mut VisualTestContext,
) {
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    cx.update(|window, cx| {
        chat.update(cx, |chat, cx| chat.set_messages(messages, window, cx));
        window.draw(cx).clear(cx);
    });
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

fn top_anchor(chat: &Chat) -> Option<(SharedString, gpui::Pixels)> {
    let offset = chat.list_state.logical_scroll_top();
    chat.messages
        .get(offset.item_ix)
        .map(|message| (message.id.clone(), offset.offset_in_item))
}

/// Zooms the way the shell does: the theme carries the base type size and
/// `Root` hands it to the window every frame.
///
/// Two draws, because Chat notices the new rem while rendering and reacts
/// afterwards — the first draw is where it sees the change, the second
/// lays out what it re-measured. Nothing here calls `remeasure`; that Chat
/// does it unprompted is the property under test.
fn zoom_to(cx: &mut VisualTestContext, font_size: f32) {
    cx.update(|window, cx| {
        Theme::global_mut(cx).font_size = px(font_size);
        window.set_rem_size(Theme::global(cx).font_size);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[derive(Clone, Copy)]
enum ChatControlKind {
    Retry,
    Jump,
}

struct CapturedControlA11y {
    role: Option<Role>,
    node: accesskit::Node,
}

struct ChatControlA11yProbe {
    kind: ChatControlKind,
    captured: Arc<Mutex<Option<CapturedControlA11y>>>,
}

impl Render for ChatControlA11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let kind = self.kind;
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let button = match kind {
                    ChatControlKind::Retry => {
                        retry_button(&"chat".into(), &"retry-me".into(), window, cx)
                    }
                    ChatControlKind::Jump => jump_to_latest_button(
                        &"chat".into(),
                        "Jump to latest, 2 unread messages".into(),
                        window,
                        cx,
                    ),
                };
                let element = button
                    .on_click(|_: &gpui::ClickEvent, _, _| {})
                    .render(window, cx)
                    .into_element();
                let role = element.a11y_role();
                let mut node = accesskit::Node::new(Role::Unknown);
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedControlA11y { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture_chat_control(kind: ChatControlKind, cx: &mut TestAppContext) -> CapturedControlA11y {
    cx.update(crate::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ChatControlA11yProbe { kind, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("chat control should be captured")
}

#[test]
fn controlled_snapshots_require_unique_stable_message_ids() {
    let valid = [message(1), message(2)];
    assert!(message_ids_are_unique(&valid));

    let duplicate = [message(1), message(1)];
    assert!(!message_ids_are_unique(&duplicate));
}

/// Rides the animated jump-to-latest drive to its end, painting between
/// ticks the way a live window does — the drive holds, by design, while a
/// step it took has not painted yet.
fn settle_jump(cx: &mut VisualTestContext) {
    for _ in 0..200 {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(8));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
}

#[gpui::test]
fn a_far_jump_starts_near_the_tail_and_settles_into_it(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    set_messages(&harness, messages(0..62), cx);

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // The drive is distance-capped: one frame after the jump the last
    // message's top is in view — legible context, never a teleport — and
    // the drive is still settling the bottom alignment.
    assert!(
        !chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()),
        "a far jump must settle into the tail, not land pinned in one frame"
    );
    assert!(
        chat.read_with(cx, |chat, _| {
            let remaining = chat.list_state.max_offset_for_scrollbar().y
                + chat.list_state.scroll_px_offset_for_scrollbar().y;
            remaining > chat.list_state.viewport_bounds().size.height * 0.5
        }),
        "the first painted frame must actually precede the tail, not just retain a drive flag"
    );
    settle_jump(cx);
    assert!(cx.debug_bounds("chat-message-m0061").is_some());
    assert!(chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()));
}

#[gpui::test]
fn changing_to_reduced_motion_finishes_an_in_flight_jump(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    chat.update(cx, |chat, cx| chat.scroll_to_latest(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(chat.read_with(cx, |chat, _| chat.jump_drive.is_some()));
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    assert!(chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()
        && chat.jump_drive.is_none()));
}

/// A reader can scroll back through a transcript that is still streaming.
///
/// A list following its tail reports its position as one past its last item,
/// and scrolling by a delta from there seeks to that sentinel and lands back
/// where it began. The wheel containment this crate puts over nested lists
/// absorbed the reader's scroll and then moved nothing, so turning the wheel
/// back during an answer did exactly nothing — and because the tail was never
/// released, every arriving chunk kept the view pinned to the end.
///
/// Measured by which message sits at the top rather than by scroll offset:
/// the offset is derived from the heights measured so far, and scrolling back
/// through a virtualized list measures more of it, so the number moves even
/// when the view does not.
#[gpui::test]
fn a_reader_can_scroll_back_while_an_answer_is_still_streaming(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    assert!(
        chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()),
        "a fresh transcript opens at its end"
    );

    let top = |cx: &mut VisualTestContext| {
        chat.read_with(cx, |chat, _| chat.list_state.logical_scroll_top().item_ix)
    };
    let center = chat.read_with(cx, |chat, _| chat.list_state.viewport_bounds().center());
    let turn_wheel = |cx: &mut VisualTestContext| {
        cx.simulate_event(gpui::ScrollWheelEvent {
            position: center,
            delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(120.))),
            modifiers: Modifiers::default(),
            touch_phase: gpui::TouchPhase::Moved,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    };

    turn_wheel(cx);
    assert!(
        !chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()),
        "turning the wheel back releases the tail"
    );
    let after_one = top(cx);
    turn_wheel(cx);
    let after_two = top(cx);
    assert!(
        after_two < after_one,
        "each notch must carry the reader further back: {after_one} then {after_two}"
    );

    // The answer keeps arriving, as it does while one is being written. New
    // messages land after the one being read, so the reader's place is the
    // same message it was before.
    let resting = after_two;
    for extra in 60..66 {
        set_messages(&harness, messages(0..extra + 1), cx);
    }

    assert!(
        !chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()),
        "content arriving must not drag the reader back to the end"
    );
    assert_eq!(
        top(cx),
        resting,
        "the reader's place must survive the rest of the answer"
    );
}

#[gpui::test]
fn a_medium_jump_keeps_its_full_distance_and_reader_input_cancels_it(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        let viewport = chat.list_state.viewport_bounds().size.height;
        chat.list_state.scroll_by(-viewport * 1.5);
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    chat.update(cx, |chat, cx| {
        let distance = chat.list_state.max_offset_for_scrollbar().y
            + chat.list_state.scroll_px_offset_for_scrollbar().y;
        chat.scroll_to_latest(cx);
        assert_eq!(chat.jump_drive.expect("medium jump").distance, distance);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let center = chat.read_with(cx, |chat, _| chat.list_state.viewport_bounds().center());
    cx.simulate_event(gpui::ScrollWheelEvent {
        position: center,
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.), px(0.5))),
        modifiers: Modifiers::default(),
        touch_phase: gpui::TouchPhase::Moved,
    });
    assert!(
        chat.read_with(cx, |chat, _| chat.jump_drive.is_none()),
        "even subpixel reader input cancels the drive"
    );
    settle_jump(cx);
    assert!(!chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()));
    cx.update(|window, cx| {
        let focus = chat.read(cx).prompt_bar.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        chat.update(cx, |chat, cx| chat.scroll_to_latest(cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(chat.read_with(cx, |chat, _| chat.jump_drive.is_some()));
    activate_key(cx, "escape");
    assert!(
        chat.read_with(cx, |chat, _| chat.jump_drive.is_none()),
        "keyboard input cancels before a descendant can consume it"
    );
}

#[gpui::test]
fn reduced_motion_jumps_without_travel(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    cx.update(|_, cx| cx.set_reduce_motion(true));
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    set_messages(&harness, messages(0..62), cx);

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("chat-message-m0061").is_some(),
        "reduced motion snaps to the tail with no drive to wait out"
    );
    assert!(chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()));
}

#[gpui::test]
fn chat_owned_controls_expose_production_role_name_and_click_action(cx: &mut TestAppContext) {
    let retry = capture_chat_control(ChatControlKind::Retry, cx);
    assert_eq!(retry.role, Some(Role::Button));
    assert_eq!(retry.node.label(), Some("Retry message"));
    assert!(retry.node.supports_action(accesskit::Action::Click));

    let jump = capture_chat_control(ChatControlKind::Jump, cx);
    assert_eq!(jump.role, Some(Role::Button));
    assert_eq!(jump.node.label(), Some("Jump to latest, 2 unread messages"));
    assert!(jump.node.supports_action(accesskit::Action::Click));
}

#[gpui::test]
fn trailing_filled_message_moves_a_bounded_bubble_surface(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(
        &harness,
        Arc::from([
            ChatMessage::new(
                "assistant",
                ChatRole::Assistant,
                Progressive::complete("A concise answer".to_owned()),
            )
            .with_appearance(ChatMessageAppearance::new(
                MessageAlignment::Leading,
                MessageBubble::Plain,
            )),
            ChatMessage::new(
                "user",
                ChatRole::User,
                Progressive::complete("A short question".to_owned()),
            )
            .with_appearance(ChatMessageAppearance::new(
                MessageAlignment::Trailing,
                MessageBubble::Filled,
            )),
        ]),
        cx,
    );
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let row = cx
        .debug_bounds("chat-message-user")
        .expect("the user message row should render");
    let bubble = cx
        .debug_bounds("chat-message-bubble-user")
        .expect("the user bubble should render");
    let left_space = bubble.left() - row.left();
    let right_space = row.right() - bubble.right();

    assert!(
        bubble.size.width < row.size.width,
        "row={row:?}, bubble={bubble:?}"
    );
    assert!(left_space > right_space, "row={row:?}, bubble={bubble:?}");
    assert!(bubble.right() <= row.right());
}

#[gpui::test]
fn prepend_preserves_the_prior_top_message_and_pixel_offset(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(10..70), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 20,
            offset_in_item: px(7.),
        });
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let before = chat.read_with(cx, |chat, _| top_anchor(chat));

    set_messages(&harness, messages(0..70), cx);

    assert_eq!(chat.read_with(cx, |chat, _| top_anchor(chat)), before);
    assert_eq!(
        chat.read_with(cx, |chat, _| chat.unread_count()),
        0,
        "older prepended history is not unread"
    );
}

#[gpui::test]
fn zooming_re_measures_the_transcript_and_keeps_following_the_tail(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    assert!(chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()));

    // 100%, 150%, 200% of the 16px base.
    for font_size in [16., 24., 32.] {
        zoom_to(cx, font_size);

        chat.read_with(cx, |chat, _| {
            assert!(
                chat.resolved_layout.matches(px(font_size)),
                "Chat must notice {font_size}px type from its own render"
            );
            assert!(
                chat.is_pinned_to_bottom(),
                "a transcript already following the tail keeps following it at \
                 {font_size}px type"
            );
        });
        assert!(
            cx.debug_bounds("chat-message-m0059").is_some(),
            "the latest message stays reachable at {font_size}px type"
        );
    }
}

#[gpui::test]
fn zooming_preserves_the_first_visible_message_when_not_following(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 12,
            offset_in_item: px(0.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let anchor = chat
        .read_with(cx, |chat, _| top_anchor(chat))
        .expect("the transcript should rest on a message")
        .0;

    for font_size in [16., 24., 32.] {
        zoom_to(cx, font_size);

        chat.read_with(cx, |chat, _| {
            assert!(chat.resolved_layout.matches(px(font_size)));
            assert_eq!(
                top_anchor(chat).map(|(id, _)| id),
                Some(anchor.clone()),
                "the message that was first on screen stays first at {font_size}px type"
            );
            assert!(
                !chat.is_pinned_to_bottom(),
                "zooming must not jump a reader who had scrolled back to the tail"
            );
        });
        assert!(
            cx.debug_bounds("chat-message-m0012").is_some(),
            "the anchored message is still drawn at {font_size}px type"
        );
    }
}

#[gpui::test]
fn append_follows_latest_only_while_pinned(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..40), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    assert!(chat.read_with(cx, |chat, _| chat.list_state.is_following_tail()));

    set_messages(&harness, messages(0..41), cx);

    assert!(chat.read_with(cx, |chat, _| chat.list_state.is_following_tail()));
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 0);
    assert!(cx.debug_bounds("chat-message-m0040").is_some());
}

#[gpui::test]
fn offscreen_append_increments_unread_without_moving_the_anchor(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 12,
            offset_in_item: px(5.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let before = chat.read_with(cx, |chat, _| top_anchor(chat));

    set_messages(&harness, messages(0..63), cx);

    assert_eq!(chat.read_with(cx, |chat, _| top_anchor(chat)), before);
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 3);
    assert!(cx.debug_bounds("chat-jump-latest").is_some());
    assert!(cx.debug_bounds("chat-message-m0062").is_none());

    let jump = cx
        .debug_bounds("chat-jump-latest")
        .expect("named jump action should remain reachable");
    cx.simulate_click(jump.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // Semantics land immediately; the travel is staged and distance-capped,
    // so the tail arrives once the drive settles.
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 0);
    settle_jump(cx);
    assert!(cx.debug_bounds("chat-message-m0062").is_some());
    harness.read_with(cx, |harness, _| {
        assert!(
            harness.events.borrow().contains(&ChatEvent::JumpedToLatest),
            "jumping should preserve the typed intent"
        );
    });
}

#[gpui::test]
fn unread_reconciles_removed_messages_and_targets_the_latest_retained_id(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 12,
            offset_in_item: px(5.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    set_messages(&harness, messages(0..63), cx);
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 3);

    set_messages(&harness, messages(0..62), cx);
    assert_eq!(
        chat.read_with(cx, |chat, _| chat.unread_count()),
        2,
        "removed unread IDs must leave the unread set"
    );
    let jump = cx
        .debug_bounds("chat-jump-latest")
        .expect("remaining unread messages should keep the jump action reachable");
    cx.simulate_click(jump.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    settle_jump(cx);
    assert!(cx.debug_bounds("chat-message-m0061").is_some());
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 0);
}

#[gpui::test]
fn clearing_the_conversation_clears_unread_state_and_jump_action(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 12,
            offset_in_item: px(5.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    set_messages(&harness, messages(0..63), cx);
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 3);

    set_messages(&harness, Arc::from([]), cx);

    assert!(chat.read_with(cx, |chat, _| chat.messages().is_empty()));
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 0);
    assert!(cx.debug_bounds("chat-jump-latest").is_none());

    set_messages(&harness, messages(100..101), cx);
    assert!(chat.read_with(cx, |chat, _| chat.is_pinned_to_bottom()));
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 0);
    assert!(cx.debug_bounds("chat-message-m0100").is_some());
}

#[gpui::test]
fn streaming_growth_and_width_change_preserve_the_logical_anchor(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let mut initial = (0..80).map(message).collect::<Vec<_>>();
    initial[24] = ChatMessage::new(
        "stream",
        ChatRole::Assistant,
        Progressive::running("A short streamed answer".to_owned()),
    );
    set_messages(&harness, Arc::from(initial.clone()), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 24,
            offset_in_item: px(6.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let before = chat.read_with(cx, |chat, _| top_anchor(chat));

    let mut grown = initial;
    let mut content = Progressive::running("A short streamed answer".to_owned());
    content
        .append(" that grows across several wrapped lines while the user reads it above the fold.");
    grown[24] = ChatMessage::new("stream", ChatRole::Assistant, content);
    set_messages(&harness, Arc::from(grown), cx);
    assert_eq!(chat.read_with(cx, |chat, _| top_anchor(chat)), before);

    cx.simulate_resize(size(px(360.), px(420.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(chat.read_with(cx, |chat, _| top_anchor(chat)), before);
}

#[gpui::test]
fn one_thousand_messages_render_only_the_visible_stable_range(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..1_000), cx);

    assert!(cx.debug_bounds("chat-message-m0999").is_some());
    assert!(cx.debug_bounds("chat-message-m0000").is_none());

    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 500,
            offset_in_item: px(0.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("chat-message-m0500").is_some());
    assert!(cx.debug_bounds("chat-message-m0000").is_none());
    assert!(cx.debug_bounds("chat-message-m0999").is_none());
}

#[gpui::test]
fn focused_virtual_message_row_survives_scroll_and_stable_replacement(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let mut snapshot = (0..999).map(message).collect::<Vec<_>>();
    snapshot.push(
        ChatMessage::new(
            "retry-me",
            ChatRole::Assistant,
            Progressive::failed("Could not finish".to_owned(), "Network unavailable"),
        )
        .retryable(true),
    );
    set_messages(&harness, Arc::from(snapshot.clone()), cx);

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    harness.read_with(cx, |harness, _| {
        assert!(harness.events.borrow().iter().any(|event| matches!(
            event,
            ChatEvent::RetryRequested { message_id } if message_id == "retry-me"
        )));
        harness.events.borrow_mut().clear();
    });

    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(3.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    snapshot.insert(0, message(2_000));
    set_messages(&harness, Arc::from(snapshot), cx);
    assert!(cx.debug_bounds("chat-message-m0000").is_some());
    assert!(cx.debug_bounds("chat-message-retry-me").is_some());

    activate_key(cx, "enter");
    harness.read_with(cx, |harness, _| {
        assert!(harness.events.borrow().iter().any(|event| matches!(
            event,
            ChatEvent::RetryRequested { message_id } if message_id == "retry-me"
        )));
    });
}

#[gpui::test]
fn message_actions_and_keyboard_composer_forward_stable_typed_events(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    let failed = ChatMessage::new(
        "retry-me",
        ChatRole::Assistant,
        Progressive::failed("Could not finish".to_owned(), "Network unavailable"),
    )
    .retryable(true);
    let answered = ChatMessage::new(
        "answer",
        ChatRole::Assistant,
        Progressive::complete("Use [[cite:pricing]] for the comparison.".to_owned()),
    )
    .citations([CitationRef::new(
        "pricing",
        "Pricing",
        "Open pricing",
        "app://pricing",
    )])
    .follow_ups([FollowUp::new("compare", "Compare suppliers")]);
    set_messages(&harness, Arc::from(vec![failed, answered]), cx);

    let retry = cx
        .debug_bounds("chat-retry-retry-me")
        .expect("retry action should render");
    cx.simulate_click(retry.center(), Modifiers::default());
    let citation = cx
        .debug_bounds("streaming-citation-pricing")
        .expect("citation action should render");
    cx.simulate_click(citation.center(), Modifiers::default());

    let follow_up = cx
        .debug_bounds("streaming-follow-up-compare")
        .expect("follow-up action should render");
    cx.simulate_click(follow_up.center(), Modifiers::default());

    #[cfg(not(target_os = "macos"))]
    {
        let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
        let prompt = chat.read_with(cx, |chat, _| chat.prompt_bar.clone());
        cx.update(|window, cx| {
            prompt.update(cx, |prompt, cx| {
                prompt.set_draft("Send from chat", window, cx);
                prompt.focus(window, cx);
            });
        });
        cx.simulate_keystrokes("enter");
    }

    harness.read_with(cx, |harness, _| {
        let events = harness.events.borrow();
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::RetryRequested { message_id } if message_id == "retry-me"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::FollowUpSelected { message_id, follow_up_id }
                if message_id == "answer" && follow_up_id == "compare"
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::CitationActivated { message_id, citation_id, destination }
                if message_id == "answer"
                    && citation_id == "pricing"
                    && destination == "app://pricing"
        )));
        #[cfg(not(target_os = "macos"))]
        assert!(events.iter().any(|event| matches!(
            event,
            ChatEvent::Prompt(PromptBarEvent::Submit { submission, .. })
                if submission.text() == "Send from chat"
        )));
    });
}

#[gpui::test]
fn rendered_follow_up_activates_from_keyboard_with_stable_ids(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(
        &harness,
        Arc::from([ChatMessage::new(
            "answer",
            ChatRole::Assistant,
            Progressive::complete("Choose the next comparison.".to_owned()),
        )
        .follow_ups([FollowUp::new("compare", "Compare suppliers")])]),
        cx,
    );

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");

    harness.read_with(cx, |harness, _| {
        assert!(harness.events.borrow().iter().any(|event| matches!(
            event,
            ChatEvent::FollowUpSelected { message_id, follow_up_id }
                if message_id == "answer" && follow_up_id == "compare"
        )));
    });
}

#[gpui::test]
fn switching_to_a_disjoint_conversation_is_not_a_burst_of_arrivals(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..12), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());

    // Read back through the transcript, so the next snapshot is not a
    // tail-following append.
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(0.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // A different conversation: no message identity in common.
    set_messages(&harness, messages(100..112), cx);
    cx.update(|window, cx| window.draw(cx).clear(cx));

    chat.read_with(cx, |chat, _| {
        assert_eq!(
            chat.unread_count(),
            0,
            "opening another conversation must not mark its history unread"
        );
        assert!(
            chat.arrivals.is_empty(),
            "a replacement is not twelve arrivals, so nothing should animate in or cue"
        );
        assert!(
            chat.pinned_to_bottom,
            "a different conversation opens at its end, not at the old scroll offset"
        );
    });
}

#[gpui::test]
fn per_message_state_does_not_outlive_its_message(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..6), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());

    cx.update(|window, cx| {
        chat.update(cx, |chat, cx| {
            chat.feedback.insert("m0001".into(), true);
            chat.copied_message = Some("m0002".into());
            chat.begin_edit("m0003", window, cx);
        });
    });
    chat.read_with(cx, |chat, _| {
        assert!(chat.editing.is_some(), "the edit session must start");
    });

    set_messages(&harness, messages(100..106), cx);

    chat.read_with(cx, |chat, _| {
        assert!(
            chat.feedback.is_empty(),
            "feedback keyed by message must not accumulate across conversations"
        );
        assert_eq!(
            chat.copied_message, None,
            "the copied marker belonged to a message that is gone"
        );
        assert!(
            chat.editing.is_none(),
            "an open editor must not keep pointing at a removed message"
        );
    });
}

#[gpui::test]
fn removing_the_edited_message_moves_focus_off_the_vanishing_editor(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..6), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());

    cx.update(|window, cx| {
        chat.update(cx, |chat, cx| chat.begin_edit("m0003", window, cx));
    });
    let editor = chat.read_with(cx, |chat, _| {
        chat.editing
            .as_ref()
            .expect("the edit session must start")
            .editor
            .clone()
    });
    cx.update(|window, cx| {
        assert!(
            editor.read(cx).focus_handle(cx).is_focused(window),
            "beginning an edit focuses its editor"
        );
    });

    // The application swaps in a conversation without the edited message.
    set_messages(&harness, messages(100..106), cx);

    cx.update(|window, cx| {
        assert!(
            !editor.read(cx).focus_handle(cx).is_focused(window),
            "focus must not stay on an editor that no longer exists"
        );
        let prompt = chat.read(cx).prompt_bar.clone();
        assert!(
            prompt.read(cx).focus_handle(cx).is_focused(window),
            "focus belongs on the composer, which is still there"
        );
    });
}

#[gpui::test]
fn rendered_jump_to_latest_activates_from_keyboard(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    set_messages(&harness, messages(0..60), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| {
        chat.list_state.scroll_to(ListOffset {
            item_ix: 12,
            offset_in_item: px(5.),
        });
        chat.pinned_to_bottom = false;
        cx.notify();
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    set_messages(&harness, messages(0..62), cx);

    cx.update(|window, cx| window.focus_next(cx));
    activate_key(cx, "enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    settle_jump(cx);

    assert!(cx.debug_bounds("chat-message-m0061").is_some());
    assert_eq!(chat.read_with(cx, |chat, _| chat.unread_count()), 0);
    harness.read_with(cx, |harness, _| {
        assert!(
            harness.events.borrow().contains(&ChatEvent::JumpedToLatest),
            "keyboard activation should emit the typed jump intent"
        );
    });
}

#[gpui::test]
fn constrained_chat_keeps_the_latest_message_and_composer_reachable(cx: &mut TestAppContext) {
    let (harness, cx) = harness(cx);
    cx.simulate_resize(size(px(360.), px(300.)));
    set_messages(&harness, messages(0..80), cx);
    let chat = harness.read_with(cx, |harness, _| harness.chat.clone());
    chat.update(cx, |chat, cx| chat.scroll_to_latest(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let transcript = cx
        .debug_bounds("chat-transcript")
        .expect("transcript should render");
    let latest = cx
        .debug_bounds("chat-message-m0079")
        .expect("latest message should render");
    let composer = cx
        .debug_bounds("chat-composer")
        .expect("composer should render");
    assert!(latest.bottom() <= transcript.bottom());
    assert!(composer.bottom() <= px(300.));
}

#[test]
fn chat_log_list_and_message_expose_direct_named_semantics() {
    let chat = chat_frame(&"semantic-chat".into()).into_element();
    let mut chat_node = accesskit::Node::new(Role::Unknown);
    chat.write_a11y_info(&mut chat_node);
    assert_eq!(chat.a11y_role(), Some(Role::Log));
    assert_eq!(chat_node.author_id(), Some("chat.semantic-chat"));
    assert_eq!(chat_node.label(), Some("Conversation"));

    let transcript = transcript_frame("transcript".into()).into_element();
    let mut transcript_node = accesskit::Node::new(Role::Unknown);
    transcript.write_a11y_info(&mut transcript_node);
    assert_eq!(transcript.a11y_role(), Some(Role::List));
    assert_eq!(transcript_node.label(), Some("Messages"));

    let message = ChatMessage::new(
        "semantic-message",
        ChatRole::Assistant,
        Progressive::failed(String::new(), "Network unavailable"),
    )
    .author("Mighty");
    let row = message_frame(&"semantic-chat".into(), &message).into_element();
    let mut row_node = accesskit::Node::new(Role::Unknown);
    row.write_a11y_info(&mut row_node);
    assert_eq!(row.a11y_role(), Some(Role::ListItem));
    assert_eq!(row_node.author_id(), Some("chat.message.semantic-message"));
    assert_eq!(row_node.label(), Some("Mighty, Assistant message"));
    assert_eq!(row_node.description(), Some("Failed: Network unavailable"));
}
