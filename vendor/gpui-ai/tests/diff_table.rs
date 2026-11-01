use std::{cell::RefCell, rc::Rc, sync::Arc};

use gpui::{
    AppContext as _, Context, Entity, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, Render,
    Subscription, TestAppContext, VisualTestContext, Window, px, size,
};
use gpui_ai::prelude::{
    DiffCell, DiffChangeKind, DiffColumn, DiffProposalAction, DiffProposalState, DiffRow,
    DiffSortDirection, DiffTable, DiffTableEvent, Progressive,
};

#[test]
fn diff_cells_encode_valid_before_and_after_states() {
    let added = DiffCell::added("supplier", "maple-orbit");
    let removed = DiffCell::removed("category", "Retro");
    let changed = DiffCell::changed("flavor", "Mint Chip", "Pistachio");
    let unchanged = DiffCell::unchanged("region", "Pacific");

    assert_eq!(added.change_kind(), DiffChangeKind::Added);
    assert_eq!(added.before(), None);
    assert_eq!(added.after(), Some("maple-orbit"));

    assert_eq!(removed.change_kind(), DiffChangeKind::Removed);
    assert_eq!(removed.before(), Some("Retro"));
    assert_eq!(removed.after(), None);

    assert_eq!(changed.change_kind(), DiffChangeKind::Changed);
    assert_eq!(changed.before(), Some("Mint Chip"));
    assert_eq!(changed.after(), Some("Pistachio"));

    assert_eq!(unchanged.change_kind(), DiffChangeKind::Unchanged);
    assert_eq!(unchanged.before(), Some("Pacific"));
    assert_eq!(unchanged.after(), Some("Pacific"));
}

#[test]
fn diff_rows_keep_stable_identity_and_controlled_proposal_state() {
    let first = DiffRow::new(
        "proposal-a",
        "Rename seasonal flavor",
        DiffChangeKind::Changed,
    )
    .cells([DiffCell::changed("flavor", "Mint Chip", "Pistachio")])
    .state(DiffProposalState::Pending);
    let second = DiffRow::new(
        "proposal-b",
        "Rename seasonal flavor",
        DiffChangeKind::Added,
    )
    .cells([DiffCell::added("flavor", "Saffron Swirl")])
    .state(DiffProposalState::Accepted)
    .disabled(true);

    assert_eq!(first.id(), "proposal-a");
    assert_eq!(second.id(), "proposal-b");
    assert_eq!(first.label(), second.label());
    assert_eq!(
        first.cell("flavor").map(DiffCell::change_kind),
        Some(DiffChangeKind::Changed)
    );
    assert_eq!(first.proposal_state(), DiffProposalState::Pending);
    assert_eq!(second.proposal_state(), DiffProposalState::Accepted);
    assert!(second.is_disabled());
    assert_ne!(DiffProposalAction::Accept, DiffProposalAction::Reject);
}

#[gpui::test]
fn controlled_diff_selection_survives_reorder_and_clears_when_disabled(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| {
        DiffTable::new("menu-cleanup", "Proposed menu cleanup", window, cx)
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns([DiffColumn::new("flavor", "Flavor")], window, cx);
            table.set_rows(
                Progressive::complete(Arc::from([
                    DiffRow::new("rocky-road", "Rocky Road", DiffChangeKind::Removed)
                        .cells([DiffCell::removed("flavor", "Rocky Road")]),
                    DiffRow::new("pistachio", "Pistachio", DiffChangeKind::Added)
                        .cells([DiffCell::added("flavor", "Pistachio")]),
                ])),
                window,
                cx,
            );
            table.set_selected_row("pistachio", window, cx);
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("pistachio"));
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([
                    DiffRow::new("pistachio", "Pistachio", DiffChangeKind::Added)
                        .cells([DiffCell::added("flavor", "Pistachio")]),
                    DiffRow::new("rocky-road", "Rocky Road", DiffChangeKind::Removed)
                        .cells([DiffCell::removed("flavor", "Rocky Road")]),
                ])),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("pistachio"));
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([DiffRow::new(
                    "pistachio",
                    "Pistachio",
                    DiffChangeKind::Added,
                )
                .cells([DiffCell::added("flavor", "Pistachio")])
                .disabled(true)])),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| assert_eq!(table.selected_row_id(), None));
}

#[gpui::test]
fn malformed_diff_row_identity_is_rejected_without_replacing_controlled_state(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (table, cx) = cx.add_window_view(|window, cx| DiffTable::new("diff", "Diff", window, cx));
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_columns([DiffColumn::new("name", "Name")], window, cx);
            table.set_rows(
                Progressive::complete(Arc::from([DiffRow::new(
                    "keep",
                    "Keep",
                    DiffChangeKind::Unchanged,
                )
                .cells([DiffCell::unchanged("name", "Keep")])])),
                window,
                cx,
            );
            table.set_selected_row("keep", window, cx);
        });
    });

    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([
                    DiffRow::new("duplicate", "First", DiffChangeKind::Added),
                    DiffRow::new("duplicate", "Second", DiffChangeKind::Removed),
                ])),
                window,
                cx,
            );
        });
    });
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(
                Progressive::complete(Arc::from([DiffRow::new(
                    "bad-cells",
                    "Bad cells",
                    DiffChangeKind::Changed,
                )
                .cells([
                    DiffCell::added("duplicate-cell", "First"),
                    DiffCell::removed("duplicate-cell", "Second"),
                ])])),
                window,
                cx,
            );
        });
    });
    table.read_with(cx, |table, _| {
        assert_eq!(table.selected_row_id(), Some("keep"));
    });
}

