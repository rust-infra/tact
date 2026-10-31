use gpui::{
    AppContext as _, Bounds, Context, Entity, Modifiers, ParentElement as _, Pixels, Render,
    Styled as _, Subscription, TestAppContext, VisualTestContext, Window, div, px, size,
};
use gpui_ai::thread_list::{ThreadItem, ThreadList, ThreadListEvent, ThreadSection};
use std::{cell::RefCell, rc::Rc};

struct Probe {
    threads: Entity<ThreadList>,
    _subscription: Subscription,
}

fn sections() -> Vec<ThreadSection> {
    vec![
        ThreadSection::new("today", "Today").items([
            ThreadItem::new("supplier", "Supplier pricing review").subtitle("2 min ago"),
            ThreadItem::new("cold-chain", "Cold-chain capacity check").subtitle("1 h ago"),
        ]),
        ThreadSection::new("earlier", "Earlier").items([
            ThreadItem::new("margins", "Q2 margin analysis").subtitle("Aug 12"),
            ThreadItem::new("packaging", "Packaging vendor shortlist")
                .subtitle("Aug 3")
                .archived(true),
        ]),
    ]
}

/// Ten thousand conversations, spread over ten sections.
const LARGE_SECTIONS: usize = 10;
const LARGE_SECTION_SIZE: usize = 1_000;

fn large_sections() -> Vec<ThreadSection> {
    (0..LARGE_SECTIONS)
        .map(|section| {
            ThreadSection::new(format!("s-{section}"), format!("Section {section}")).items(
                (0..LARGE_SECTION_SIZE).map(|row| {
                    let index = section * LARGE_SECTION_SIZE + row;
                    ThreadItem::new(format!("t-{index:05}"), format!("Conversation {index}"))
                        .subtitle(format!("{index} min ago"))
                }),
            )
        })
        .collect()
}

impl Probe {
    fn new(
        events: Rc<RefCell<Vec<ThreadListEvent>>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let threads = cx.new(|cx| {
            let mut list = ThreadList::new("probe", window, cx);
            list.set_sections(sections(), cx);
            list.set_active(Some("supplier"), cx);
            list
        });
        let subscription = cx.subscribe(&threads, move |_, _, event: &ThreadListEvent, _| {
            events.borrow_mut().push(event.clone());
        });
        Self {
            threads,
            _subscription: subscription,
        }
    }
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div().size_full().child(self.threads.clone())
    }
}

/// A caller's styles reach a stateful component's own frame.
///
/// The half of the library that is an entity used to have no way to take them:
/// `Styled` was implemented by every fluent builder and by none of the
/// entities, so `.w()` on a `ThreadList` did not compile and the only recourse
/// was a wrapper — which sizes the space around a component and not the
/// component. This narrows the list itself and reads a row inside it, because
/// a wrapper would leave the row exactly as wide as it was.
#[gpui::test]
fn a_caller_can_style_a_stateful_component(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let threads = cx.new(|cx| {
            let mut list = ThreadList::new("styled", window, cx);
            list.set_sections(sections(), cx);
            list
        });
        StyledProbe { threads }
    });
    cx.simulate_resize(size(px(320.), px(480.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let row = cx
        .debug_bounds("thread-row-supplier")
        .expect("a row should render");
    assert!(
        row.size.width <= px(NARROW),
        "a width set on the component must reach its rows, not stop at a wrapper: \
         row is {:?} wide in a {NARROW}px list",
        row.size.width
    );
}

const NARROW: f32 = 180.;

struct StyledProbe {
    threads: Entity<ThreadList>,
}

impl Render for StyledProbe {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // The style goes on the component, in a window three times as wide.
        self.threads.update(cx, |threads, _| {
            threads.style().size.width = Some(px(NARROW).into());
        });
        div().size_full().child(self.threads.clone())
    }
}

fn harness(
    cx: &mut TestAppContext,
    height: f32,
) -> (
    Entity<Probe>,
    Rc<RefCell<Vec<ThreadListEvent>>>,
    &mut VisualTestContext,
) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (view, cx) = cx.add_window_view({
        let events = events.clone();
        move |window, cx| Probe::new(events, window, cx)
    });
    cx.simulate_resize(size(px(320.), px(height)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (view, events, cx)
}

fn draw(cx: &mut VisualTestContext) {
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn click(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} should render"));
    cx.simulate_click(bounds.center(), Modifiers::default());
    draw(cx);
}

fn press(cx: &mut VisualTestContext, keystrokes: &str) {
    cx.simulate_keystrokes(keystrokes);
    draw(cx);
}

/// `debug_bounds` wants a `'static` selector; the large snapshots need one per
/// conversation, so a test-lifetime leak buys the lookup.
fn bounds_of(cx: &mut VisualTestContext, selector: String) -> Option<Bounds<Pixels>> {
    cx.debug_bounds(Box::leak(selector.into_boxed_str()))
}

fn set_sections(view: &Entity<Probe>, sections: Vec<ThreadSection>, cx: &mut VisualTestContext) {
    view.update_in(cx, |probe, _, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.set_sections(sections, cx));
    });
    draw(cx);
}

