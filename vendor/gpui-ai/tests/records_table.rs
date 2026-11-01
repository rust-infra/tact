use std::sync::Arc;

use std::{cell::RefCell, rc::Rc};

use gpui::{
    AppContext as _, Context, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render,
    Subscription, TestAppContext, VisualTestContext, Window, px, size,
};
use gpui_ai::prelude::{
    Progressive, RecordCell, RecordCellKind, RecordColumn, RecordColumnAlignment, RecordRow,
    RecordSortDirection, RecordStatusTone, RecordsTable, RecordsTableEvent,
};

#[test]
fn record_identity_and_cell_lookup_do_not_depend_on_visible_labels() {
    let primary = RecordColumn::new("primary-status", "Status").sortable(true);
    let secondary = RecordColumn::new("secondary-status", "Status").sortable(true);
    let row = RecordRow::new("supplier-42", "Aurora Scoops").cells([
        RecordCell::new("secondary-status", "Verified"),
        RecordCell::new("primary-status", "Active"),
    ]);

    assert_eq!(primary.id(), "primary-status");
    assert_eq!(secondary.id(), "secondary-status");
    assert_eq!(primary.label(), secondary.label());
    assert_eq!(row.id(), "supplier-42");
    assert_eq!(row.label(), "Aurora Scoops");
    assert_eq!(
        row.cell("primary-status").map(RecordCell::value),
        Some("Active")
    );
    assert_eq!(
        row.cell("secondary-status").map(RecordCell::value),
        Some("Verified")
    );
    assert!(row.cell("missing").is_none());
    assert_ne!(
        RecordSortDirection::Ascending,
        RecordSortDirection::Descending
    );
}

#[test]
fn records_specific_column_cell_and_disabled_policies_are_typed() {
    let column = RecordColumn::new("strength", "Connection strength")
        .width(px(220.))
        .alignment(RecordColumnAlignment::Right)
        .fixed(true)
        .description("Relationship health from recent communication");
    let tags = RecordCell::tags("categories", ["B2B", "Gelato", "Wholesale"]);
    let status = RecordCell::status("strength", "Very strong", RecordStatusTone::Positive);
    let disabled = RecordRow::new("offline", "Offline supplier").disabled(true);

    assert_eq!(column.configured_width(), Some(px(220.)));
    assert_eq!(column.column_alignment(), RecordColumnAlignment::Right);
    assert!(column.is_fixed());
    assert_eq!(
        column.accessible_description(),
        Some("Relationship health from recent communication")
    );
    assert_eq!(tags.kind(), RecordCellKind::Tags);
    assert_eq!(tags.value(), "B2B, Gelato, Wholesale");
    assert_eq!(status.kind(), RecordCellKind::Status);
    assert_eq!(status.status_tone(), Some(RecordStatusTone::Positive));
    assert!(disabled.is_disabled());
}

#[gpui::test]
fn controlled_selection_survives_reorder_by_row_id_and_clears_when_removed(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| {
        RecordsTable::new("suppliers", "Supplier records", window, cx)
    });
    let cx: &mut VisualTestContext = cx;

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns([RecordColumn::new("company", "Company")], window, cx);
            table.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("alpine", "Alpine Churn")
                        .cells([RecordCell::new("company", "Alpine Churn")]),
                    RecordRow::new("aurora", "Aurora Scoops")
                        .cells([RecordCell::new("company", "Aurora Scoops")]),
                ])),
                window,
                cx,
            );
            table.set_selected_row("aurora", window, cx);
        });
    });

    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("aurora"));
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("aurora", "Aurora Scoops")
                        .cells([RecordCell::new("company", "Aurora Scoops")]),
                    RecordRow::new("alpine", "Alpine Churn")
                        .cells([RecordCell::new("company", "Alpine Churn")]),
                ])),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("aurora"));
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("alpine", "Alpine Churn")
                    .cells([RecordCell::new("company", "Alpine Churn")])])),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), None);
    });
}

