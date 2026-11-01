//! SidebarNav's tree navigation, filter, and collapsed header.
//!
//! One shared section fixture carries the shapes the contract depends on —
//! nested parents, an unavailable row, and two items with the same label but
//! different stable IDs — so identity, filtering, hover, and keyboard walking
//! are all asserted against the same catalog. A second, forty-section probe
//! covers the virtualized scroll path.

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, Modifiers, ParentElement as _,
    Render, ScrollDelta, ScrollWheelEvent, Styled as _, Subscription, TestAppContext,
    VisualTestContext, Window, div, point, px,
};
use gpui_ai::prelude::{SidebarNav, SidebarNavEvent, SidebarNavItem, SidebarSection};
use gpui_component::IconName;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use crate::harness::activate_key;

struct PublicSidebarNavProbe {
    nav: Entity<SidebarNav>,
    events: Rc<RefCell<Vec<SidebarNavEvent>>>,
    _subscription: Subscription,
}

fn sidebar_sections() -> Vec<SidebarSection> {
    vec![
        SidebarSection::new("workspace", "Workspace").items([
            SidebarNavItem::new("overview", "Overview").icon(IconName::LayoutDashboard),
            SidebarNavItem::new("orders", "Orders")
                .icon(IconName::SquareTerminal)
                .badge("12")
                .children([
                    SidebarNavItem::new("history", "History"),
                    SidebarNavItem::new("suppliers", "Suppliers").children([
                        SidebarNavItem::new("supplier-risk", "Risk reports"),
                        SidebarNavItem::new("supplier-score", "Scorecards"),
                    ]),
                    SidebarNavItem::new("exports", "Exports").disabled(true),
                ]),
        ]),
        SidebarSection::new("reports", "Reports").items([
            SidebarNavItem::new("live-report", "Reports").icon(IconName::ChartPie),
            SidebarNavItem::new("archive-report", "Reports").icon(IconName::BookOpen),
        ]),
    ]
}

impl PublicSidebarNavProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| SidebarNav::new("public-sidebar", window, cx));
        nav.update(cx, |nav, cx| {
            nav.set_sections(sidebar_sections(), cx);
            nav.set_active_item("archive-report", cx);
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let subscription = cx.subscribe(&nav, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            nav,
            events,
            _subscription: subscription,
        }
    }
}

impl Render for PublicSidebarNavProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .debug_selector(|| "public-sidebar-host".to_owned())
            .w(px(260.))
            .h(px(520.))
            .overflow_hidden()
            .child(self.nav.clone())
    }
}

struct OverflowSidebarProbe {
    nav: Entity<SidebarNav>,
}

impl OverflowSidebarProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| SidebarNav::new("overflow-sidebar", window, cx));
        nav.update(cx, |nav, cx| {
            nav.set_sections(
                (0..40).map(|index| {
                    SidebarSection::new(format!("section-{index}"), format!("Section {index}"))
                        .items([SidebarNavItem::new(
                            format!("overflow-{index}"),
                            format!("Navigation item {index}"),
                        )])
                }),
                cx,
            );
            nav.set_active_item("overflow-39", cx);
        });
        Self { nav }
    }
}

impl Render for OverflowSidebarProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        div()
            .debug_selector(|| "overflow-sidebar-host".to_owned())
            .w(px(260.))
            .h(px(220.))
            .overflow_hidden()
            .child(self.nav.clone())
    }
}

#[gpui::test]
fn public_sidebar_nav_filters_recursively_and_routes_duplicate_labels_by_stable_id(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("sidebar-nav-item-live-report").is_some());
    let live = cx
        .debug_bounds("sidebar-nav-item-live-report")
        .expect("the duplicate-label item should render by stable ID");
    cx.simulate_click(
        point(live.left() + px(12.), live.center().y),
        Modifiers::default(),
    );
    activate_key(cx, "enter");
    activate_key(cx, "space");

    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("risk", window, cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("sidebar-nav-item-orders").is_some());
    assert!(cx.debug_bounds("sidebar-nav-item-suppliers").is_some());
    assert!(cx.debug_bounds("sidebar-nav-item-supplier-risk").is_some());
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_none());
    assert!(cx.debug_bounds("sidebar-nav-item-live-report").is_none());

    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "live-report".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "live-report".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "live-report".into(),
            },
            SidebarNavEvent::QueryChanged {
                id: "public-sidebar".into(),
                query: "risk".into(),
            },
        ]
    );
}

