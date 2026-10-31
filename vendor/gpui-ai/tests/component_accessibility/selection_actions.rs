//! SelectionActions' selection lifecycle and its action toolbar.
//!
//! Every test drives a real drag through `SelectionTestRoot`, the minimal host
//! that mounts the text-selection layer and the Copy action, because the
//! contract under test is what survives a selection: the toolbar appearing only
//! once a drag settles, the selected text outliving an action press, and the
//! clearing paths that put it away again.

use gpui::{
    AppContext as _, ClipboardItem, Context, Entity, InteractiveElement as _, Modifiers,
    MouseButton, ParentElement as _, Render, ScrollDelta, ScrollWheelEvent, Styled as _,
    Subscription, TestAppContext, VisualTestContext, Window, div, point, px,
};
use gpui_ai::selection_actions::{SelectionAction, SelectionActions, SelectionActionsEvent};
use std::{cell::RefCell, rc::Rc};

struct PublicSelectionProbe {
    selection: Entity<SelectionActions>,
    events: Rc<RefCell<Vec<SelectionActionsEvent>>>,
    _subscription: Subscription,
}

struct SelectionTestRoot<V: Render + 'static> {
    view: Entity<V>,
}

impl<V: Render + 'static> SelectionTestRoot<V> {
    fn new(view: Entity<V>) -> Self {
        Self { view }
    }

    fn on_copy(
        &mut self,
        _: &gpui_component::input::Copy,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let selected = gpui_base::TextSelection::selected_text(window, cx)
            .trim()
            .to_string();
        if selected.is_empty() {
            cx.propagate();
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(selected));
        }
    }
}

impl<V: Render + 'static> Render for SelectionTestRoot<V> {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .id("selection-test-root")
            .key_context("Root")
            .relative()
            .size_full()
            .on_action(cx.listener(Self::on_copy))
            .child(gpui_base::TextSelectionLayer)
            .child(self.view.clone())
    }
}

struct BoundedSelectionProbe {
    selection: Entity<SelectionActions>,
}

impl BoundedSelectionProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selection = cx.new(|cx| {
            SelectionActions::new(
                "bounded-selection",
                "Selectable action words for testing outside release and narrow overflow.",
                window,
                cx,
            )
        });
        selection.update(cx, |selection, cx| {
            selection.set_actions(
                [
                    SelectionAction::new("ask", "Ask about this selection"),
                    SelectionAction::new("explain", "Explain this selected passage"),
                    SelectionAction::new("rewrite", "Rewrite this selected passage clearly"),
                    SelectionAction::new("compare", "Compare this passage with the source"),
                    SelectionAction::new("final", "Open the final long selection action"),
                ],
                cx,
            );
        });
        Self { selection }
    }
}

impl Render for BoundedSelectionProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .size_full()
            .flex()
            .flex_col()
            .child(
                div()
                    .debug_selector(|| "bounded-selection-host".to_owned())
                    .w(px(184.))
                    .h(px(144.))
                    .child(self.selection.clone()),
            )
            .child(
                div()
                    .debug_selector(|| "selection-actions-outside-target".to_owned())
                    .flex_1(),
            )
    }
}

impl PublicSelectionProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selection = cx.new(|cx| {
            SelectionActions::new(
                "public-selection",
                "Selectable action words for testing.",
                window,
                cx,
            )
        });
        selection.update(cx, |selection, cx| {
            selection.set_actions(
                [
                    SelectionAction::new("ask", "Ask"),
                    SelectionAction::new("explain", "Explain"),
                    SelectionAction::new("rewrite", "Rewrite"),
                ],
                cx,
            );
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.subscribe(&selection, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            selection,
            events,
            _subscription,
        }
    }
}

impl Render for PublicSelectionProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.selection.clone()
    }
}

