use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{
    AppContext as _, Context, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render,
    Subscription, TestAppContext, VisualTestContext, Window, px, size,
};
use gpui_ai::prelude::{
    FilterCell, FilterColumn, FilterDefinition, FilterRow, FilterSortDirection, FilterTable,
    FilterTableEvent, Progressive,
};

#[test]
fn filter_definitions_keep_stable_identity_count_and_controlled_state() {
    let filter = FilterDefinition::new("in-progress", "In Progress", 2)
        .active(true)
        .disabled(true);

    assert_eq!(filter.id(), "in-progress");
    assert_eq!(filter.label(), "In Progress");
    assert_eq!(filter.count(), 2);
    assert!(filter.is_active());
    assert!(filter.is_disabled());
}

#[gpui::test]
fn duplicate_filter_identity_is_rejected_without_replacing_controlled_state(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (table, cx) =
        cx.add_window_view(|window, cx| FilterTable::new("tasks", "Tasks", window, cx));
    let valid = [
        FilterDefinition::new("all", "All", 5).active(true),
        FilterDefinition::new("done", "Done", 2),
    ];
    cx.update(|_, cx| {
        table.update(cx, |table, cx| table.set_filters(valid.clone(), cx));
    });
    cx.update(|_, cx| {
        table.update(cx, |table, cx| {
            table.set_filters(
                [
                    FilterDefinition::new("duplicate", "First", 1),
                    FilterDefinition::new("duplicate", "Second", 2),
                ],
                cx,
            );
        });
    });

    table.read_with(cx, |table, _| assert_eq!(table.filters(), &valid));
}

#[test]
fn filter_events_carry_stable_application_identity() {
    assert_eq!(
        FilterTableEvent::FilterRequested {
            id: "task-board".into(),
            filter_id: "completed".into(),
            active: true,
        },
        FilterTableEvent::FilterRequested {
            id: "task-board".into(),
            filter_id: "completed".into(),
            active: true,
        }
    );
    assert_eq!(
        FilterTableEvent::SortRequested {
            id: "task-board".into(),
            column_id: "date".into(),
            direction: Some(FilterSortDirection::Ascending),
        },
        FilterTableEvent::SortRequested {
            id: "task-board".into(),
            column_id: "date".into(),
            direction: Some(FilterSortDirection::Ascending),
        }
    );
}

fn task_row(id: &str, name: &str, status: &str) -> FilterRow {
    FilterRow::new(id.to_owned(), name.to_owned()).cells([
        FilterCell::new("task", name.to_owned()),
        FilterCell::new("status", status.to_owned()),
    ])
}

#[gpui::test]
fn controlled_filter_selection_survives_reorder_and_clears_when_removed(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) =
        cx.add_window_view(|window, cx| FilterTable::new("task-board", "Task board", window, cx));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns([FilterColumn::new("task", "Task")], window, cx);
            table.set_rows(
                Progressive::complete(Arc::from([
                    task_row("task-a", "Restock mango", "To do"),
                    task_row("task-b", "Print menu", "In Progress"),
                ])),
                cx,
            );
            table.set_selected_row("task-b", window, cx);
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("task-b"));
    });

    cx.update(|_window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([
                    task_row("task-b", "Print menu", "In Progress"),
                    task_row("task-a", "Restock mango", "To do"),
                ])),
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("task-b"));
    });

    cx.update(|_window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([task_row("task-a", "Restock mango", "To do")])),
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| assert_eq!(table.selected_row_id(), None));
}

#[gpui::test]
fn malformed_filter_row_identity_is_rejected_without_replacing_controlled_state(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (table, cx) =
        cx.add_window_view(|window, cx| FilterTable::new("tasks", "Tasks", window, cx));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns([FilterColumn::new("task", "Task")], window, cx);
            table.set_rows(
                Progressive::complete(Arc::from([task_row("keep", "Keep", "To do")])),
                cx,
            );
            table.set_selected_row("keep", window, cx);
        });
    });

    cx.update(|_, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([
                    task_row("duplicate", "First", "To do"),
                    task_row("duplicate", "Second", "To do"),
                ])),
                cx,
            );
        });
    });
    cx.update(|_, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([FilterRow::new("bad-cells", "Bad cells").cells(
                    [
                        FilterCell::new("duplicate-cell", "First"),
                        FilterCell::new("duplicate-cell", "Second"),
                    ],
                )])),
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("keep"));
    });
}

#[gpui::test]
fn controlled_sort_rejects_invalid_columns_and_clears_when_the_column_is_removed(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (table, cx) =
        cx.add_window_view(|window, cx| FilterTable::new("tasks", "Tasks", window, cx));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns(
                [
                    FilterColumn::new("task", "Task"),
                    FilterColumn::new("status", "Status").sortable(true),
                ],
                window,
                cx,
            );
            table.set_sort("status", Some(FilterSortDirection::Ascending), window, cx);
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(
            table.sort(),
            Some(("status", FilterSortDirection::Ascending))
        );
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns([FilterColumn::new("task", "Task")], window, cx);
        });
    });
    table.read_with(cx, |table, _| assert_eq!(table.sort(), None));

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_sort("missing", Some(FilterSortDirection::Descending), window, cx);
        });
    });
    table.read_with(cx, |table, _| assert_eq!(table.sort(), None));
}

struct FilterEventHarness {
    table: Entity<FilterTable>,
    events: Rc<RefCell<Vec<FilterTableEvent>>>,
    _subscription: Subscription,
}