#[gpui::test]
fn controlled_snapshots_clear_selection_and_stale_sort_state(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| {
        RecordsTable::new("suppliers", "Supplier records", window, cx)
    });
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns(
                [RecordColumn::new("company", "Company").sortable(true)],
                window,
                cx,
            );
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("aurora", "Aurora")
                    .cells([RecordCell::new("company", "Aurora")])])),
                window,
                cx,
            );
            table.set_selected_row("aurora", window, cx);
            table.set_sort("company", Some(RecordSortDirection::Ascending), window, cx);
            table.clear_selected_row(cx);
            table.set_columns([RecordColumn::new("region", "Region")], window, cx);
        });
    });

    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), None);
        assert_eq!(table.sort(), None);
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_selected_row("aurora", window, cx);
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("aurora", "Aurora")
                    .disabled(true)
                    .cells([RecordCell::new("region", "West")])])),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| assert_eq!(table.selected_row_id(), None));
}

struct RecordsEventHarness {
    table: Entity<RecordsTable>,
    events: Rc<RefCell<Vec<RecordsTableEvent>>>,
    _subscription: Subscription,
}

impl RecordsEventHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| RecordsTable::new("suppliers", "Supplier records", window, cx));
        table.update(cx, |table, cx| {
            table.set_columns(
                [
                    RecordColumn::new("company", "Company"),
                    RecordColumn::new("strength", "Status").sortable(true),
                ],
                window,
                cx,
            );
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("aurora", "Aurora Scoops")
                    .cells([
                        RecordCell::new("company", "Aurora Scoops"),
                        RecordCell::new("strength", "Very strong"),
                    ])])),
                window,
                cx,
            );
        });
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let subscription = cx.subscribe(&table, move |_, _, event, _| {
            captured.borrow_mut().push(event.clone());
        });
        Self {
            table,
            events,
            _subscription: subscription,
        }
    }
}

impl Render for RecordsEventHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.table.clone()
    }
}

fn activate_key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("test key should parse");
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
    cx.run_until_parked();
}

#[gpui::test]
fn pointer_selection_and_sort_emit_stable_application_ids(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let row = cx
        .debug_bounds("records-row-9:suppliersaurora")
        .unwrap_or_else(|| {
            panic!(
                "the virtualized row should expose its stable ID; root={:?}, header={:?}, cell={:?}, empty={:?}",
                cx.debug_bounds("records-table-suppliers"),
                cx.debug_bounds("records-column-9:supplierscompany"),
                cx.debug_bounds("records-cell-9:suppliers6:auroracompany"),
                cx.debug_bounds("records-state-9:suppliersrecords-empty"),
            )
        });
    cx.simulate_click(row.center(), Modifiers::default());

    let sort = cx
        .debug_bounds("records-sort-9:suppliersstrength")
        .expect("the sortable header should expose its stable column ID");
    cx.simulate_click(sort.center(), Modifiers::default());

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [
            RecordsTableEvent::SelectionRequested {
                id: "suppliers".into(),
                row_id: "aurora".into(),
            },
            RecordsTableEvent::SortRequested {
                id: "suppliers".into(),
                column_id: "strength".into(),
                direction: Some(RecordSortDirection::Descending),
            },
        ]
    );

    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_sort(
                "strength",
                Some(RecordSortDirection::Descending),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    let sort = cx
        .debug_bounds("records-sort-9:suppliersstrength")
        .expect("the controlled sort header should remain reachable");
    cx.simulate_click(sort.center(), Modifiers::default());
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [RecordsTableEvent::SortRequested {
            id: "suppliers".into(),
            column_id: "strength".into(),
            direction: Some(RecordSortDirection::Ascending),
        }]
    );

    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_sort("strength", Some(RecordSortDirection::Ascending), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    let sort = cx
        .debug_bounds("records-sort-9:suppliersstrength")
        .expect("the controlled sort header should remain reachable");
    cx.simulate_click(sort.center(), Modifiers::default());
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [RecordsTableEvent::SortRequested {
            id: "suppliers".into(),
            column_id: "strength".into(),
            direction: None,
        }]
    );
}

