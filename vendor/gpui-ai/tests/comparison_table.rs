use std::{cell::RefCell, rc::Rc};

use gpui::{
    AppContext as _, Context, Entity, InteractiveElement as _, KeyDownEvent, KeyUpEvent, Keystroke,
    Modifiers, MouseButton, ParentElement as _, Render, Styled as _, Subscription, TestAppContext,
    VisualTestContext, Window, point, px, size,
};
use gpui_ai::prelude::{
    ComparisonFeature, ComparisonItem, ComparisonItemState, ComparisonSnapshot,
    ComparisonSnapshotError, ComparisonTable, ComparisonTableEvent, ComparisonValue, Progressive,
};

#[test]
fn comparison_snapshot_uses_stable_ids_and_accepts_duplicate_visible_labels() {
    let snapshot = ComparisonSnapshot::try_new(
        [
            ComparisonItem::new("starter", "Creamery"),
            ComparisonItem::new("business", "Creamery").state(ComparisonItemState::Highlighted),
        ],
        [
            ComparisonFeature::new("price", "Monthly price").values([
                ComparisonValue::new("starter", "$12"),
                ComparisonValue::new("business", "$24"),
            ]),
            ComparisonFeature::new("support", "Priority support").values([
                ComparisonValue::included("starter", false),
                ComparisonValue::included("business", true),
            ]),
        ],
    )
    .expect("a small stable comparison should be valid");

    assert_eq!(snapshot.items()[0].id(), "starter");
    assert_eq!(snapshot.items()[1].id(), "business");
    assert_eq!(snapshot.items()[0].label(), snapshot.items()[1].label());
    assert_eq!(
        snapshot
            .feature("support")
            .and_then(|feature| feature.value("business"))
            .map(ComparisonValue::display),
        Some("Included")
    );
    assert_eq!(
        snapshot.items()[1].item_state(),
        ComparisonItemState::Highlighted
    );
}

#[test]
fn comparison_snapshot_rejects_duplicate_ids_dangling_values_and_unbounded_shapes() {
    assert_eq!(
        ComparisonSnapshot::try_new(
            [
                ComparisonItem::new("same", "First"),
                ComparisonItem::new("same", "Second"),
            ],
            [],
        ),
        Err(ComparisonSnapshotError::DuplicateItemId("same".into()))
    );

    assert_eq!(
        ComparisonSnapshot::try_new(
            [ComparisonItem::new("known", "Known")],
            [ComparisonFeature::new("feature", "Feature")
                .values([ComparisonValue::new("missing", "value")])],
        ),
        Err(ComparisonSnapshotError::UnknownItemId {
            feature_id: "feature".into(),
            item_id: "missing".into(),
        })
    );

    assert_eq!(
        ComparisonSnapshot::try_new(
            (0..13).map(|index| ComparisonItem::new(format!("item-{index}"), "Item")),
            [],
        ),
        Err(ComparisonSnapshotError::TooManyItems { maximum: 12 })
    );

    assert_eq!(
        ComparisonSnapshot::try_new(
            [ComparisonItem::new("item", "Item")],
            (0..129).map(|index| ComparisonFeature::new(format!("feature-{index}"), "Feature")),
        ),
        Err(ComparisonSnapshotError::TooManyFeatures { maximum: 128 })
    );
}

#[gpui::test]
fn controlled_item_selection_survives_reorder_and_clears_when_disabled(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx
        .add_window_view(|window, cx| ComparisonTable::new("plans", "Plan comparison", window, cx));

    let snapshot = |business_state| {
        ComparisonSnapshot::try_new(
            [
                ComparisonItem::new("starter", "Starter"),
                ComparisonItem::new("business", "Business").state(business_state),
            ],
            [ComparisonFeature::new("price", "Monthly price").values([
                ComparisonValue::new("starter", "$12"),
                ComparisonValue::new("business", "$24"),
            ])],
        )
        .expect("the fixture should stay inside the bounded contract")
    };

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_snapshot(
                Progressive::complete(snapshot(ComparisonItemState::Highlighted)),
                window,
                cx,
            );
            table.set_selected_item("business", window, cx);
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_item_id(), Some("business"));
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            let reordered = ComparisonSnapshot::try_new(
                [
                    ComparisonItem::new("business", "Business")
                        .state(ComparisonItemState::Highlighted),
                    ComparisonItem::new("starter", "Starter"),
                ],
                [ComparisonFeature::new("price", "Monthly price").values([
                    ComparisonValue::new("business", "$24"),
                    ComparisonValue::new("starter", "$12"),
                ])],
            )
            .expect("the reordered fixture should be valid");
            table.set_snapshot(Progressive::complete(reordered), window, cx);
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_item_id(), Some("business"));
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_snapshot(
                Progressive::complete(snapshot(ComparisonItemState::Disabled)),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_item_id(), None);
    });
}

