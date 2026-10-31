//! The navigation's unit tests, moved verbatim from the root module.

#![cfg(test)]

use super::render::{sidebar_item_control, sidebar_section_control, sidebar_tree_container};
use super::rows::{VisibleRow, collect_parent_ids, snapshot_ids_are_unique, visible_rows};
use super::*;
use gpui::{
    Bounds, Element as _, ListAlignment, ListOffset, ListState, RenderOnce as _, ScrollDelta,
    ScrollWheelEvent, TestAppContext, VisualTestContext, accesskit, canvas, list, px, size,
};
use gpui_component::theme::Theme;
use std::sync::{Arc, Mutex};

type CapturedSemanticNodes = Arc<Mutex<Vec<(Option<Role>, accesskit::Node)>>>;

struct SemanticsProbe {
    rows: Arc<[VisibleRow]>,
    captured: CapturedSemanticNodes,
}

struct SidebarInCatalogProbe {
    nav: Entity<SidebarNav>,
    catalog: ListState,
}

fn probe_sections() -> Vec<SidebarSection> {
    vec![
        SidebarSection::new("workspace", "Workspace").items([
            SidebarNavItem::new("overview", "Overview"),
            SidebarNavItem::new("orders", "Orders")
                .badge("12")
                .children([
                    SidebarNavItem::new("history", "History"),
                    SidebarNavItem::new("suppliers", "Suppliers")
                        .children([SidebarNavItem::new("risk", "Risk reports")]),
                    SidebarNavItem::new("exports", "Exports").disabled(true),
                ]),
        ]),
        SidebarSection::new("reports", "Reports")
            .items([SidebarNavItem::new("live-report", "Reports")]),
    ]
}

/// Every parent expanded, which is what `set_sections` does for a snapshot
/// whose parents are all new.
fn every_parent(sections: &[SidebarSection]) -> HashSet<SharedString> {
    let mut parents = HashSet::new();
    for section in sections {
        collect_parent_ids(&section.items, &mut parents);
    }
    parents
}

fn row_ids(rows: &[VisibleRow]) -> Vec<&str> {
    rows.iter().map(|row| row.id.as_ref()).collect()
}

fn row<'a>(rows: &'a [VisibleRow], id: &str) -> &'a VisibleRow {
    rows.iter()
        .find(|row| row.id == id)
        .expect("the row should be in the snapshot")
}

impl SidebarInCatalogProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| {
            let mut nav = SidebarNav::new("nested-nav", window, cx);
            nav.set_sections(
                [
                    SidebarSection::new("workspace", "Workspace").items((0..30).map(|index| {
                        SidebarNavItem::new(
                            format!("destination-{index}"),
                            format!("Destination {index}"),
                        )
                    })),
                ],
                cx,
            );
            nav
        });
        Self {
            nav,
            catalog: ListState::new(8, ListAlignment::Top, px(0.)),
        }
    }
}

/// A standalone rail whose collapse glide the width tests measure.
struct CollapseProbe {
    nav: Entity<SidebarNav>,
}

impl CollapseProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| {
            let mut nav = SidebarNav::new("collapse-nav", window, cx);
            nav.set_sections(probe_sections(), cx);
            nav
        });
        Self { nav }
    }
}

impl Render for CollapseProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(480.)).h(px(360.)).child(self.nav.clone())
    }
}

fn rail_width(cx: &mut VisualTestContext) -> Pixels {
    cx.debug_bounds("sidebar-nav-collapse-nav")
        .expect("the rail should render")
        .size
        .width
}