#[gpui::test]
fn thousand_records_construct_only_a_bounded_visible_range_and_reach_the_last_id(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    let records = (0..1_000)
        .map(|index| {
            RecordRow::new(format!("supplier-{index}"), format!("Supplier {index}"))
                .cells([RecordCell::new("company", format!("Supplier {index}"))])
        })
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(Progressive::complete(records.into()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let visible = (0..1_000)
        .filter(|index| {
            let selector: &'static str =
                Box::leak(format!("records-row-9:supplierssupplier-{index}").into_boxed_str());
            cx.debug_bounds(selector).is_some()
        })
        .count();
    assert!(
        visible < 50,
        "only a bounded visible row range should be constructed, got {visible}"
    );
    assert!(
        cx.debug_bounds("records-row-9:supplierssupplier-999")
            .is_none()
    );

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.scroll_to_row("supplier-999", cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("records-row-9:supplierssupplier-999")
            .is_some()
    );
    assert!(
        cx.debug_bounds("records-row-9:supplierssupplier-0")
            .is_none()
    );
}

#[gpui::test]
fn row_anchor_survives_prepend_by_stable_id(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    let rows = (0..100)
        .map(|index| {
            RecordRow::new(format!("row-{index}"), format!("Row {index}"))
                .cells([RecordCell::new("company", format!("Row {index}"))])
        })
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(Progressive::complete(rows.clone().into()), window, cx);
            table.scroll_to_row("row-50", cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let mut prepended = (0..10)
        .map(|index| {
            RecordRow::new(format!("new-{index}"), format!("New {index}"))
                .cells([RecordCell::new("company", format!("New {index}"))])
        })
        .collect::<Vec<_>>();
    prepended.extend(rows);
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(Progressive::complete(prepended.into()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("records-row-9:suppliersrow-50").is_some(),
        "the prior stable row anchor should remain visible after prepend"
    );
}

#[gpui::test]
fn keyboard_selection_emits_the_same_stable_row_event_as_pointer(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.focus(window, cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    activate_key(cx, "down");

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [RecordsTableEvent::SelectionRequested {
            id: "suppliers".into(),
            row_id: "aurora".into(),
        }]
    );
}

#[gpui::test]
fn keyboard_navigation_skips_disabled_rows_by_stable_identity(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("first", "First").cells([RecordCell::new("company", "First")]),
                    RecordRow::new("offline", "Offline")
                        .disabled(true)
                        .cells([RecordCell::new("company", "Offline")]),
                    RecordRow::new("third", "Third").cells([RecordCell::new("company", "Third")]),
                ])),
                window,
                cx,
            );
            table.set_selected_row("first", window, cx);
            table.focus(window, cx);
        });
        window.draw(cx).clear(cx);
    });
    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());

    activate_key(cx, "down");

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [RecordsTableEvent::SelectionRequested {
            id: "suppliers".into(),
            row_id: "third".into(),
        }]
    );
}

#[gpui::test]
fn enter_activates_the_consumer_controlled_selected_row(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_selected_row("aurora", window, cx);
            table.focus(window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    activate_key(cx, "enter");

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [RecordsTableEvent::ActivationRequested {
            id: "suppliers".into(),
            row_id: "aurora".into(),
        }]
    );
}

#[gpui::test]
fn named_pointer_activation_control_emits_the_same_stable_row_event(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let activation = cx
        .debug_bounds("records-activate-9:suppliersaurora")
        .expect("the named activation control should be rendered");
    cx.simulate_click(activation.center(), Modifiers::default());

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [RecordsTableEvent::ActivationRequested {
            id: "suppliers".into(),
            row_id: "aurora".into(),
        }]
    );
}