struct ComparisonEventHarness {
    table: Entity<ComparisonTable>,
    events: Rc<RefCell<Vec<ComparisonTableEvent>>>,
    _subscription: Subscription,
}

impl ComparisonEventHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| ComparisonTable::new("plans", "Plan comparison", window, cx));
        table.update(cx, |table, cx| {
            let snapshot = ComparisonSnapshot::try_new(
                [
                    ComparisonItem::new("starter", "Starter"),
                    ComparisonItem::new("business", "Business")
                        .state(ComparisonItemState::Highlighted),
                    ComparisonItem::new("legacy", "Legacy").state(ComparisonItemState::Disabled),
                ],
                [ComparisonFeature::new("price", "Monthly price").values([
                    ComparisonValue::new("starter", "$12"),
                    ComparisonValue::new("business", "$24"),
                    ComparisonValue::new("legacy", "$8"),
                ])],
            )
            .expect("the interaction fixture should be valid");
            table.set_snapshot(Progressive::complete(snapshot), window, cx);
            table.set_selected_item("starter", window, cx);
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

impl Render for ComparisonEventHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        self.table.clone()
    }
}

struct ComparisonSelectionHarness {
    table: Entity<ComparisonTable>,
}

impl ComparisonSelectionHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| {
            let mut table = ComparisonTable::new("literal", "Literal values", window, cx);
            let literal = "# plan *literal* <value> [x] | + - !";
            let snapshot = ComparisonSnapshot::try_new(
                [ComparisonItem::new("plan", "Plan")],
                [ComparisonFeature::new("syntax", "Syntax")
                    .values([ComparisonValue::new("plan", format!("{literal} {literal}"))])],
            )
            .expect("the literal fixture should be valid");
            table.set_snapshot(Progressive::complete(snapshot), window, cx);
            table
        });
        Self { table }
    }
}

impl Render for ComparisonSelectionHarness {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        gpui::div()
            .id("comparison-selection-root")
            .relative()
            .size_full()
            .child(gpui_base::TextSelectionLayer)
            .child(self.table.clone())
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
fn pointer_and_keyboard_selection_emit_stable_item_identity(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(ComparisonEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(560.), px(320.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let business = cx
        .debug_bounds("comparison-item-control:plans:business")
        .expect("the stable comparison item control should render");
    cx.simulate_click(business.center(), Modifiers::default());
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [ComparisonTableEvent::SelectionRequested {
            id: "plans".into(),
            item_id: "business".into(),
        }]
    );

    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.focus_item("starter", window, cx);
        });
    });
    activate_key(cx, "right");
    activate_key(cx, "enter");
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [ComparisonTableEvent::SelectionRequested {
            id: "plans".into(),
            item_id: "business".into(),
        }]
    );
}