/// The row's accessibility overlay blocks pointer input for everything
/// under it, so the presentation beneath can never see hover: the crate
/// owes the row a hover of its own. It draws one gliding highlight over
/// the hovered row — and a row hovered under the pointer must produce
/// one, at that row's own geometry. Between R1.2 and this test the
/// sidebar had no hover at all, because the probe that would have caught
/// it had been re-baselined into a pure layout assertion.
#[gpui::test]
fn public_sidebar_nav_paints_hover_for_the_row_under_the_pointer(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("sidebar-nav-glide").is_none(),
        "nothing is hovered yet, so nothing is highlighted"
    );

    let overview = cx
        .debug_bounds("sidebar-nav-item-overview")
        .expect("expanded row should render through the production tree");
    cx.simulate_mouse_move(overview.center(), None, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let highlight = cx
        .debug_bounds("sidebar-nav-glide")
        .expect("the row under the pointer must be highlighted");
    // The highlight fills the row inside its border box, so the focus ring
    // the row draws on that border stays visible around it.
    let inset = px(1.);
    assert_eq!(highlight.origin.x, overview.origin.x + inset);
    assert_eq!(highlight.origin.y, overview.origin.y + inset);
    assert_eq!(highlight.size.width, overview.size.width - inset * 2.);
    assert_eq!(highlight.size.height, overview.size.height - inset * 2.);

    // Leaving the rows drops it, the way a per-row hover fill would.
    let host = cx
        .debug_bounds("public-sidebar-host")
        .expect("sidebar host should remain rendered");
    cx.simulate_mouse_move(
        point(host.right() + px(20.), host.bottom() + px(20.)),
        None,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar-nav-glide").is_none(),
        "leaving the rows extinguishes the highlight"
    );
}

#[gpui::test]
fn public_sidebar_nav_native_hover_survives_stationary_pointer_replacement_and_query(
    cx: &mut TestAppContext,
) {
    // The crate's own accessibility overlay blocks pointer input for the
    // row, so the crate — not the upstream item — must paint hover; the
    // companion test below guards that it does. What this one guards is
    // layout: with a stationary pointer, stable
    // rows keep their exact bounds through snapshot replacement and a
    // programmatic query, and the row under the pointer stays the row the
    // click lands on.
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let overview = cx
        .debug_bounds("sidebar-nav-item-overview")
        .expect("expanded row should render through the production tree");
    cx.simulate_mouse_move(overview.center(), None, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let orders = cx
        .debug_bounds("sidebar-nav-item-orders")
        .expect("second expanded row should render");
    cx.simulate_mouse_move(orders.center(), None, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    let mut replacement = sidebar_sections();
    replacement.truncate(1);
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_sections(replacement, cx));
        window.draw(cx).clear(cx);
    });
    assert_eq!(
        cx.debug_bounds("sidebar-nav-item-orders"),
        Some(orders),
        "stable replacement should leave Orders under the stationary pointer"
    );

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("workspace", window, cx));
        window.draw(cx).clear(cx);
    });
    assert_eq!(
        cx.debug_bounds("sidebar-nav-item-orders"),
        Some(orders),
        "section-label filtering should preserve the hovered row layout"
    );

    cx.simulate_click(orders.center(), Modifiers::default());
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::QueryChanged {
                id: "public-sidebar".into(),
                query: "workspace".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "orders".into(),
            },
        ]
    );

    // Leaving the host entirely still leaves the layout untouched: hover
    // is style state, never structure.
    let host = cx
        .debug_bounds("public-sidebar-host")
        .expect("sidebar host should remain rendered");
    cx.simulate_mouse_move(
        point(host.right() + px(20.), host.bottom() + px(20.)),
        None,
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.debug_bounds("sidebar-nav-item-orders"),
        Some(orders),
        "pointer exit must not restructure the rows"
    );
}