impl FilterEventHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| FilterTable::new("task-board", "Task board", window, cx));
        table.update(cx, |table, cx| {
            table.set_filters(
                [
                    FilterDefinition::new("all", "All", 2).active(true),
                    FilterDefinition::new("todo", "To do", 1),
                    FilterDefinition::new("blocked", "Blocked", 0).disabled(true),
                ],
                cx,
            );
            table.set_columns(
                [
                    FilterColumn::new("task", "Task"),
                    FilterColumn::new("status", "Status").sortable(true),
                ],
                window,
                cx,
            );
            table.set_rows(
                Progressive::complete(Arc::from([
                    task_row("task-a", "Restock mango", "To do"),
                    task_row("task-b", "Print menu", "In Progress"),
                ])),
                cx,
            );
            table.set_selected_row("task-a", window, cx);
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

impl Render for FilterEventHarness {
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
fn pointer_filter_activation_and_records_actions_emit_stable_typed_intent(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(FilterEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(760.), px(420.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let todo = cx
        .debug_bounds("filter-table-filter-10:task-boardtodo")
        .expect("the stable To do filter should render");
    cx.simulate_click(todo.center(), Modifiers::default());

    let blocked = cx
        .debug_bounds("filter-table-filter-10:task-boardblocked")
        .expect("the disabled filter should remain visible");
    cx.simulate_click(blocked.center(), Modifiers::default());

    let open = cx
        .debug_bounds("records-activate-39:filter-table-records-10:task-boardtabletask-a")
        .expect("the stable row activation should render");
    cx.simulate_click(open.center(), Modifiers::default());

    let sort = cx
        .debug_bounds("records-sort-39:filter-table-records-10:task-boardtablestatus")
        .expect("the stable sortable status column should render");
    cx.simulate_click(sort.center(), Modifiers::default());

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [
            FilterTableEvent::FilterRequested {
                id: "task-board".into(),
                filter_id: "todo".into(),
                active: true,
            },
            FilterTableEvent::ActivationRequested {
                id: "task-board".into(),
                row_id: "task-a".into(),
            },
            FilterTableEvent::SortRequested {
                id: "task-board".into(),
                column_id: "status".into(),
                direction: Some(FilterSortDirection::Descending),
            },
        ]
    );
    harness.read_with(cx, |harness, cx| {
        assert!(!harness.table.read(cx).filters()[1].is_active());
    });

    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.focus(window, cx));
        window.draw(cx).clear(cx);
    });
    activate_key(cx, "enter");
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [FilterTableEvent::ActivationRequested {
            id: "task-board".into(),
            row_id: "task-a".into(),
        }]
    );
}

#[gpui::test]
fn thousand_filtered_rows_construct_a_bounded_range_and_reach_the_last_id(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) =
        cx.add_window_view(|window, cx| FilterTable::new("tasks", "Filtered tasks", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(720.), px(360.)));
    let rows = (0..1_000)
        .map(|index| task_row(&format!("task-{index}"), &format!("Task {index}"), "To do"))
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_filters(
                [FilterDefinition::new("all", "All", 1_000).active(true)],
                cx,
            );
            table.set_columns([FilterColumn::new("task", "Task")], window, cx);
            table.set_rows(Progressive::complete(rows.into()), cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let visible = (0..1_000)
        .filter(|index| {
            let selector: &'static str = Box::leak(
                format!("records-row-33:filter-table-records-5:taskstabletask-{index}")
                    .into_boxed_str(),
            );
            cx.debug_bounds(selector).is_some()
        })
        .count();
    assert!(
        visible < 50,
        "only a bounded row range should render, got {visible}"
    );

    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.scroll_to_row("task-999", cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("records-row-33:filter-table-records-5:taskstabletask-999")
            .is_some()
    );
    assert!(
        cx.debug_bounds("records-row-33:filter-table-records-5:taskstabletask-0")
            .is_none()
    );
}

#[gpui::test]
fn overflowing_filter_controls_keep_the_last_stable_filter_reachable(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(FilterEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(420.), px(280.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_filters(
                (0..100).map(|index| {
                    FilterDefinition::new(format!("filter-{index}"), format!("Filter {index}"), 1)
                }),
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let viewport = cx
        .debug_bounds("filter-table-controls-10:task-boardfilters")
        .expect("the filter viewport should render");
    let last = cx
        .debug_bounds("filter-table-filter-10:task-boardfilter-99")
        .expect("the stable final filter should be constructed inside the bounded scroller");
    assert!(last.left() >= viewport.right());

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.focus_filter("filter-99", window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let last = cx
        .debug_bounds("filter-table-filter-10:task-boardfilter-99")
        .expect("the final filter should remain mounted after scrolling");
    assert!(
        last.left() < viewport.right(),
        "last={last:?}, viewport={viewport:?}"
    );
    assert!(last.center().x <= px(420.), "last={last:?}");

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_filters(
                (0..100).rev().map(|index| {
                    FilterDefinition::new(format!("filter-{index}"), format!("Filter {index}"), 1)
                }),
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let last = cx
        .debug_bounds("filter-table-filter-10:task-boardfilter-99")
        .expect("the focused filter should remain mounted after snapshot reordering");
    assert!(
        last.left() >= viewport.left() && last.right() <= viewport.right(),
        "focused filter should be revealed after its stable ID moves: last={last:?}, viewport={viewport:?}"
    );

    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());
    activate_key(cx, "enter");
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [FilterTableEvent::FilterRequested {
            id: "task-board".into(),
            filter_id: "filter-99".into(),
            active: true,
        }]
    );
}