#[gpui::test]
fn public_selection_actions_preserve_selection_and_activate_typed_events(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let probe = cx.new(|cx| PublicSelectionProbe::new(window, cx));
        SelectionTestRoot::new(probe)
    });
    let probe = root.read_with(cx, |root, _| root.view.clone());
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let surface = cx
        .debug_bounds("selection-actions-surface")
        .expect("selection surface should render");
    let from = point(surface.left() + px(14.), surface.top() + px(14.));
    let to = point(surface.right() - px(14.), surface.top() + px(24.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    assert!(
        cx.debug_bounds("selection-actions-toolbar").is_none(),
        "toolbar must stay hidden while selection drag is active"
    );
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let selected = probe.read_with(cx, |probe, cx| {
        probe.selection.read(cx).selected_text().to_string()
    });
    assert!(selected.contains("Selectable action words"), "{selected:?}");
    let ask = cx
        .debug_bounds("selection-action-ask")
        .expect("settled selection should expose Ask");
    cx.simulate_mouse_down(ask.center(), MouseButton::Left, Modifiers::default());
    assert!(probe.read_with(cx, |probe, cx| {
        !probe.selection.read(cx).selected_text().is_empty()
    }));
    cx.simulate_mouse_up(ask.center(), MouseButton::Left, Modifiers::default());
    assert!(probe.read_with(cx, |probe, _| {
        let events = probe.events.borrow();
        let matched = events.iter().any(|event| {
            matches!(
                event,
                SelectionActionsEvent::Invoked {
                    id,
                    action_id,
                    selected_text,
                } if id == "public-selection"
                    && action_id == "ask"
                    && selected_text.contains("Selectable action words")
            )
        });
        assert!(matched, "unexpected selection events: {events:?}");
        true
    }));

    probe.update(cx, |probe, cx| {
        probe.selection.update(cx, |selection, cx| {
            selection.set_markdown("Replacement content", cx)
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("selection-actions-toolbar").is_none());
    assert!(probe.read_with(cx, |probe, cx| {
        probe.selection.read(cx).selected_text().is_empty()
    }));
}

#[gpui::test]
fn public_selection_actions_follow_keyboard_select_all_and_copy(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let probe = cx.new(|cx| PublicSelectionProbe::new(window, cx));
        SelectionTestRoot::new(probe)
    });
    let probe = root.read_with(cx, |root, _| root.view.clone());
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let surface = cx
        .debug_bounds("selection-actions-surface")
        .expect("selection surface should render");

    let focus_from = point(surface.left() + px(14.), surface.top() + px(14.));
    let focus_to = point(surface.left() + px(42.), surface.top() + px(14.));
    cx.simulate_mouse_down(focus_from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(focus_to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(focus_to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-a");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-a");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(
        probe.read_with(cx, |probe, cx| probe
            .selection
            .read(cx)
            .selected_text()
            .to_string()),
        "Selectable action words for testing."
    );
    assert!(cx.debug_bounds("selection-actions-toolbar").is_some());

    #[cfg(target_os = "macos")]
    cx.simulate_keystrokes("cmd-c");
    #[cfg(not(target_os = "macos"))]
    cx.simulate_keystrokes("ctrl-c");
    let clipboard = cx.update(|_, cx| cx.read_from_clipboard().and_then(|item| item.text()));
    assert_eq!(
        clipboard.as_deref(),
        Some("Selectable action words for testing.")
    );
}

#[gpui::test]
fn public_selection_actions_clear_after_an_outside_left_click(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let probe = cx.new(|cx| BoundedSelectionProbe::new(window, cx));
        SelectionTestRoot::new(probe)
    });
    let probe = root.read_with(cx, |root, _| root.view.clone());
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let surface = cx
        .debug_bounds("selection-actions-surface")
        .expect("selection surface should render");
    let from = point(surface.left() + px(14.), surface.top() + px(14.));
    let to = point(surface.right() - px(14.), surface.top() + px(24.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("selection-actions-toolbar").is_some());

    let outside = cx
        .debug_bounds("selection-actions-outside-target")
        .expect("outside target should render");
    cx.simulate_click(outside.center(), Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("selection-actions-toolbar").is_none());
    assert!(probe.read_with(cx, |probe, cx| {
        probe.selection.read(cx).selected_text().is_empty()
    }));
}

#[gpui::test]
fn public_selection_actions_follow_native_empty_selection(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let probe = cx.new(|cx| BoundedSelectionProbe::new(window, cx));
        SelectionTestRoot::new(probe)
    });
    let probe = root.read_with(cx, |root, _| root.view.clone());
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let surface = cx
        .debug_bounds("selection-actions-surface")
        .expect("selection surface should render");
    let from = point(surface.left() + px(14.), surface.top() + px(14.));
    let to = point(surface.right() - px(14.), surface.top() + px(24.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("selection-actions-toolbar").is_some());

    cx.update(gpui_base::TextSelection::clear);
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("selection-actions-toolbar").is_none());
    assert!(probe.read_with(cx, |probe, cx| {
        probe.selection.read(cx).selected_text().is_empty()
    }));
}

#[gpui::test]
fn public_selection_actions_settle_when_a_drag_releases_outside(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (root, cx) = cx.add_window_view(|window, cx| {
        let probe = cx.new(|cx| BoundedSelectionProbe::new(window, cx));
        SelectionTestRoot::new(probe)
    });
    let probe = root.read_with(cx, |root, _| root.view.clone());
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let surface = cx
        .debug_bounds("selection-actions-surface")
        .expect("selection surface should render");
    let outside = cx
        .debug_bounds("selection-actions-outside-target")
        .expect("outside target should render");
    let from = point(surface.left() + px(14.), surface.top() + px(14.));
    let release = point(surface.right() - px(8.), outside.top() + px(12.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(release, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(release, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(probe.read_with(cx, |probe, cx| {
        !probe.selection.read(cx).selected_text().is_empty()
    }));
    assert!(cx.debug_bounds("selection-actions-toolbar").is_some());
}

#[gpui::test]
fn public_selection_actions_keep_long_final_action_reachable_in_a_narrow_root(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let probe = cx.new(|cx| BoundedSelectionProbe::new(window, cx));
        SelectionTestRoot::new(probe)
    });
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let surface = cx
        .debug_bounds("selection-actions-surface")
        .expect("selection surface should render");
    let from = point(surface.left() + px(14.), surface.top() + px(14.));
    let to = point(surface.right() - px(14.), surface.top() + px(24.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let toolbar = cx
        .debug_bounds("selection-actions-toolbar")
        .expect("toolbar should render after selection");
    // The toolbar is placed by the upstream positioner, whose containment
    // boundary is the viewport rather than the narrow root the selection
    // lives in: a toolbar wider than its host clamps on-screen instead of
    // clipping inside it. Reachability is asserted against the window and,
    // below, against the toolbar's own scrollable frame.
    let viewport = cx.update(|window, _| window.viewport_size());
    assert!(toolbar.left() >= px(0.), "{toolbar:?}");
    assert!(
        toolbar.right() <= viewport.width,
        "{toolbar:?} vs {viewport:?}"
    );

    for _ in 0..12 {
        cx.simulate_event(ScrollWheelEvent {
            position: toolbar.center(),
            delta: ScrollDelta::Pixels(point(px(-120.), px(0.))),
            ..Default::default()
        });
    }
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let final_action = cx
        .debug_bounds("selection-action-final")
        .expect("final action should remain rendered after horizontal scrolling");
    assert!(
        final_action.left() >= toolbar.left() && final_action.right() <= toolbar.right(),
        "{final_action:?} vs {toolbar:?}"
    );
    assert!(
        final_action.left() >= px(0.) && final_action.right() <= viewport.width,
        "{final_action:?} vs {viewport:?}"
    );
}