#[gpui::test]
fn public_sidebar_nav_suppresses_disabled_selection_and_emits_collapse_identity(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let disabled = cx
        .debug_bounds("sidebar-nav-item-exports")
        .expect("disabled item should remain visible and named");
    assert!(cx.debug_bounds("sidebar-nav-filter").is_some());
    cx.simulate_click(disabled.center(), Modifiers::default());
    assert!(probe.read_with(cx, |probe, _| probe.events.borrow().is_empty()));

    let new_task = cx
        .debug_bounds("sidebar-nav-new-task")
        .expect("new-task control should render");
    cx.simulate_click(new_task.center(), Modifiers::default());

    let collapse = cx
        .debug_bounds("sidebar-nav-collapse")
        .expect("collapse control should render");
    cx.simulate_click(collapse.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-item-overview").is_some());
    assert!(cx.debug_bounds("sidebar-nav-filter").is_none());
    assert!(cx.debug_bounds("sidebar-nav-new-task").is_none());

    let host = cx
        .debug_bounds("public-sidebar-host")
        .expect("the constrained sidebar host should remain available");
    let expand = cx
        .debug_bounds("sidebar-nav-collapse")
        .expect("collapsed navigation should expose one expand control");
    assert!(expand.left() >= host.left(), "{expand:?} vs {host:?}");
    assert!(expand.right() <= host.right(), "{expand:?} vs {host:?}");
    assert!(expand.size.width >= px(30.), "{expand:?}");

    // Calling focus_filter while collapsed must not move focus into its
    // unmounted input.
    cx.update(|window, cx| nav.update(cx, |nav, cx| nav.focus_filter(window, cx)));
    cx.simulate_keystrokes("risk");
    assert_eq!(nav.read_with(cx, |nav, _| nav.query().clone()), "");

    // Pointer completes the collapsed-to-expanded round trip.
    cx.simulate_click(expand.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-filter").is_some());

    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::NewTaskRequested {
                id: "public-sidebar".into(),
            },
            SidebarNavEvent::CollapsedChanged {
                id: "public-sidebar".into(),
                collapsed: true,
            },
            SidebarNavEvent::CollapsedChanged {
                id: "public-sidebar".into(),
                collapsed: false,
            },
        ]
    );
}

#[gpui::test]
fn public_sidebar_nav_keyboard_expands_the_only_compact_header_control(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_collapsed(true, cx));
        window.draw(cx).clear(cx);
    });

    cx.update(|window, cx| {
        window.focus_next(cx);
        assert!(window.focused(cx).is_some());
        window.draw(cx).clear(cx);
    });
    activate_key(cx, "enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(!nav.read_with(cx, |nav, _| nav.is_collapsed()));
    assert!(cx.debug_bounds("sidebar-nav-filter").is_some());
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::CollapsedChanged {
                id: "public-sidebar".into(),
                collapsed: true,
            },
            SidebarNavEvent::CollapsedChanged {
                id: "public-sidebar".into(),
                collapsed: false,
            },
        ]
    );
}

#[cfg_attr(
    target_os = "macos",
    ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
)]
#[gpui::test]
fn public_sidebar_nav_native_filter_typing_updates_query_and_emits_identity(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.focus_filter(window, cx));
    });
    cx.simulate_keystrokes("risk");
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert_eq!(nav.read_with(cx, |nav, _| nav.query().clone()), "risk");
    assert!(cx.debug_bounds("sidebar-nav-item-supplier-risk").is_some());
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().last().cloned()),
        Some(SidebarNavEvent::QueryChanged {
            id: "public-sidebar".into(),
            query: "risk".into(),
        })
    );
}

#[gpui::test]
fn public_sidebar_nav_programmatic_query_notifies_while_filter_is_unmounted(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_collapsed(true, cx));
        window.draw(cx).clear(cx);
    });
    probe.read_with(cx, |probe, _| probe.events.borrow_mut().clear());

    let notifications = Rc::new(Cell::new(0));
    let observed = notifications.clone();
    let _observation =
        cx.update(|_, cx| cx.observe(&nav, move |_, _| observed.set(observed.get() + 1)));
    cx.update(|window, cx| nav.update(cx, |nav, cx| nav.set_query("risk", window, cx)));

    assert_eq!(notifications.get(), 1);
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [SidebarNavEvent::QueryChanged {
            id: "public-sidebar".into(),
            query: "risk".into(),
        }]
    );

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_collapsed(false, cx));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("sidebar-nav-item-orders").is_some());
    assert!(cx.debug_bounds("sidebar-nav-item-supplier-risk").is_some());
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_none());
    assert_eq!(
        probe.read_with(cx, |probe, _| {
            probe
                .events
                .borrow()
                .iter()
                .filter(|event| matches!(event, SidebarNavEvent::QueryChanged { .. }))
                .count()
        }),
        1
    );
}

#[gpui::test]
fn public_sidebar_nav_preserves_active_identity_after_controlled_reorder(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| {
            nav.set_sections(
                [
                    SidebarSection::new("reports", "Reports").items([
                        SidebarNavItem::new("archive-report", "Reports").icon(IconName::BookOpen),
                        SidebarNavItem::new("live-report", "Reports").icon(IconName::ChartPie),
                    ]),
                    sidebar_sections().remove(0),
                ],
                cx,
            )
        });
        window.draw(cx).clear(cx);
    });

    assert!(
        cx.debug_bounds("sidebar-nav-active-archive-report")
            .is_some()
    );
    assert!(cx.debug_bounds("sidebar-nav-active-live-report").is_none());
}