#[gpui::test]
fn collapsing_glides_the_rail_width_between_its_poles(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (probe, cx) = cx.add_window_view(CollapseProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    let wide = rail_width(cx);

    nav.update(cx, |nav, cx| nav.set_collapsed(true, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(80));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let mid = rail_width(cx);

    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let narrow = rail_width(cx);

    assert!(narrow < wide, "collapse must narrow the rail");
    assert!(
        narrow < mid && mid < wide,
        "mid-glide the rail must sit between its poles: narrow {narrow:?} mid {mid:?} wide {wide:?}"
    );
}

#[gpui::test]
fn reduced_motion_collapses_without_travel(cx: &mut TestAppContext) {
    cx.update(crate::init);
    cx.update(|cx| cx.set_reduce_motion(true));
    let (probe, cx) = cx.add_window_view(CollapseProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    let wide = rail_width(cx);

    nav.update(cx, |nav, cx| nav.set_collapsed(true, cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let snapped = rail_width(cx);
    assert!(snapped < wide, "collapse must narrow the rail");

    cx.executor()
        .advance_clock(std::time::Duration::from_secs(2));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        rail_width(cx),
        snapped,
        "reduced motion must land on the pole in one frame"
    );
}

/// A nav in a box too short for its rows, so the row list scrolls.
struct ShortSidebarProbe {
    nav: Entity<SidebarNav>,
}

impl ShortSidebarProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| {
            let mut nav = SidebarNav::new("zoom-nav", window, cx);
            nav.set_sections(
                (0..4).map(|section| {
                    SidebarSection::new(format!("section-{section}"), format!("Section {section}"))
                        .items((0..8).map(|item| {
                            SidebarNavItem::new(
                                format!("s{section}-item-{item}"),
                                format!("Destination {section}.{item}"),
                            )
                        }))
                }),
                cx,
            );
            nav
        });
        Self { nav }
    }
}

impl Render for ShortSidebarProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(320.)).h(px(240.)).child(self.nav.clone())
    }
}

/// One section holding ten thousand descendants, all expanded.
struct HugeSidebarProbe {
    nav: Entity<SidebarNav>,
}

impl HugeSidebarProbe {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let nav = cx.new(|cx| {
            let mut nav = SidebarNav::new("huge-nav", window, cx);
            nav.set_sections(
                [
                    SidebarSection::new("workspace", "Workspace").items((0..100).map(|parent| {
                        SidebarNavItem::new(format!("parent-{parent}"), format!("Parent {parent}"))
                            .children((0..99).map(move |child| {
                                SidebarNavItem::new(
                                    format!("item-{parent}-{child}"),
                                    format!("Item {parent}.{child}"),
                                )
                            }))
                    })),
                ],
                cx,
            );
            nav
        });
        Self { nav }
    }
}

impl Render for HugeSidebarProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().w(px(320.)).h(px(240.)).child(self.nav.clone())
    }
}

impl Render for SidebarInCatalogProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let nav = self.nav.clone();
        div().w(px(320.)).h(px(260.)).child(
            list(self.catalog.clone(), move |index, _, _| {
                if index == 0 {
                    div()
                        .w(px(280.))
                        .h(px(180.))
                        .child(nav.clone())
                        .into_any_element()
                } else {
                    div().w(px(280.)).h(px(100.)).into_any_element()
                }
            })
            .size_full(),
        )
    }
}

impl Render for SemanticsProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let captured = self.captured.clone();
        let rows = self.rows.clone();
        canvas(
            move |_, window, cx| {
                let component_id: SharedString = "test".into();
                let collapsed = false;
                let nodes = rows
                    .iter()
                    .map(|row| {
                        let control = if row.header {
                            sidebar_section_control(&component_id, row, false, cx)
                        } else {
                            sidebar_item_control(
                                &component_id,
                                row,
                                false,
                                collapsed,
                                None,
                                &SidebarNavLabels::default(),
                                cx,
                            )
                                .on_click(|_, _, _| {})
                        }
                        .render(window, cx)
                        .into_element();
                        let mut node = accesskit::Node::new(Role::Unknown);
                        control.write_a11y_info(&mut node);
                        (control.a11y_role(), node)
                    })
                    .collect();
                *captured.lock().expect("capture mutex should be available") = nodes;
            },
            |_, _, _, _| {},
        )
    }
}