#[gpui::test]
fn long_bounded_comparison_keeps_the_last_item_horizontally_reachable(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| {
        ComparisonTable::new("wide-plans", "Wide plan comparison", window, cx)
    });
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(420.), px(280.)));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            let items = (0..12)
                .map(|index| {
                    ComparisonItem::new(
                        format!("plan-{index}"),
                        format!("Plan {index} with a deliberately long label"),
                    )
                })
                .collect::<Vec<_>>();
            let values = (0..12)
                .map(|index| ComparisonValue::new(format!("plan-{index}"), format!("${index}")))
                .collect::<Vec<_>>();
            let snapshot = ComparisonSnapshot::try_new(
                items,
                [ComparisonFeature::new("price", "Monthly price").values(values)],
            )
            .expect("the maximum bounded shape should be valid");
            table.set_snapshot(Progressive::complete(snapshot), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let viewport = cx
        .debug_bounds("comparison-table-root:wide-plans")
        .expect("the comparison viewport should render");
    let last = cx
        .debug_bounds("comparison-item-control:wide-plans:plan-11")
        .expect("the final bounded item should be constructed");
    assert!(last.left() >= viewport.right());

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.focus_item("plan-11", window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let last = cx
        .debug_bounds("comparison-item-control:wide-plans:plan-11")
        .expect("the final bounded item should remain mounted");
    assert!(
        last.left() >= viewport.left() && last.right() <= viewport.right(),
        "last={last:?}, viewport={viewport:?}"
    );
    let header = cx
        .debug_bounds("comparison-item-header:wide-plans:plan-11")
        .expect("the final header should remain mounted");
    let cell = cx
        .debug_bounds("comparison-cell:wide-plans:price:plan-11")
        .expect("the final value cell should remain mounted");
    assert!(
        (header.left() - cell.left()).abs() <= px(1.)
            && (header.right() - cell.right()).abs() <= px(1.),
        "header={header:?}, cell={cell:?}"
    );
    assert!(last.left() >= header.left() && last.right() <= header.right());
}

#[gpui::test]
fn maximum_feature_snapshot_keeps_the_final_stable_feature_vertically_reachable(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| {
        ComparisonTable::new("tall-plans", "Tall plan comparison", window, cx)
    });
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(420.), px(280.)));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            let snapshot = ComparisonSnapshot::try_new(
                [ComparisonItem::new("plan", "Plan")],
                (0..128).map(|index| {
                    ComparisonFeature::new(
                        format!("feature-{index}"),
                        if index == 64 {
                            "Feature 64 with a deliberately wrapping intermediate label".to_owned()
                        } else {
                            format!("Feature {index}")
                        },
                    )
                    .description(
                        "Selectable supporting detail that makes every feature row variable height",
                    )
                    .values([ComparisonValue::new("plan", format!("Value {index}"))])
                }),
            )
            .expect("the maximum bounded feature shape should be valid");
            table.set_snapshot(Progressive::complete(snapshot), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let viewport = cx
        .debug_bounds("comparison-table-root:tall-plans")
        .expect("the tall comparison viewport should render");
    // Reachability, not mounting: feature rows are virtualized, so a feature
    // 127 rows below the fold must cost nothing until the reader goes there.
    assert!(
        cx.debug_bounds("comparison-feature:tall-plans:feature-127")
            .is_none(),
        "a feature far below the viewport must not be constructed"
    );
    let first_row = cx
        .debug_bounds("comparison-feature:tall-plans:feature-0")
        .expect("the first feature should be on screen");
    assert!(
        first_row.top() < viewport.bottom() && first_row.bottom() > viewport.top(),
        "first_row={first_row:?}, viewport={viewport:?}"
    );

    // The wrapping label is the row-measurement stressor: it is taller than
    // its neighbours, so a list that measured rows uniformly would place it
    // wrong.
    table.update(cx, |table, cx| {
        table.scroll_to_feature("feature-64", cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let intermediate = cx
        .debug_bounds("comparison-feature:tall-plans:feature-64")
        .expect("the intermediate described feature should be constructed on demand");
    assert!(
        intermediate.center().y >= viewport.top() && intermediate.center().y <= viewport.bottom(),
        "intermediate={intermediate:?}, viewport={viewport:?}"
    );
    assert!(
        intermediate.size.height > first_row.size.height,
        "the wrapping label must measure taller than a plain row: \
         intermediate={intermediate:?}, first_row={first_row:?}"
    );

    table.update(cx, |table, cx| {
        table.scroll_to_feature("feature-127", cx);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let final_row = cx
        .debug_bounds("comparison-feature:tall-plans:feature-127")
        .expect("the final feature should be constructed once it is revealed");
    assert!(
        final_row.top() >= viewport.top() - px(1.)
            && final_row.top() < viewport.bottom()
            && final_row.bottom() > viewport.top(),
        "final_row={final_row:?}, viewport={viewport:?}"
    );
    // Revealing the end of a long comparison must not drag the whole
    // snapshot back into the frame.
    assert!(
        cx.debug_bounds("comparison-feature:tall-plans:feature-0")
            .is_none(),
        "the first feature must fall out of the window once the last is revealed"
    );
}

#[gpui::test]
fn comparison_values_export_literal_selected_text(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(ComparisonSelectionHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(520.), px(260.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let cell = cx
        .debug_bounds("comparison-cell:literal:syntax:plan")
        .expect("the literal comparison cell should render");
    let from = point(cell.left() + px(10.), cell.top() + px(12.));
    let to = point(cell.right() - px(10.), cell.bottom() - px(12.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let selected = cx.update(gpui_base::TextSelection::selected_text);
    assert!(
        selected.contains("# plan *literal* <value> [x] | + - !"),
        "{selected:?}"
    );
}