#[gpui::test]
fn public_sidebar_nav_keeps_controlled_active_descendants_reachable(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let orders = cx
        .debug_bounds("sidebar-nav-item-orders")
        .expect("parent route should render");
    cx.simulate_click(orders.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_none());

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_active_item("supplier-risk", cx));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("sidebar-nav-item-supplier-risk").is_some());
    assert!(
        cx.debug_bounds("sidebar-nav-active-supplier-risk")
            .is_some()
    );

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_collapsed(true, cx));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("sidebar-nav-active-orders").is_some());
    assert!(cx.debug_bounds("sidebar-nav-item-supplier-risk").is_none());
}

#[gpui::test]
fn public_sidebar_nav_parent_activation_intentionally_selects_and_toggles(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let orders = cx
        .debug_bounds("sidebar-nav-item-orders")
        .expect("parent route should render");
    cx.simulate_click(orders.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_none());
    cx.simulate_click(orders.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_some());

    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "orders".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "orders".into(),
            },
        ]
    );
}

#[gpui::test]
fn public_sidebar_nav_distinguishes_empty_catalog_from_no_results(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("absent", window, cx));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("sidebar-nav-no-results").is_some());
    assert!(cx.debug_bounds("sidebar-nav-empty").is_none());

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_sections([], cx));
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("sidebar-nav-no-results").is_none());
    assert!(cx.debug_bounds("sidebar-nav-empty").is_some());
}

#[gpui::test]
fn public_sidebar_nav_scrolls_the_final_stable_item_into_the_constrained_viewport(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(OverflowSidebarProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let host = cx
        .debug_bounds("overflow-sidebar-host")
        .expect("constrained sidebar host should render");
    assert!(cx.debug_bounds("sidebar-nav-item-overflow-39").is_none());

    // A frame per wheel event, because the nav now virtualizes rows rather
    // than whole sections: each frame measures the window it drew, so the
    // reachable end of a forty-section list is discovered as the reader
    // scrolls toward it rather than known from the first frame.
    for _ in 0..36 {
        cx.simulate_event(ScrollWheelEvent {
            position: host.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-180.))),
            ..Default::default()
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let final_item = cx
        .debug_bounds("sidebar-nav-item-overflow-39")
        .expect("the final stable item should enter the rendered range after scrolling");
    assert!(final_item.top() >= host.top(), "{final_item:?} vs {host:?}");
    assert!(
        final_item.bottom() <= host.bottom(),
        "{final_item:?} vs {host:?}"
    );
}

#[gpui::test]
fn public_sidebar_nav_tree_keyboard_walks_rows_honors_bounds_and_skips_unavailable(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // Pointer activation is what puts a reader inside the tree: it moves the
    // roving row onto what it activated and focuses the tree itself, so the
    // arrow keys start from a row rather than from wherever the list rests.
    let overview = cx
        .debug_bounds("sidebar-nav-item-overview")
        .expect("a root item should render");
    assert!(
        cx.debug_bounds("sidebar-nav-item-exports").is_some(),
        "the unavailable row is rendered, so skipping it is a navigation claim"
    );
    cx.simulate_click(overview.center(), Modifiers::default());

    // End reaches the last visible row; Home reaches the section header, which
    // names its items and carries no application intent of its own.
    activate_key(cx, "end");
    activate_key(cx, "enter");
    activate_key(cx, "home");
    activate_key(cx, "enter");
    activate_key(cx, "down");
    activate_key(cx, "space");

    // Down to the last enabled descendant, then past the unavailable row into
    // the next section.
    for _ in 0..5 {
        activate_key(cx, "down");
    }
    activate_key(cx, "down");
    activate_key(cx, "down");
    activate_key(cx, "enter");

    // Up steps over the same unavailable row on the way back.
    activate_key(cx, "up");
    activate_key(cx, "up");
    activate_key(cx, "enter");

    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "overview".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "archive-report".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "overview".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "live-report".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "supplier-score".into(),
            },
        ]
    );
}