/// Captures the AccessKit node of every row in `rows`, in row order.
fn capture_semantics(
    cx: &mut TestAppContext,
    rows: Arc<[VisibleRow]>,
) -> Vec<(Option<Role>, accesskit::Node)> {
    cx.update(crate::init);
    let captured: CapturedSemanticNodes = Arc::new(Mutex::new(Vec::new()));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| SemanticsProbe { rows, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex should be available")
        .drain(..)
        .collect()
}

#[test]
fn the_snapshot_flattens_expanded_branches_and_states_their_relationships() {
    let sections = probe_sections();
    let rows = visible_rows(&sections, "", &every_parent(&sections), None, false);

    assert_eq!(
        row_ids(&rows),
        [
            "workspace",
            "overview",
            "orders",
            "history",
            "suppliers",
            "risk",
            "exports",
            "reports",
            "live-report",
        ]
    );

    let workspace = row(&rows, "workspace");
    assert!(workspace.header);
    assert_eq!(
        (workspace.level, workspace.position, workspace.set_size),
        (1, 1, 2)
    );

    let orders = row(&rows, "orders");
    assert_eq!(orders.parent.as_deref(), Some("workspace"));
    assert_eq!((orders.level, orders.position, orders.set_size), (2, 2, 2));
    assert!(orders.has_children && orders.expanded);

    let suppliers = row(&rows, "suppliers");
    assert_eq!(suppliers.parent.as_deref(), Some("orders"));
    assert_eq!(
        (suppliers.level, suppliers.position, suppliers.set_size),
        (3, 2, 3)
    );

    let risk = row(&rows, "risk");
    assert_eq!(risk.parent.as_deref(), Some("suppliers"));
    assert_eq!((risk.level, risk.indent), (4, 2));
    assert!(!risk.has_children);
}

#[test]
fn a_collapsed_rail_flattens_root_items_without_headers() {
    let sections = probe_sections();
    let rows = visible_rows(&sections, "", &every_parent(&sections), None, true);

    assert_eq!(row_ids(&rows), ["overview", "orders", "live-report"]);
    assert_eq!(row(&rows, "orders").level, 1);
}

#[test]
fn a_collapsed_rail_marks_the_ancestor_of_the_controlled_active_item() {
    let sections = probe_sections();
    let expanded = every_parent(&sections);
    let active: SharedString = "risk".into();

    let expanded_rows = visible_rows(&sections, "", &expanded, Some(&active), false);
    assert!(row(&expanded_rows, "risk").active);
    assert!(!row(&expanded_rows, "orders").active);
    assert!(row(&expanded_rows, "orders").contains_active);

    let rail = visible_rows(&sections, "", &expanded, Some(&active), true);
    assert!(row(&rail, "orders").active);
}

#[test]
fn a_controlled_active_descendant_stays_visible_through_a_collapsed_parent() {
    let sections = probe_sections();
    let active: SharedString = "risk".into();
    // Nothing the reader expanded: only the controlled active ancestry
    // opens the path to it.
    let rows = visible_rows(&sections, "", &HashSet::new(), Some(&active), false);

    assert_eq!(
        row_ids(&rows),
        [
            "workspace",
            "overview",
            "orders",
            "history",
            "suppliers",
            "risk",
            "exports",
            "reports",
            "live-report"
        ]
    );
}

#[test]
fn the_projection_retains_only_the_matching_branch_and_its_ancestors() {
    let sections = probe_sections();
    let rows = visible_rows(&sections, "RISK", &HashSet::new(), None, false);

    assert_eq!(row_ids(&rows), ["workspace", "orders", "suppliers", "risk"]);
    // A query reveals matched ancestry without the reader having expanded
    // anything, and position metadata describes the visible set.
    let suppliers = row(&rows, "suppliers");
    assert!(suppliers.expanded);
    assert_eq!((suppliers.position, suppliers.set_size), (1, 1));
}

#[test]
fn a_matching_section_label_keeps_the_whole_section() {
    let sections = probe_sections();
    let rows = visible_rows(&sections, "workspace", &HashSet::new(), None, false);

    assert!(row_ids(&rows).contains(&"exports"));
    assert!(!row_ids(&rows).contains(&"reports"));
}

#[test]
fn duplicate_labels_are_valid_but_duplicate_ids_are_rejected() {
    let duplicate_labels = [SidebarSection::new("reports", "Reports").items([
        SidebarNavItem::new("live", "Reports"),
        SidebarNavItem::new("archive", "Reports"),
    ])];
    assert!(snapshot_ids_are_unique(&duplicate_labels));

    let duplicate_ids = [SidebarSection::new("reports", "Reports").items([
        SidebarNavItem::new("same", "Live"),
        SidebarNavItem::new("same", "Archive"),
    ])];
    assert!(!snapshot_ids_are_unique(&duplicate_ids));
}

#[gpui::test]
fn tree_rows_expose_level_position_expansion_and_activation_semantics(cx: &mut TestAppContext) {
    let sections = probe_sections();
    let active: SharedString = "risk".into();
    let rows = visible_rows(
        &sections,
        "",
        &every_parent(&sections),
        Some(&active),
        false,
    );
    let ids = row_ids(&rows)
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();
    let nodes = capture_semantics(cx, rows);
    let node = |id: &str| {
        let index = ids
            .iter()
            .position(|candidate| candidate == id)
            .expect("the row should be in the snapshot");
        &nodes[index]
    };

    // A section is the parent of its items, so it is level 1 and its root
    // items are level 2: parent and child relationships a flattened tree
    // can only state, never nest.
    let (workspace_role, workspace) = node("workspace");
    assert_eq!(*workspace_role, Some(Role::TreeItem));
    assert_eq!(workspace.label(), Some("Workspace"));
    assert_eq!(workspace.level(), Some(1));
    assert_eq!(workspace.position_in_set(), Some(1));
    assert_eq!(workspace.size_of_set(), Some(2));
    assert_eq!(workspace.is_expanded(), Some(true));

    let (orders_role, orders) = node("orders");
    assert_eq!(*orders_role, Some(Role::TreeItem));
    assert_eq!(orders.level(), Some(2));
    assert_eq!(orders.position_in_set(), Some(2));
    assert_eq!(orders.size_of_set(), Some(2));
    assert_eq!(orders.is_expanded(), Some(true));
    assert_eq!(orders.description(), Some("Badge 12"));
    assert!(orders.supports_action(accesskit::Action::Click));

    let (_, risk) = node("risk");
    assert_eq!(risk.label(), Some("Risk reports"));
    assert_eq!(risk.level(), Some(4));
    assert_eq!(risk.is_selected(), Some(true));
    assert_eq!(risk.is_expanded(), None, "a leaf has nothing to expand");

    let (disabled_role, disabled) = node("exports");
    assert_eq!(*disabled_role, Some(Role::TreeItem));
    assert_eq!(disabled.description(), Some("Unavailable"));
    assert_eq!(disabled.position_in_set(), Some(3));
    assert_eq!(disabled.size_of_set(), Some(3));
    assert!(!disabled.supports_action(accesskit::Action::Click));
}

#[gpui::test]
fn a_collapsed_parent_reports_unmounted_children_as_not_expanded(cx: &mut TestAppContext) {
    let sections = probe_sections();
    let active: SharedString = "risk".into();
    let rail = visible_rows(&sections, "", &every_parent(&sections), Some(&active), true);
    let ids = row_ids(&rail)
        .iter()
        .map(|id| id.to_string())
        .collect::<Vec<_>>();

    // The rail renders through the same control with `collapsed` set, so
    // capture it the way the row list does.
    cx.update(crate::init);
    let captured: CapturedSemanticNodes = Arc::new(Mutex::new(Vec::new()));
    let result = captured.clone();
    let rows = rail.clone();
    let (_, cx) = cx.add_window_view(move |_, _| RailSemanticsProbe { rows, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nodes = result
        .lock()
        .expect("capture mutex should be available")
        .drain(..)
        .collect::<Vec<_>>();

    let index = ids
        .iter()
        .position(|candidate| candidate == "orders")
        .expect("the rail should render root items");
    let (role, parent) = &nodes[index];
    assert_eq!(*role, Some(Role::TreeItem));
    assert_eq!(parent.label(), Some("Orders"));
    assert_eq!(parent.is_selected(), Some(true));
    assert_eq!(
        parent.description(),
        Some("Badge 12. Contains selected item")
    );
    assert_eq!(parent.is_expanded(), Some(false));
}

struct RailSemanticsProbe {
    rows: Arc<[VisibleRow]>,
    captured: CapturedSemanticNodes,
}

impl Render for RailSemanticsProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let captured = self.captured.clone();
        let rows = self.rows.clone();
        canvas(
            move |_, window, cx| {
                let component_id: SharedString = "test".into();
                let nodes = rows
                    .iter()
                    .map(|row| {
                        let control =
                            sidebar_item_control(
                                &component_id,
                                row,
                                false,
                                true,
                                None,
                                &SidebarNavLabels::default(),
                                cx,
                            )
                                .on_click(|_, _, _| {})
                                .render(window, cx)
                                .into_element();
                        let mut node = accesskit::Node::new(Role::Unknown);
                        control.write_a11y_info(&mut node);
                        (control.a11y_role(), node)
                    })
                    .collect();
                *captured.lock().expect("capture mutex should be available") = nodes;
            },
            |_, _, _, _| {},
        )
    }
}

#[gpui::test]
fn the_tree_container_is_the_navigation_tree(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured: CapturedSemanticNodes = Arc::new(Mutex::new(Vec::new()));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| TreeContainerProbe { captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nodes = result
        .lock()
        .expect("capture mutex should be available")
        .drain(..)
        .collect::<Vec<_>>();

    let (role, tree) = &nodes[0];
    assert_eq!(*role, Some(Role::Tree));
    assert_eq!(tree.label(), Some("Navigation items"));
}

struct TreeContainerProbe {
    captured: CapturedSemanticNodes,
}

impl Render for TreeContainerProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, _, _| {
                let tree =
                    sidebar_tree_container(&"test".into(), "Navigation items").into_element();
                let mut node = accesskit::Node::new(Role::Unknown);
                tree.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") =
                    vec![(tree.a11y_role(), node)];
            },
            |_, _, _, _| {},
        )
    }
}

