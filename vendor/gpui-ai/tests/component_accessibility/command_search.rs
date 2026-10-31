//! CommandSearch's keyboard path and row identity.
//!
//! The probe seeds one available and one unavailable command so the rows the
//! reader can see stay distinguishable from the one Enter is allowed to select.

use gpui::{
    AppContext as _, Context, Entity, Render, Subscription, TestAppContext, VisualTestContext,
    Window,
};
use gpui_ai::prelude::{CommandSearch, CommandSearchEvent, CommandSearchItem};
use std::{cell::RefCell, rc::Rc};

struct PublicCommandSearchProbe {
    search: Entity<CommandSearch>,
    events: Rc<RefCell<Vec<CommandSearchEvent>>>,
    _subscription: Subscription,
}

impl PublicCommandSearchProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| CommandSearch::new("public-command-search", window, cx));
        search.update(cx, |search, cx| {
            search.set_items(
                [
                    CommandSearchItem::new("pricing", "Open report")
                        .subtitle("Supplier pricing")
                        .keywords(["margin"])
                        .shortcut("Ctrl+R"),
                    CommandSearchItem::new("disabled", "Unavailable report").disabled(true),
                ],
                window,
                cx,
            );
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let subscription = cx.subscribe(&search, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            search,
            events,
            _subscription: subscription,
        }
    }
}

impl Render for PublicCommandSearchProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.search.clone()
    }
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
)]
#[gpui::test]
fn public_command_search_exposes_stable_keyboard_events_and_row_bounds(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicCommandSearchProbe::new);
    let cx: &mut VisualTestContext = cx;
    let search = probe.read_with(cx, |probe, _| probe.search.clone());
    cx.update(|window, cx| {
        search.update(cx, |search, cx| search.focus(window, cx));
        window.draw(cx).clear(cx);
    });

    assert!(
        cx.debug_bounds("command-search-public-command-search")
            .is_some()
    );
    assert!(cx.debug_bounds("command-search-item-pricing").is_some());
    assert!(cx.debug_bounds("command-search-item-disabled").is_some());

    cx.simulate_keystrokes("enter");
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [CommandSearchEvent::Selected {
            id: "public-command-search".into(),
            item_id: "pricing".into(),
        }]
    );
}