fn take_events(events: &Rc<RefCell<Vec<ThreadListEvent>>>) -> Vec<ThreadListEvent> {
    std::mem::take(&mut *events.borrow_mut())
}

#[gpui::test]
fn selecting_and_row_actions_report_stable_ids(cx: &mut TestAppContext) {
    let (_, events, cx) = harness(cx, 600.);
    click(cx, "thread-cold-chain");
    // Row actions live in a popup menu, so nothing of them exists until the
    // row's ellipsis opens it.
    assert!(cx.debug_bounds("thread-actions-menu").is_none());
    click(cx, "thread-more-supplier");
    assert!(cx.debug_bounds("thread-actions-menu").is_some());
    press(cx, "down enter");
    // Choosing an action dismisses the menu, so a stale target cannot linger.
    assert!(cx.debug_bounds("thread-actions-menu").is_none());

    click(cx, "thread-more-supplier");
    press(cx, "down down down enter");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ThreadListEvent::Selected {
                id: "cold-chain".into()
            },
            ThreadListEvent::RenameRequested {
                id: "supplier".into()
            },
            ThreadListEvent::DeleteRequested {
                id: "supplier".into()
            },
        ]
    );
    assert!(cx.debug_bounds("thread-actions-menu").is_none());
    click(cx, "thread-list-new");
    assert_eq!(events.borrow().last(), Some(&ThreadListEvent::NewRequested));
}

#[gpui::test]
fn archived_threads_hide_until_requested_and_can_be_restored(cx: &mut TestAppContext) {
    let (_, events, cx) = harness(cx, 600.);
    assert!(cx.debug_bounds("thread-packaging").is_none());
    click(cx, "thread-list-archived-toggle");
    assert!(cx.debug_bounds("thread-packaging").is_some());
    click(cx, "thread-more-packaging");
    press(cx, "down down enter");
    assert_eq!(
        events.borrow().as_slice(),
        &[ThreadListEvent::UnarchiveRequested {
            id: "packaging".into()
        }]
    );
}

#[gpui::test]
fn programmatic_query_filters_rows_and_reports_no_matches(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx, 600.);
    view.update_in(cx, |probe, window, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.set_query("margin", window, cx));
    });
    draw(cx);
    assert!(cx.debug_bounds("thread-supplier").is_none());
    assert!(cx.debug_bounds("thread-margins").is_some());
    assert_eq!(
        events.borrow().as_slice(),
        &[ThreadListEvent::QueryChanged {
            query: "margin".into()
        }]
    );

    view.update_in(cx, |probe, window, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.set_query("zzz", window, cx));
    });
    draw(cx);
    assert!(cx.debug_bounds("thread-margins").is_none());
    assert!(cx.debug_bounds("thread-list-empty").is_some());
}

#[gpui::test]
fn active_thread_survives_a_reordered_snapshot(cx: &mut TestAppContext) {
    let (view, _, cx) = harness(cx, 600.);
    let mut reordered = sections();
    reordered.reverse();
    set_sections(&view, reordered, cx);
    let active = view.read_with(cx, |probe, cx| probe.threads.read(cx).active_id().cloned());
    assert_eq!(active.as_deref(), Some("supplier"));
    assert!(cx.debug_bounds("thread-supplier").is_some());
}

#[gpui::test]
fn constrained_list_keeps_the_last_thread_reachable(cx: &mut TestAppContext) {
    let (view, _, cx) = harness(cx, 150.);
    let host = cx
        .debug_bounds("thread-list-scroll")
        .expect("scroll region should render");
    view.update_in(cx, |probe, _, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.scroll_to_end(cx));
    });
    draw(cx);
    let last = cx
        .debug_bounds("thread-margins")
        .expect("last visible thread should render");
    assert!(
        last.bottom() <= host.bottom() + px(1.),
        "last thread {last:?} must be inside the scroll region {host:?}"
    );
    assert!(last.top() >= host.top() - px(1.));
}

#[gpui::test]
fn search_keeps_one_line_height_under_an_overlong_query(cx: &mut TestAppContext) {
    let (view, _, cx) = harness(cx, 600.);
    let one_line = cx
        .debug_bounds("thread-list-search")
        .expect("the search field should render")
        .size
        .height;
    view.update_in(cx, |probe, window, cx| {
        probe.threads.update(cx, |threads, cx| {
            threads.set_query("margin analysis ".repeat(24), window, cx);
        });
    });
    draw(cx);
    // Upstream mounts its editor scrollbar only for multi-line input, so a
    // single-line field keeps exactly one line of height however long its
    // value grows; the overflow stays reachable inside the field instead of
    // wrapping, growing, or gaining a scrollbar strip.
    let overlong = cx
        .debug_bounds("thread-list-search")
        .expect("the search field should survive an overlong query")
        .size
        .height;
    assert_eq!(one_line, overlong);
    assert!(cx.debug_bounds("thread-list-empty").is_some());
    view.update_in(cx, |probe, window, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.set_query("", window, cx));
    });
    draw(cx);
    assert!(cx.debug_bounds("thread-supplier").is_some());
}