#[gpui::test]
fn public_sidebar_nav_tree_keyboard_expands_collapses_and_walks_parents(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let overview = cx
        .debug_bounds("sidebar-nav-item-overview")
        .expect("a root item should render");
    cx.simulate_click(overview.center(), Modifiers::default());
    activate_key(cx, "down");

    activate_key(cx, "left");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar-nav-item-history").is_none(),
        "Left collapses the expanded parent the reader is standing on"
    );

    activate_key(cx, "right");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar-nav-item-history").is_some(),
        "Right expands the collapsed parent the reader is standing on"
    );

    // A second Right enters the first child; Left from a leaf walks back out
    // to the parent that owns it.
    activate_key(cx, "right");
    activate_key(cx, "enter");
    activate_key(cx, "left");
    activate_key(cx, "enter");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("sidebar-nav-item-history").is_none(),
        "activating the parent toggled it the way a click does"
    );

    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "overview".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "history".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "orders".into(),
            },
        ]
    );
}

#[gpui::test]
fn public_sidebar_nav_filter_reveals_matched_ancestry_and_restores_expansion(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let suppliers = cx
        .debug_bounds("sidebar-nav-item-suppliers")
        .expect("the nested parent should render");
    cx.simulate_click(suppliers.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-item-supplier-risk").is_none());

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("risk", window, cx));
        window.draw(cx).clear(cx);
    });
    assert!(
        cx.debug_bounds("sidebar-nav-item-supplier-risk").is_some(),
        "a query exposes the ancestry it matched inside a collapsed parent"
    );
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_none());

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("", window, cx));
        window.draw(cx).clear(cx);
    });
    assert!(
        cx.debug_bounds("sidebar-nav-item-supplier-risk").is_none(),
        "clearing the query restores the expansion the reader chose, not the one it revealed"
    );
    assert!(cx.debug_bounds("sidebar-nav-item-history").is_some());
}

#[gpui::test]
fn public_sidebar_nav_keyboard_focus_survives_a_filter_round_trip(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let history = cx
        .debug_bounds("sidebar-nav-item-history")
        .expect("the nested leaf should render");
    cx.simulate_click(history.center(), Modifiers::default());

    // Filtering rebuilds the rows around a smaller projection; the focused row
    // is named, not numbered, so it comes through both directions intact.
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("history", window, cx));
        window.draw(cx).clear(cx);
    });
    activate_key(cx, "enter");

    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("", window, cx));
        window.draw(cx).clear(cx);
    });
    activate_key(cx, "enter");

    let selections = probe.read_with(cx, |probe, _| {
        probe
            .events
            .borrow()
            .iter()
            .filter_map(|event| match event {
                SidebarNavEvent::Selected { item_id, .. } => Some(item_id.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    });
    assert_eq!(selections, ["history", "history", "history"]);
}

#[gpui::test]
fn public_sidebar_nav_keyboard_focus_survives_a_controlled_reorder(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    let cx: &mut VisualTestContext = cx;
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let history = cx
        .debug_bounds("sidebar-nav-item-history")
        .expect("the nested leaf should render");
    cx.simulate_click(history.center(), Modifiers::default());

    // The same rows in a different order: focus is retained by stable ID, so
    // it stays on the row it was on rather than on the position it occupied.
    let mut reordered = sidebar_sections();
    reordered.reverse();
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_sections(reordered, cx));
        window.draw(cx).clear(cx);
    });
    assert!(
        cx.debug_bounds("sidebar-nav-active-archive-report")
            .is_some(),
        "the controlled active marker survives the reorder too"
    );

    activate_key(cx, "enter");
    assert_eq!(
        probe.read_with(cx, |probe, _| probe.events.borrow().clone()),
        [
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "history".into(),
            },
            SidebarNavEvent::Selected {
                id: "public-sidebar".into(),
                item_id: "history".into(),
            },
        ]
    );
}

#[gpui::test]
fn sidebar_filter_keeps_one_line_height_under_an_overlong_query(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (probe, cx) = cx.add_window_view(PublicSidebarNavProbe::new);
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    let one_line = cx
        .debug_bounds("sidebar-nav-filter")
        .expect("the filter field should render")
        .size
        .height;
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| {
            nav.set_query("wholesale scorecards ".repeat(24), window, cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    // A single-line filter holds one line of height however long the query
    // grows; upstream reserves its editor scrollbar for multi-line input.
    let overlong = cx
        .debug_bounds("sidebar-nav-filter")
        .expect("the filter field should survive an overlong query")
        .size
        .height;
    assert_eq!(one_line, overlong);
    cx.update(|window, cx| {
        nav.update(cx, |nav, cx| nav.set_query("", window, cx));
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(cx.debug_bounds("sidebar-nav-item-orders").is_some());
}