struct DiffEventHarness {
    table: Entity<DiffTable>,
    events: Rc<RefCell<Vec<DiffTableEvent>>>,
    _subscription: Subscription,
}

impl DiffEventHarness {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table =
            cx.new(|cx| DiffTable::new("menu-cleanup", "Proposed menu cleanup", window, cx));
        table.update(cx, |table, cx| {
            table.set_columns(
                [
                    DiffColumn::new("flavor", "Flavor"),
                    DiffColumn::new("supplier", "Supplier").sortable(true),
                ],
                window,
                cx,
            );
            table.set_rows(
                Progressive::complete(Arc::from([DiffRow::new(
                    "pistachio",
                    "Pistachio",
                    DiffChangeKind::Changed,
                )
                .cells([
                    DiffCell::changed("flavor", "Mint Chip", "Pistachio"),
                    DiffCell::unchanged("supplier", "maple-orbit"),
                ])])),
                window,
                cx,
            );
            table.set_selected_row("pistachio", window, cx);
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

impl Render for DiffEventHarness {
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
fn pointer_and_keyboard_diff_actions_emit_stable_typed_intent(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(DiffEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(760.), px(420.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let accept = cx
        .debug_bounds("diff-table-accept-12:menu-cleanuppistachio")
        .expect("the selected proposal should expose its stable accept control");
    cx.simulate_click(accept.center(), Modifiers::default());
    let reject = cx
        .debug_bounds("diff-table-reject-12:menu-cleanuppistachio")
        .expect("the selected proposal should expose its stable reject control");
    cx.simulate_click(reject.center(), Modifiers::default());

    let review = cx
        .debug_bounds("records-activate-39:diff-table-records-12:menu-cleanuptablepistachio")
        .expect("the proposal should expose its stable review control");
    cx.simulate_click(review.center(), Modifiers::default());

    let sort = cx
        .debug_bounds("records-sort-39:diff-table-records-12:menu-cleanuptablesupplier")
        .expect("the sortable diff column should remain reachable");
    cx.simulate_click(sort.center(), Modifiers::default());

    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [
            DiffTableEvent::DecisionRequested {
                id: "menu-cleanup".into(),
                row_id: "pistachio".into(),
                action: DiffProposalAction::Accept,
            },
            DiffTableEvent::DecisionRequested {
                id: "menu-cleanup".into(),
                row_id: "pistachio".into(),
                action: DiffProposalAction::Reject,
            },
            DiffTableEvent::ReviewRequested {
                id: "menu-cleanup".into(),
                row_id: "pistachio".into(),
            },
            DiffTableEvent::SortRequested {
                id: "menu-cleanup".into(),
                column_id: "supplier".into(),
                direction: Some(DiffSortDirection::Descending),
            },
        ]
    );

    harness.update(cx, |harness, _| harness.events.borrow_mut().clear());
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    cx.update(|window, cx| table.update(cx, |table, cx| table.focus(window, cx)));
    activate_key(cx, "enter");
    assert_eq!(
        harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
        [DiffTableEvent::ReviewRequested {
            id: "menu-cleanup".into(),
            row_id: "pistachio".into(),
        }]
    );
}

#[gpui::test]
fn thousand_diff_rows_construct_only_a_bounded_range_and_reach_the_last_id(
    cx: &mut TestAppContext,
) {
    cx.update(gpui_ai::init);
    let (harness, cx) = cx.add_window_view(DiffEventHarness::new);
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(760.), px(360.)));
    let table = harness.read_with(cx, |harness, _| harness.table.clone());
    let rows = (0..1_000)
        .map(|index| {
            DiffRow::new(
                format!("proposal-{index}"),
                format!("Proposal {index}"),
                DiffChangeKind::Changed,
            )
            .cells([DiffCell::changed(
                "flavor",
                format!("Before {index}"),
                format!("After {index}"),
            )])
        })
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        table.update(cx, |table, cx| {
            table.set_rows(Progressive::complete(rows.into()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let visible = (0..1_000)
        .filter(|index| {
            let selector: &'static str = Box::leak(
                format!("records-row-39:diff-table-records-12:menu-cleanuptableproposal-{index}")
                    .into_boxed_str(),
            );
            cx.debug_bounds(selector).is_some()
        })
        .count();
    assert!(
        visible < 50,
        "only a bounded visible proposal range should be constructed, got {visible}"
    );
    assert!(
        cx.debug_bounds("records-row-39:diff-table-records-12:menu-cleanuptableproposal-999")
            .is_none()
    );

    cx.update(|window, cx| {
        table.update(cx, |table, cx| table.scroll_to_row("proposal-999", cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(
        cx.debug_bounds("records-row-39:diff-table-records-12:menu-cleanuptableproposal-999")
            .is_some()
    );
    assert!(
        cx.debug_bounds("records-row-39:diff-table-records-12:menu-cleanuptableproposal-0")
            .is_none()
    );
}
