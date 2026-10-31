//! Message branching and in-place editing: the version switcher reports a
//! zero-based index by stable ID and stops at either end; the editor opens
//! from the edit action, commits on Save, and abandons on Cancel.

use gpui::{
    AppContext as _, Context, Entity, Modifiers, ParentElement as _, Render, Styled as _,
    Subscription, TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_ai::{
    chat::{BranchPosition, Chat, ChatEvent, ChatMessage, ChatRole, MessageActions},
    prompt_bar::PromptBar,
    stream::StreamedContent,
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

struct Probe {
    chat: Entity<Chat>,
    _subscription: Subscription,
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
    let events: Rc<RefCell<Vec<ChatEvent>>> = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let events = events.clone();
        move |window, cx| {
            let prompt = cx.new(|cx| PromptBar::new("probe-prompt", window, cx));
            let chat = cx.new(|cx| Chat::new("probe", prompt, window, cx));
            let subscription = cx.subscribe(&chat, move |_, _, event: &ChatEvent, _| {
                events.borrow_mut().push(event.clone())
            });
            Probe {
                chat,
                _subscription: subscription,
            }
        }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(640.), px(520.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (view, events, cx)
}

fn conversation(position: BranchPosition) -> Arc<[ChatMessage]> {
    Arc::from([
        ChatMessage::new(
            "q-1",
            ChatRole::User,
            StreamedContent::done("Which supplier is safest?"),
        )
        .actions(MessageActions::for_role(ChatRole::User))
        .branch(position),
        ChatMessage::new(
            "a-1",
            ChatRole::Assistant,
            StreamedContent::done("Alpenrose, by a wide margin."),
        )
        .actions(MessageActions::for_role(ChatRole::Assistant)),
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
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn branch_switcher_reports_neighbouring_versions_and_stops_at_the_ends(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx);
    set_messages(&view, conversation(BranchPosition::new(1, 3)), cx);
    assert!(cx.debug_bounds("chat-branches-q-1").is_some());
    assert!(
        cx.debug_bounds("chat-branches-a-1").is_none(),
        "unbranched messages show no switcher"
    );

    click(cx, "chat-branch-prev-q-1");
    click(cx, "chat-branch-next-q-1");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ChatEvent::BranchSelected {
                message_id: "q-1".into(),
                index: 0
            },
            ChatEvent::BranchSelected {
                message_id: "q-1".into(),
                index: 2
            },
        ]
    );

    events.borrow_mut().clear();
    set_messages(&view, conversation(BranchPosition::new(0, 3)), cx);
    click(cx, "chat-branch-prev-q-1");
    assert!(
        events.borrow().is_empty(),
        "the first version has no previous version"
    );
    set_messages(&view, conversation(BranchPosition::new(2, 3)), cx);
    click(cx, "chat-branch-next-q-1");
    assert!(
        events.borrow().is_empty(),
        "the last version has no next version"
    );
}

#[test]
fn branch_positions_clamp_and_label_one_based() {
    let position = BranchPosition::new(7, 3);
    assert_eq!(position.index(), 2);
    assert_eq!(position.count(), 3);
    assert_eq!(position.label(), "Version 3 of 3");
    assert_eq!(BranchPosition::new(0, 0).count(), 1);
}

#[gpui::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
fn edit_action_opens_the_editor_and_save_reports_the_text(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx);
    set_messages(&view, conversation(BranchPosition::new(0, 1)), cx);

    click(cx, "chat-action-edit-q-1");
    assert!(cx.debug_bounds("chat-edit-editor-q-1").is_some());
    assert!(
        cx.debug_bounds("chat-action-edit-q-1").is_none(),
        "message actions hide while editing"
    );
    assert_eq!(
        view.read_with(cx, |probe, cx| probe
            .chat
            .read(cx)
            .editing_message()
            .cloned()),
        Some("q-1".into())
    );

    view.update_in(cx, |probe, window, cx| {
        probe.chat.update(cx, |chat, cx| {
            chat.set_edit_draft("Which supplier is cheapest?", window, cx);
        });
    });
    click(cx, "chat-edit-save-q-1");
    assert!(cx.debug_bounds("chat-edit-editor-q-1").is_none());
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ChatEvent::EditRequested {
                message_id: "q-1".into()
            },
            ChatEvent::EditSubmitted {
                message_id: "q-1".into(),
                text: "Which supplier is cheapest?".into(),
            },
        ]
    );
}

#[gpui::test]
#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused TextareaState"
)]
fn cancel_abandons_the_edit_and_empty_drafts_do_not_submit(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx);
    set_messages(&view, conversation(BranchPosition::new(0, 1)), cx);

    click(cx, "chat-action-edit-q-1");
    view.update_in(cx, |probe, window, cx| {
        probe.chat.update(cx, |chat, cx| {
            chat.set_edit_draft("   ", window, cx);
        });
    });
    click(cx, "chat-edit-save-q-1");
    assert!(
        cx.debug_bounds("chat-edit-editor-q-1").is_some(),
        "an empty draft keeps the editor open"
    );

    click(cx, "chat-edit-cancel-q-1");
    assert!(cx.debug_bounds("chat-edit-editor-q-1").is_none());
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ChatEvent::EditRequested {
                message_id: "q-1".into()
            },
            ChatEvent::EditCancelled {
                message_id: "q-1".into()
            },
        ]
    );
}