/// Ten thousand conversations must cost a screenful, on the first draw and
/// again after scrolling. Rows outside the window are absent from the frame,
/// not merely painted off-screen.
#[gpui::test]
fn a_ten_thousand_thread_snapshot_only_builds_its_window(cx: &mut TestAppContext) {
    let (view, _, cx) = harness(cx, 480.);
    set_sections(&view, large_sections(), cx);

    assert!(
        bounds_of(cx, "thread-t-00000".to_owned()).is_some(),
        "the first conversation should be on screen"
    );
    for far in ["thread-t-00200", "thread-t-05000", "thread-t-09999"] {
        assert!(
            bounds_of(cx, far.to_owned()).is_none(),
            "{far} is outside the window and must not be built"
        );
    }

    view.update_in(cx, |probe, _, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.scroll_to_end(cx));
    });
    draw(cx);
    assert!(
        bounds_of(cx, "thread-t-09999".to_owned()).is_some(),
        "the last conversation should be reachable"
    );
    assert!(
        bounds_of(cx, "thread-t-00000".to_owned()).is_none(),
        "the first conversation scrolled away and must not still be built"
    );
}

/// The whole listbox keyboard model, on stable IDs: Up/Down step over the
/// visible options only, Home/End reach the bounds, Enter and Space select
/// without the pointer, and neither end wraps.
#[gpui::test]
fn the_listbox_walks_visible_rows_with_the_keyboard(cx: &mut TestAppContext) {
    let (_, events, cx) = harness(cx, 600.);
    // Clicking takes the roving focus with it, so the keyboard starts where
    // the pointer left off.
    click(cx, "thread-supplier");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "supplier".into()
        }]
    );

    press(cx, "down");
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "cold-chain".into()
        }],
        "Down moves focus without selecting; Enter selects"
    );

    // Down crosses the section header without ever landing on it, and stops
    // at the last option rather than wrapping. "packaging" is archived and
    // hidden, so it is not an option at all.
    press(cx, "down down down");
    press(cx, "space");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "margins".into()
        }],
        "Down skips headers and hidden rows, and the last option is the floor"
    );

    press(cx, "home");
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "supplier".into()
        }]
    );

    press(cx, "up");
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "supplier".into()
        }],
        "the first option is the ceiling"
    );

    press(cx, "end");
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "margins".into()
        }],
        "End reaches the last visible option, not the archived one behind it"
    );
}

/// Escape closes the menu and hands focus back to the row, which the keyboard
/// then proves by moving and selecting without another click.
#[gpui::test]
fn escape_closes_the_menu_and_returns_focus_to_the_list(cx: &mut TestAppContext) {
    let (_, events, cx) = harness(cx, 600.);
    click(cx, "thread-supplier");
    take_events(&events);

    click(cx, "thread-more-supplier");
    assert!(cx.debug_bounds("thread-actions-menu").is_some());
    press(cx, "escape");
    assert!(
        cx.debug_bounds("thread-actions-menu").is_none(),
        "Escape closes the menu"
    );

    press(cx, "down");
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "cold-chain".into()
        }],
        "focus returned to the listbox, so its keyboard model still answers"
    );
}

/// A press outside the menu dismisses it without invoking anything.
#[gpui::test]
fn an_outside_press_dismisses_the_menu(cx: &mut TestAppContext) {
    let (_, events, cx) = harness(cx, 600.);
    click(cx, "thread-more-supplier");
    assert!(cx.debug_bounds("thread-actions-menu").is_some());
    click(cx, "thread-list-search");
    assert!(cx.debug_bounds("thread-actions-menu").is_none());
    assert_eq!(
        take_events(&events),
        vec![],
        "dismissing a menu is not choosing from it"
    );
}

/// Keyboard focus is an ID, not an index: it survives a reordered snapshot and
/// a narrowing query, and is dropped only when the thread it names goes away.
#[gpui::test]
fn keyboard_focus_survives_reorder_and_filtering(cx: &mut TestAppContext) {
    let (view, events, cx) = harness(cx, 600.);
    click(cx, "thread-margins");
    take_events(&events);

    let mut reordered = sections();
    reordered.reverse();
    set_sections(&view, reordered, cx);
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "margins".into()
        }],
        "a reordered snapshot must not move the focus onto another thread"
    );

    view.update_in(cx, |probe, window, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.set_query("margin", window, cx));
    });
    draw(cx);
    take_events(&events);
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "margins".into()
        }],
        "a query that keeps the focused thread keeps the focus"
    );

    view.update_in(cx, |probe, window, cx| {
        probe
            .threads
            .update(cx, |threads, cx| threads.set_query("supplier", window, cx));
    });
    draw(cx);
    take_events(&events);
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![],
        "a query that hides the focused thread drops the focus rather than \
         selecting whatever now sits at its index"
    );
    press(cx, "home");
    press(cx, "enter");
    assert_eq!(
        take_events(&events),
        vec![ThreadListEvent::Selected {
            id: "supplier".into()
        }],
        "Home re-enters the narrowed list at its first option"
    );
}