#[gpui::test]
fn wide_records_construct_only_visible_cells_and_reach_the_last_column_id(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    let columns = (0..100)
        .map(|index| RecordColumn::new(format!("column-{index}"), format!("Column {index}")))
        .collect::<Vec<_>>();
    let cells = (0..100)
        .map(|index| RecordCell::new(format!("column-{index}"), format!("Value {index}")))
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns(columns, window, cx);
            table.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("supplier", "Supplier").cells(cells)
                ])),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let visible = (0..100)
        .filter(|index| {
            let selector: &'static str = Box::leak(
                format!("records-cell-9:suppliers8:suppliercolumn-{index}").into_boxed_str(),
            );
            cx.debug_bounds(selector).is_some()
        })
        .count();
    assert!(
        visible < 30,
        "only a bounded visible column range should be constructed, got {visible}"
    );
    assert!(
        cx.debug_bounds("records-cell-9:suppliers8:suppliercolumn-99")
            .is_none()
    );

    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.scroll_to_column("column-99", cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("records-cell-9:suppliers8:suppliercolumn-99")
            .is_some()
    );
    assert!(
        cx.debug_bounds("records-cell-9:suppliers8:suppliercolumn-0")
            .is_none()
    );
}

#[gpui::test]
fn column_anchor_survives_prepend_by_stable_id(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    let columns = (0..30)
        .map(|index| {
            RecordColumn::new(format!("col-{index}"), format!("Column {index}")).fixed(index == 0)
        })
        .collect::<Vec<_>>();
    let cells = (0..30)
        .map(|index| RecordCell::new(format!("col-{index}"), format!("Value {index}")))
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns(columns.clone(), window, cx);
            table.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("supplier", "Supplier").cells(cells)
                ])),
                window,
                cx,
            );
            table.scroll_to_column("col-20", cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let anchor = (0..30)
        .find(|index| {
            let selector: &'static str =
                Box::leak(format!("records-column-9:supplierscol-{index}").into_boxed_str());
            cx.debug_bounds(selector).is_some()
        })
        .expect("a stable visible column anchor should exist");

    let mut prepended = vec![columns[0].clone()];
    prepended.extend(
        (0..5)
            .map(|index| RecordColumn::new(format!("new-{index}"), format!("New {index}")))
            .collect::<Vec<_>>(),
    );
    prepended.extend(columns.into_iter().skip(1));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.set_columns(prepended, window, cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds(Box::leak(
            format!("records-column-9:supplierscol-{anchor}").into_boxed_str(),
        ))
        .is_some(),
        "the prior stable column anchor should remain visible after prepend"
    );
}

#[gpui::test]
fn disabled_records_reject_pointer_keyboard_and_activation_intent(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(RecordsEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("offline", "Offline supplier")
                    .disabled(true)
                    .cells([RecordCell::new("company", "Offline supplier")])])),
                window,
                cx,
            );
            table.focus(window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let row = cx
        .debug_bounds("records-row-9:suppliersoffline")
        .expect("the disabled row should remain visible and readable");
    cx.simulate_click(row.center(), Modifiers::default());
    activate_key(cx, "down");
    activate_key(cx, "enter");

    assert!(
        harness.read_with(cx, |harness, _| harness.events.borrow().is_empty()),
        "disabled rows must reject every interaction path"
    );
}

/// The activation control rails at the trailing edge by default and
/// moves beside the content when a consumer chooses inline — the
/// placement half of the row-action affordance. Visibility's hover
/// reveal is opacity-only, so the control keeps its bounds (and its
/// keyboard reachability) either way.
#[gpui::test]
fn row_actions_default_to_the_trailing_edge_and_inline_is_a_choice(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| {
        RecordsTable::new("suppliers", "Supplier records", window, cx)
    });
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns(
                [RecordColumn::new("company", "Company").width_in_rems(20.)],
                window,
                cx,
            );
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("alpine", "Alpine Churn")
                    .cells([RecordCell::new("company", "Alpine Churn")])])),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });

    let end = cx
        .debug_bounds("records-activate-9:suppliersalpine")
        .expect("the activation control renders under the hover default");

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_row_action_placement(gpui_ai::prelude::RowActionPlacement::Inline, cx);
        });
        window.draw(cx).clear(cx);
    });
    let inline = cx
        .debug_bounds("records-activate-9:suppliersalpine")
        .expect("the inline control still renders");
    assert!(
        inline.left() < end.left(),
        "inline must sit beside the content, before the trailing edge: {:?} vs {:?}",
        inline.left(),
        end.left()
    );
}