/// Zooms the way the shell does: the theme carries the base type size and
/// `Root` hands it to the window every frame.
///
/// Two draws, because the nav notices the new rem while rendering and
/// reacts afterwards. Nothing here calls `remeasure`; that the nav does it
/// unprompted is the property under test.
fn zoom_to(cx: &mut VisualTestContext, font_size: f32) {
    cx.update(|window, cx| {
        Theme::global_mut(cx).font_size = px(font_size);
        window.set_rem_size(Theme::global(cx).font_size);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

fn top_row(nav: &SidebarNav) -> Option<SharedString> {
    let offset = nav.row_list.logical_scroll_top();
    nav.rows.get(offset.item_ix).map(|row| row.id.clone())
}

#[gpui::test]
fn zooming_re_measures_rows_and_keeps_the_first_visible_row(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (probe, cx) = cx.add_window_view(ShortSidebarProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());

    // Rest on a row other than the first, so preserving the anchor is a
    // claim about the row and not about the top of the list.
    nav.read_with(cx, |nav, _| {
        nav.row_list.scroll_to(ListOffset {
            item_ix: 4,
            offset_in_item: px(0.),
        })
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let anchor = nav.read_with(cx, |nav, _| top_row(nav));
    assert_eq!(anchor.as_deref(), Some("s0-item-3"));

    // 100%, 150%, 200% of the 16px base.
    for font_size in [16., 24., 32.] {
        zoom_to(cx, font_size);

        nav.read_with(cx, |nav, _| {
            assert!(
                nav.resolved_layout.matches(px(font_size)),
                "the nav must notice {font_size}px type from its own render"
            );
            assert_eq!(
                top_row(nav),
                anchor,
                "the row that was first on screen stays first at {font_size}px type"
            );
        });
        assert!(
            cx.debug_bounds("sidebar-nav-item-s0-item-3").is_some(),
            "the anchored row stays reachable at {font_size}px type"
        );
    }
}

#[gpui::test]
fn one_section_of_ten_thousand_descendants_constructs_a_bounded_window(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (probe, cx) = cx.add_window_view(HugeSidebarProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    let nav = probe.read_with(cx, |probe, _| probe.nav.clone());
    nav.update(cx, |nav, _| nav.constructed_rows.set(0));
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let (rows, first_draw) =
        nav.read_with(cx, |nav, _| (nav.rows.len(), nav.constructed_rows.get()));
    assert_eq!(rows, 10_001, "one header over ten thousand descendants");
    assert!(
        (1..=64).contains(&first_draw),
        "a 240px viewport constructed {first_draw} of {rows} rows"
    );

    // One page further down is still one window of work, not a walk from
    // the top of the section.
    nav.update(cx, |nav, _| {
        nav.row_list.scroll_to(ListOffset {
            item_ix: 5_000,
            offset_in_item: px(0.),
        });
        nav.constructed_rows.set(0);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let scrolled = nav.read_with(cx, |nav, _| nav.constructed_rows.get());
    assert!(
        (1..=64).contains(&scrolled),
        "scrolled draw constructed {scrolled} of {rows} rows"
    );
    // Row 5,000 is the last child of parent 49, five thousand rows past
    // anything the first draw touched.
    assert!(cx.debug_bounds("sidebar-nav-item-item-49-98").is_some());
}

#[gpui::test]
fn wheel_over_scrollable_sidebar_moves_sidebar_before_catalog(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (probe, cx) = cx.add_window_view(SidebarInCatalogProbe::new);
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let nav_bounds = cx
        .debug_bounds("sidebar-nav-nested-nav")
        .expect("the nested sidebar should render");
    let (nav_list, catalog) = probe.read_with(cx, |probe, cx| {
        (probe.nav.read(cx).row_list.clone(), probe.catalog.clone())
    });
    assert!(nav_list.max_offset_for_scrollbar().y > px(0.));

    cx.simulate_event(ScrollWheelEvent {
        position: nav_bounds.center(),
        delta: ScrollDelta::Pixels(gpui::point(px(0.), px(-40.))),
        ..Default::default()
    });

    assert_eq!(nav_list.scroll_px_offset_for_scrollbar().y, px(-40.));
    let catalog_top = catalog.logical_scroll_top();
    assert_eq!(
        (catalog_top.item_ix, catalog_top.offset_in_item),
        (0, px(0.))
    );

    // Rows are measured one window at a time, so the end of the list moves
    // further down as more of them are measured. Settle there before
    // asserting that the wheel is released: releasing early would hand the
    // catalog a wheel the nav could still use. (Virtualizing rows rather
    // than section roots is what made this take more than one pass.)
    let mut bottom = nav_list.scroll_px_offset_for_scrollbar().y;
    for _ in 0..8 {
        nav_list.scroll_by(nav_list.max_offset_for_scrollbar().y);
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let settled = nav_list.scroll_px_offset_for_scrollbar().y;
        if settled == bottom {
            break;
        }
        bottom = settled;
    }
    cx.simulate_event(ScrollWheelEvent {
        position: nav_bounds.center(),
        delta: ScrollDelta::Pixels(gpui::point(px(0.), px(-40.))),
        ..Default::default()
    });

    assert_eq!(nav_list.scroll_px_offset_for_scrollbar().y, bottom);
    let catalog_top = catalog.logical_scroll_top();
    assert_ne!(
        (catalog_top.item_ix, catalog_top.offset_in_item),
        (0, px(0.))
    );
}

/// A nav inside a host box of an arbitrary size, in either presentation.
struct HostedNavProbe {
    nav: Entity<SidebarNav>,
    size: gpui::Size<Pixels>,
}

impl HostedNavProbe {
    fn new(
        presentation: SidebarNavPresentation,
        host: gpui::Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let nav = cx.new(|cx| {
            let mut nav = SidebarNav::new("hosted-nav", window, cx).with_presentation(presentation);
            nav.set_sections(probe_sections(), cx);
            nav
        });
        Self { nav, size: host }
    }
}

impl Render for HostedNavProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("hosted-nav-host")
            .debug_selector(|| "hosted-nav-host".to_owned())
            .w(self.size.width)
            .h(self.size.height)
            .flex()
            .child(self.nav.clone())
    }
}

/// Draws one hosted nav and returns the host's and the nav's bounds.
fn hosted_nav_bounds(
    cx: &mut TestAppContext,
    presentation: SidebarNavPresentation,
    host: gpui::Size<Pixels>,
) -> (Bounds<Pixels>, Bounds<Pixels>) {
    cx.update(crate::init);
    let (_, cx) =
        cx.add_window_view(move |window, cx| HostedNavProbe::new(presentation, host, window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(900.), px(700.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    (
        cx.debug_bounds("hosted-nav-host")
            .expect("the host should render"),
        cx.debug_bounds("sidebar-nav-hosted-nav")
            .expect("the navigation should render"),
    )
}

#[gpui::test]
fn an_embedded_shell_fills_a_wide_short_host(cx: &mut TestAppContext) {
    let (host, nav) = hosted_nav_bounds(
        cx,
        SidebarNavPresentation::Embedded,
        size(px(620.), px(160.)),
    );

    // The bottom-dock case: an embedded nav takes the width it is given
    // rather than a rail's, which is the whole point of the mode.
    assert_eq!(nav.size, host.size);
    assert_eq!(nav.origin, host.origin);
}

#[gpui::test]
fn an_embedded_shell_fills_a_narrow_tall_host(cx: &mut TestAppContext) {
    let (host, nav) = hosted_nav_bounds(
        cx,
        SidebarNavPresentation::Embedded,
        size(px(196.), px(460.)),
    );

    // Narrower than the expanded rail: the shell shrinks with the host
    // instead of overflowing it.
    assert_eq!(nav.size, host.size);
    assert_eq!(nav.origin, host.origin);
}

#[gpui::test]
fn the_default_shell_keeps_a_rail_width_inside_a_wide_host(cx: &mut TestAppContext) {
    let host_size = size(px(620.), px(360.));
    let (host, nav) = hosted_nav_bounds(cx, SidebarNavPresentation::Standalone, host_size);

    assert_eq!(nav.size.height, host.size.height);
    assert!(
        nav.size.width < host.size.width,
        "a standalone rail sizes itself: {:?} in a {:?} host",
        nav.size,
        host.size,
    );
}

#[gpui::test]
fn the_shells_differ_only_by_width_growth_and_trailing_edge(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (nav, cx) = cx.add_window_view(|window, cx| SidebarNav::new("shell", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();

    let (standalone, embedded, rail) = cx.update(|_, cx| {
        let rail = cx.theme().semantic_tokens().spacing.xxl * 8.;
        let standalone = nav.update(cx, |nav, cx| nav.render_shell(1.0, cx).style().clone());
        let embedded = nav.update(cx, |nav, cx| {
            nav.presentation = SidebarNavPresentation::Embedded;
            let style = nav.render_shell(1.0, cx).style().clone();
            nav.presentation = SidebarNavPresentation::Standalone;
            style
        });
        (standalone, embedded, rail)
    });

    let expected_rail = div().w(rail).flex_none().border_r_1().style().clone();
    assert_eq!(standalone.size.width, expected_rail.size.width);
    assert_eq!(standalone.flex_grow, expected_rail.flex_grow);
    assert_eq!(standalone.flex_shrink, expected_rail.flex_shrink);
    assert_eq!(standalone.border_widths, expected_rail.border_widths);

    // Embedded keeps neither the rail's width nor its edge; the host draws
    // whatever divider the composition needs.
    let expected_embedded = div().w_full().style().clone();
    assert_eq!(embedded.size.width, expected_embedded.size.width);
    assert_eq!(embedded.flex_grow, expected_embedded.flex_grow);
    assert_eq!(embedded.flex_shrink, expected_embedded.flex_shrink);
    assert_eq!(embedded.border_widths, expected_embedded.border_widths);

    // Everything the shell is not asked to change is shared, so the seam
    // cannot quietly become a second skin.
    assert_eq!(standalone.size.height, embedded.size.height);
    assert_eq!(standalone.background, embedded.background);
    assert_eq!(standalone.text.color, embedded.text.color);
}
