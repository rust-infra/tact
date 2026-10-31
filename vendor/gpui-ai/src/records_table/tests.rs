//! Records-grid behaviour: direct accessibility contracts, the notification
//! decision setter by setter, atomic rejection of malformed snapshots,
//! stable-ID lookup cost, and retained reorder motion.

#![cfg(test)]

use super::delegate::{
    record_activation_button, record_cell_accessible_value, record_cell_frame, record_row_frame,
    record_sort_button, records_state_frame,
};
use super::reorder::take_row_reorder_sample_writes;
use super::stable_id::take_stable_id_visits;
use super::*;
use gpui::{
    Element as _, RenderOnce as _, TestAppContext, VisualTestContext, accesskit, canvas, px, size,
};
use gpui_component::Theme;
use gpui_component::table::TableDelegate;
use std::{
    cell::Cell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Zooms the way the shell does: the theme carries the base type size and
/// `Root` hands it to the window every frame.
///
/// Two draws, because the table notices the new rem while rendering and
/// reacts afterwards — the first draw is where it sees the change, the
/// second lays out the widths it resolved.
fn zoom_to(cx: &mut VisualTestContext, font_size: f32) {
    cx.update(|window, cx| {
        Theme::global_mut(cx).font_size = px(font_size);
        window.set_rem_size(Theme::global(cx).font_size);
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[test]
fn table_row_and_cell_builders_expose_direct_accesskit_contracts() {
    let table = records_table_frame("suppliers".into(), "Supplier records".into()).into_element();
    let mut table_node = accesskit::Node::new(Role::Unknown);
    table.write_a11y_info(&mut table_node);
    assert_eq!(table.a11y_role(), Some(Role::Table));
    assert_eq!(table_node.label(), Some("Supplier records"));

    let row = RecordRow::new("aurora", "Aurora Scoops");
    let row = record_row_frame("suppliers", &row, true).into_element();
    let mut row_node = accesskit::Node::new(Role::Unknown);
    row.write_a11y_info(&mut row_node);
    assert_eq!(row.a11y_role(), Some(Role::Row));
    assert_eq!(row_node.label(), Some("Aurora Scoops"));
    assert_eq!(row_node.is_selected(), Some(true));

    let cell = record_cell_frame("aurora-strength", "Very strong".into()).into_element();
    let mut cell_node = accesskit::Node::new(Role::Unknown);
    cell.write_a11y_info(&mut cell_node);
    assert_eq!(cell.a11y_role(), Some(Role::Cell));
    assert_eq!(cell_node.label(), Some("Very strong"));
    assert_eq!(cell_node.value(), Some("Very strong"));
}

#[test]
fn missing_first_column_value_names_the_synthetic_activation_cell() {
    let row = RecordRow::new("pistachio", "Pistachio proposal");
    let value = record_cell_accessible_value(None, Some(&row), 0, "Review");
    assert_eq!(value, "Review Pistachio proposal");

    let cell = record_cell_frame("pistachio-review", value).into_element();
    let mut cell_node = accesskit::Node::new(Role::Unknown);
    cell.write_a11y_info(&mut cell_node);
    assert_eq!(cell.a11y_role(), Some(Role::Cell));
    assert_eq!(cell_node.label(), Some("Review Pistachio proposal"));
    assert_eq!(cell_node.value(), Some("Review Pistachio proposal"));
}

/// A table with a footer draws one, below the rows and inside the frame.
///
/// The third part of a table, and the reason a frame taller than its rows
/// stops reading as rows that were cut off: the space above a bottom that says
/// something is a body with room left in it. Asserted by position rather than
/// existence, because a footer drawn above the rows would be a header.
#[gpui::test]
fn a_footer_sits_below_the_rows(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("bottom", "Bottom", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(210.)));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(
                Progressive::complete(Arc::from([RecordRow::new("aurora", "Aurora Scoops")])),
                window,
                cx,
            );
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();

    assert!(
        cx.debug_bounds("records-table-footer-bottom").is_none(),
        "a table given no footer draws none"
    );

    cx.update(|_, cx| {
        records.update(cx, |records, cx| {
            records.set_footer(Some("1 supplier".into()), cx);
        });
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let frame = cx
        .debug_bounds("records-table-bottom")
        .expect("the table renders");
    let footer = cx
        .debug_bounds("records-table-footer-bottom")
        .expect("the footer renders");
    assert!(
        footer.size.height > px(0.),
        "the footer has a height: {footer:?}"
    );
    assert!(
        footer.top() > frame.top(),
        "the footer is at the bottom of the table, not the top: footer {footer:?}, frame {frame:?}"
    );
    assert!(
        footer.bottom() <= frame.bottom(),
        "the footer is inside the frame: footer {footer:?}, frame {frame:?}"
    );
}

#[gpui::test]
fn visible_row_reorder_state_is_bounded_and_reduced_motion_snaps(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("motion", "Motion", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let make_rows = || {
        (0..1_000)
            .map(|index| {
                RecordRow::new(format!("row-{index}"), format!("Row {index}"))
                    .cells([RecordCell::new("name", format!("Row {index}"))])
            })
            .collect::<Vec<_>>()
    };
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(Progressive::complete(make_rows().into()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let mut reordered = make_rows();
    reordered.swap(1, 2);
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::complete(reordered.into()), window, cx);
        });
    });
    let moving = records.read_with(cx, |records, cx| {
        records
            .table
            .read(cx)
            .delegate()
            .row_reorder
            .animating_len()
    });
    assert!(moving > 0);
    assert!(
        moving < 64,
        "only visible stable rows should retain motion state, got {}",
        moving
    );
    cx.update(|_, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(None, cx);
        });
    });
    assert_eq!(
        records.read_with(cx, |records, cx| records.animating_row_count(cx)),
        0,
        "disabling reorder must retire channels that can no longer settle"
    );
    cx.update(|_, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
        });
    });

    cx.update(|window, cx| {
        records.update(cx, |records, cx| records.scroll_to_row("row-990", cx));
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    records.read_with(cx, |records, cx| {
        let delegate = records.table.read(cx).delegate();
        assert!(
            delegate.row_reorder.retains_only_visible(),
            "virtualized rows must not retain spring motion or sampled offsets"
        );
        assert!(!delegate.row_reorder.contains_motion("row-1"));
        assert!(!delegate.row_reorder.contains_motion("row-2"));
    });

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::complete(make_rows().into()), window, cx);
            records.scroll_to_row("row-990", cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let filtered = make_rows()
        .into_iter()
        .enumerate()
        .filter_map(|(index, row)| (index % 3 == 2).then_some(row))
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::complete(filtered.into()), window, cx);
        });
    });
    let scrolled_offsets = records.read_with(cx, |records, cx| {
        records
            .table
            .read(cx)
            .delegate()
            .row_reorder
            .animating_len()
    });
    assert!(
        scrolled_offsets > 0,
        "a filtered snapshot should animate retained rows in the post-filter anchored viewport"
    );
    assert!(scrolled_offsets < 64);

    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::complete(make_rows().into()), window, cx);
        });
    });
    records.read_with(cx, |records, cx| {
        assert!(
            records.table.read(cx).delegate().row_reorder.is_empty(),
            "reduced motion should retain no animated offsets"
        );
    });
}

#[gpui::test]
fn visible_membership_is_reported_when_reorder_motion_is_disabled(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("visible", "Visible", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let rows = (0..100)
        .map(|index| {
            RecordRow::new(format!("row-{index}"), format!("Row {index}"))
                .cells([RecordCell::new("name", format!("Row {index}"))])
        })
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(Progressive::complete(rows.into()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let visible = records.read_with(cx, |records, cx| records.visible_row_count(cx));
    assert!(visible > 0, "the painted window should report visible rows");
    assert!(visible < 100, "visibility must remain a virtualized window");
    assert_eq!(
        records.read_with(cx, |records, cx| records.animating_row_count(cx)),
        0
    );
}

/// A hundred thousand rows is the size the plan asks the index to survive.
///
/// The viewport sits deep in the snapshot on purpose: a scan for a row that
/// happens to be near the top is cheap, and measuring there would flatter
/// the scan rather than describe what a reader who scrolled actually pays.
///
/// Both phases are measured, because they answer different questions. The
/// snapshot phase says whether acceptance walks the snapshot once per
/// visible row on top of the pass that validates it. The command phase says
/// whether a lookup arriving between snapshots — the case an
/// acceptance-scoped temporary map cannot serve — still walks every row.
#[gpui::test]
fn stable_id_lookups_do_not_scale_with_a_hundred_thousand_records(cx: &mut TestAppContext) {
    const ROWS: usize = 100_000;
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("scale", "Scale", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let rows = (0..ROWS)
        .map(|index| {
            RecordRow::new(format!("row-{index}"), format!("Row {index}"))
                .cells([RecordCell::new("name", format!("Row {index}"))])
        })
        .collect::<Vec<_>>();
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(Progressive::complete(rows.clone().into()), window, cx);
            records.scroll_to_row("row-90000", cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let mut reordered = rows.clone();
    reordered.swap(90_001, 90_002);
    take_stable_id_visits();
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::complete(reordered.into()), window, cx);
        });
    });
    let snapshot_visits = take_stable_id_visits();
    assert!(
        snapshot_visits <= ROWS.saturating_add(1_000),
        "accepting a snapshot should index it once, not once per lookup: {snapshot_visits}"
    );

    take_stable_id_visits();
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.scroll_to_row("row-99999", cx);
            records.set_selected_row("row-99999", window, cx);
            records.scroll_to_column("name", cx);
        });
    });
    let command_visits = take_stable_id_visits();
    assert!(
        command_visits <= 32,
        "a command outside acceptance should look its ID up, not scan: {command_visits}"
    );
}

/// The notification decision, setter by setter.
///
/// A value only the table draws is notified once, to the table: this
/// entity renders that table, so the window is invalidated either way, and
/// a second notification would wake application observers about a value
/// the application already owns. The record snapshot is the exception —
/// this entity draws the progress and failure banner from it — so it stays
/// paired, which the banner assertion below holds in place.
#[gpui::test]
fn setters_notify_this_entity_only_when_its_own_render_reads_the_value(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("notify", "Notify", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let table = records.read_with(cx, |records, _| records.table.clone());
    let rows: Arc<[RecordRow]> = Arc::from([
        RecordRow::new("first", "First").cells([RecordCell::new("name", "First")]),
        RecordRow::new("second", "Second").cells([RecordCell::new("name", "Second")]),
    ]);
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(
                [RecordColumn::new("name", "Name").sortable(true)],
                window,
                cx,
            );
            records.set_records(Progressive::complete(rows.clone()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();

    let owner_notifications = Rc::new(Cell::new(0usize));
    let table_notifications = Rc::new(Cell::new(0usize));
    let _owner_observation = {
        let counted = owner_notifications.clone();
        cx.update(|_, cx| cx.observe(&records, move |_, _| counted.set(counted.get() + 1)))
    };
    let _table_observation = {
        let counted = table_notifications.clone();
        cx.update(|_, cx| cx.observe(&table, move |_, _| counted.set(counted.get() + 1)))
    };
    let mut observed: Vec<(&str, usize, bool)> = Vec::new();
    let mut note = |cx: &mut VisualTestContext, setter: &'static str| {
        cx.run_until_parked();
        observed.push((
            setter,
            owner_notifications.replace(0),
            table_notifications.replace(0) > 0,
        ));
    };

    cx.update(|_, cx| {
        records.update(cx, |records, cx| records.set_activation_label("Review", cx));
    });
    note(cx, "set_activation_label");

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(
                [
                    RecordColumn::new("name", "Name").sortable(true),
                    RecordColumn::new("status", "Status"),
                ],
                window,
                cx,
            );
        });
    });
    note(cx, "set_columns");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("records-column-6:notifystatus").is_some(),
        "the table's own notification must be enough to draw a column this entity never renders"
    );

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_sort("name", Some(RecordSortDirection::Descending), window, cx);
        });
    });
    note(cx, "set_sort");

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_selected_row("second", window, cx);
        });
    });
    note(cx, "set_selected_row");
    assert_eq!(
        records.read_with(cx, |records, cx| records.table.read(cx).selected_row()),
        Some(1),
        "the selection reached the table without notifying this entity"
    );

    cx.update(|_, cx| {
        records.update(cx, |records, cx| records.clear_selected_row(cx));
    });
    note(cx, "clear_selected_row");

    cx.update(|_, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
        });
    });
    note(cx, "set_row_reorder_response");

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::running(rows.clone()), window, cx);
        });
    });
    note(cx, "set_records");
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        cx.debug_bounds("records-state-6:notifyrecords-loading")
            .is_some(),
        "this entity draws the progress banner, so a snapshot must notify it too"
    );

    assert_eq!(
        observed,
        vec![
            ("set_activation_label", 0, true),
            ("set_columns", 0, true),
            ("set_sort", 0, true),
            ("set_selected_row", 0, true),
            ("clear_selected_row", 0, true),
            ("set_row_reorder_response", 0, true),
            ("set_records", 1, true),
        ],
        "setter, notifications of this entity, and whether the table was notified"
    );
}

#[test]
fn column_width_builders_carry_their_own_unit() {
    let pixels = RecordColumn::new("pixel", "Pixel").width(px(220.));
    let scaled = RecordColumn::new("scaled", "Scaled").width_in_rems(12.);

    assert_eq!(pixels.configured_width(), Some(px(220.)));
    assert_eq!(
        scaled.configured_width(),
        None,
        "a rem width has no pixel value until a window resolves it"
    );
    assert_eq!(
        RecordColumn::new("scaled", "Scaled")
            .width_in_rems(12.)
            .width(px(220.))
            .configured_width(),
        Some(px(220.)),
        "the later width call wins"
    );
}

/// A named row height is the height the rows are drawn at.
///
/// The table's own rhythm comes from the size policy; an application that has
/// measured its content may say the number itself, and clearing it hands the
/// rhythm back rather than freezing whatever was last set.
#[gpui::test]
fn a_named_row_height_is_the_height_rows_are_drawn_at(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("tall", "Tall", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(400.)));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("only", "Only").cells([RecordCell::new("name", "Only")])
                ])),
                window,
                cx,
            );
        });
    });
    let draw = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    };
    let row_height = |cx: &mut VisualTestContext| {
        cx.debug_bounds("records-row-4:tallonly")
            .expect("the row should paint")
            .size
            .height
    };

    draw(cx);
    let policy_height = row_height(cx);

    cx.update(|_, cx| {
        records.update(cx, |records, cx| {
            records.set_row_height(Some(px(56.)), cx);
        });
    });
    draw(cx);
    assert_eq!(row_height(cx), px(56.), "the named height is what rows get");

    cx.update(|_, cx| {
        records.update(cx, |records, cx| records.set_row_height(None, cx));
    });
    draw(cx);
    assert_eq!(
        row_height(cx),
        policy_height,
        "clearing it returns the rows to the table's own rhythm"
    );
}

/// The division itself, without a window: every rule the fit mode has, read
/// off one function.
#[test]
fn columns_divide_the_surplus_and_never_shrink() {
    let base = |width| RecordColumn::new("c", "C").width(px(width));
    let columns = [base(100.), base(200.)];

    assert_eq!(
        &*resolve_column_widths(&columns, ColumnFit::Intrinsic, Some(px(900.)), px(16.)),
        &[px(100.), px(200.)],
        "intrinsic columns keep exactly what they asked for"
    );
    assert_eq!(
        &*resolve_column_widths(&columns, ColumnFit::Fill, None, px(16.)),
        &[px(100.), px(200.)],
        "a table that has not been measured yet has no surplus to divide"
    );
    assert_eq!(
        &*resolve_column_widths(&columns, ColumnFit::Fill, Some(px(150.)), px(16.)),
        &[px(100.), px(200.)],
        "a table narrower than its columns scrolls; it never shrinks them"
    );
    assert_eq!(
        &*resolve_column_widths(&columns, ColumnFit::Fill, Some(px(600.)), px(16.)),
        &[px(200.), px(400.)],
        "with nobody asking, the surplus goes round in proportion to the bases"
    );

    let asked = [base(100.), base(200.).fill(1.)];
    assert_eq!(
        &*resolve_column_widths(&asked, ColumnFit::Fill, Some(px(600.)), px(16.)),
        &[px(100.), px(500.)],
        "the moment one column asks, it takes the surplus and the rest stay exact"
    );

    let shares = [base(100.).fill(1.), base(100.).fill(3.)];
    assert_eq!(
        &*resolve_column_widths(&shares, ColumnFit::Fill, Some(px(600.)), px(16.)),
        &[px(200.), px(400.)],
        "three shares against one takes three quarters of the surplus"
    );

    let unset = [RecordColumn::new("a", "A"), RecordColumn::new("b", "B")];
    assert_eq!(
        &*resolve_column_widths(&unset, ColumnFit::Fill, Some(px(600.)), px(16.)),
        &[px(300.), px(300.)],
        "a column that named no width still divides the table"
    );
}

/// A filling table ends where its frame ends, and follows the frame when it
/// moves.
///
/// The width the grid was given is what a column divides, so this measures the
/// painted headers rather than the configured widths: the point is that the
/// last column reaches the far edge, not that an arithmetic returned a number.
#[gpui::test]
fn a_filling_table_reaches_its_own_edge(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("fills", "Fills", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(900.), px(300.)));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(
                [
                    RecordColumn::new("name", "Name").width(px(120.)),
                    RecordColumn::new("notes", "Notes").width(px(120.)).fill(1.),
                ],
                window,
                cx,
            );
            records.set_records(
                Progressive::complete(Arc::from([RecordRow::new("only", "Only").cells([
                    RecordCell::new("name", "Only"),
                    RecordCell::new("notes", "Notes"),
                ])])),
                window,
                cx,
            );
        });
    });
    let draw = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
    };
    // The widths the grid consumes, plus the box it was measured in: a table
    // fills when those add up.
    // The widths the grid consumes, and the room the columns had to divide:
    // the box the grid was given, less the lane the overlaid bar is drawn in.
    let columns = |cx: &mut VisualTestContext| {
        records.read_with(cx, |records, cx| {
            let delegate = records.table.read(cx).delegate();
            (
                delegate.column(0, cx).width,
                delegate.column(1, cx).width,
                records.measured_width.expect("the grid should be measured")
                    - crate::scrolling::control_lane(cx),
            )
        })
    };

    draw(cx);
    let (exact, filling, available) = columns(cx);
    assert_eq!(
        exact,
        px(120.),
        "a column nobody asked to grow keeps the width it asked for"
    );
    assert_eq!(
        exact + filling,
        available,
        "the columns divide the whole of the room they were given, and no more:          a column drawn under the scrollbar is a column with a bar over its text"
    );

    let wide = filling;
    cx.simulate_resize(size(px(600.), px(300.)));
    draw(cx);
    let (exact, filling, available) = columns(cx);
    assert_eq!(exact, px(120.), "the exact column does not move");
    assert!(
        filling < wide,
        "a narrower frame gives the filling column less: {filling:?} was {wide:?}"
    );
    assert_eq!(
        exact + filling,
        available,
        "and it still divides the whole of it"
    );
}

/// A rem-scaled width follows the reader's type scale and a pixel width
/// does not, in one table, through the widths the grid actually consumes
/// and the header it actually paints.
///
/// Measured with the columns intrinsic, because a table that divides its width
/// adds the same surplus to both and the question here is what each one asked
/// for, not what it was given on top.
#[gpui::test]
fn rem_column_widths_resolve_against_the_readers_type_size(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("widths", "Widths", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(900.), px(300.)));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_column_fit(ColumnFit::Intrinsic, cx);
            records.set_columns(
                [
                    RecordColumn::new("pixel", "Pixel").width(px(220.)),
                    RecordColumn::new("scaled", "Scaled").width_in_rems(12.),
                ],
                window,
                cx,
            );
            records.set_records(
                Progressive::complete(Arc::from([RecordRow::new("only", "Only").cells([
                    RecordCell::new("pixel", "Pixel"),
                    RecordCell::new("scaled", "Scaled"),
                ])])),
                window,
                cx,
            );
        });
    });
    let resolved_widths = |cx: &mut VisualTestContext| {
        records.read_with(cx, |records, cx| {
            let delegate = records.table.read(cx).delegate();
            (delegate.column(0, cx).width, delegate.column(1, cx).width)
        })
    };
    let painted_width = |cx: &mut VisualTestContext, selector: &'static str| {
        cx.debug_bounds(selector)
            .expect("a configured column should paint a header")
            .size
            .width
    };

    zoom_to(cx, 16.);
    assert_eq!(resolved_widths(cx), (px(220.), px(192.)));
    let pixel_at_base = painted_width(cx, "records-column-6:widthspixel");
    let scaled_at_base = painted_width(cx, "records-column-6:widthsscaled");

    zoom_to(cx, 24.);
    assert_eq!(
        resolved_widths(cx),
        (px(220.), px(288.)),
        "a rem width follows the reader's type size; a pixel width stays put"
    );
    assert_eq!(
        painted_width(cx, "records-column-6:widthspixel"),
        pixel_at_base,
        "the pixel column must not move when the reader zooms"
    );
    assert!(
        painted_width(cx, "records-column-6:widthsscaled") > scaled_at_base,
        "the zoom must reach the painted header, not just the resolved value"
    );

    zoom_to(cx, 16.);
    assert_eq!(
        resolved_widths(cx),
        (px(220.), px(192.)),
        "zooming back resolves back"
    );
}

#[gpui::test]
fn reorder_reversal_carries_the_row_instead_of_restarting_it(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("carry", "Carry", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let make_rows = |order: &[usize]| {
        order
            .iter()
            .map(|row_ix| {
                RecordRow::new(format!("row-{row_ix}"), format!("Row {row_ix}"))
                    .cells([RecordCell::new("name", format!("Row {row_ix}"))])
            })
            .collect::<Vec<_>>()
    };
    let forward: Vec<usize> = (0..12).collect();
    let mut swapped = forward.clone();
    swapped.swap(1, 2);
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(
                Progressive::complete(make_rows(&forward).into()),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let offset_of = |cx: &mut VisualTestContext, id: &str| {
        records.read_with(cx, |records, cx| {
            records
                .table
                .read(cx)
                .delegate()
                .row_reorder
                .sampled_offset(id)
        })
    };
    let incarnation_of = |cx: &mut VisualTestContext, id: &str| {
        records.read_with(cx, |records, cx| {
            records
                .table
                .read(cx)
                .delegate()
                .row_reorder
                .incarnation(id)
        })
    };

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(
                Progressive::complete(make_rows(&swapped).into()),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    let row_height = Size::Medium.table_row_height();
    let first_frame = offset_of(cx, "row-1").expect("a moved row should carry an offset");
    // The first painted frame after a projection change already holds the
    // full displacement. A one-frame flash at the destination followed by
    // a reverse jump is the failure that reverted the MessageQueue spring
    // reorder (568b9f9); it cannot happen here because the seed and the
    // retarget land in the same render pass.
    assert_eq!(first_frame, row_height * -1.0, "no destination flash");

    cx.executor().advance_clock(Duration::from_millis(60));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let mid_flight = offset_of(cx, "row-1").expect("the row should still be travelling");
    assert!(
        mid_flight > first_frame && mid_flight < px(0.),
        "{mid_flight:?} should sit between {first_frame:?} and rest"
    );
    let incarnation = incarnation_of(cx, "row-1").expect("the channel should be live");

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(
                Progressive::complete(make_rows(&forward).into()),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    let reversed = offset_of(cx, "row-1").expect("the reversal should keep the row tracked");
    // Position continuity: the reversal repaints the row where it
    // visually was plus the one-row displacement back — no jump to either
    // endpoint of the old motion.
    assert!(
        (reversed - (mid_flight + row_height)).abs() <= px(1.),
        "{reversed:?} should continue from {mid_flight:?} + {row_height:?}"
    );
    // Velocity continuity is the channel's to keep: the same spring is
    // retargeted, never restarted.
    assert_eq!(incarnation_of(cx, "row-1"), Some(incarnation));

    for _ in 0..40 {
        cx.executor().advance_clock(Duration::from_millis(50));
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }
    records.read_with(cx, |records, cx| {
        let delegate = records.table.read(cx).delegate();
        assert!(
            delegate.row_reorder.is_empty(),
            "settled rows must own no motion state or sampled offsets"
        );
    });
    assert_eq!(
        records.read_with(cx, |records, cx| records.animating_row_count(cx)),
        0
    );
}

/// Sampling is the only reorder cost that scales with the frame rate, so
/// its map churn is counted rather than argued about.
///
/// The budget, exactly: a seeding frame retires the row's seed and stores
/// the offset it painted, a travelling frame stores only that offset, and
/// a settling frame drops both of the row's entries. A grid with nothing
/// in flight writes nothing at all, which is what keeps a quiet table off
/// these maps entirely.
#[gpui::test]
fn sampling_writes_once_per_travelling_row_each_frame(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("churn", "Churn", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let make_rows = |order: &[usize]| {
        order
            .iter()
            .map(|row_ix| {
                RecordRow::new(format!("row-{row_ix}"), format!("Row {row_ix}"))
                    .cells([RecordCell::new("name", format!("Row {row_ix}"))])
            })
            .collect::<Vec<_>>()
    };
    let forward: Vec<usize> = (0..12).collect();
    let mut swapped = forward.clone();
    swapped.swap(1, 2);
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(
                Progressive::complete(make_rows(&forward).into()),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    take_row_reorder_sample_writes();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        take_row_reorder_sample_writes(),
        0,
        "a grid with nothing in flight must not touch the reorder maps"
    );

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(
                Progressive::complete(make_rows(&swapped).into()),
                window,
                cx,
            );
        });
    });
    // Accepting a projection paints the seeding frame, which is the only
    // frame that both retires a seed and stores an offset.
    let seeding = take_row_reorder_sample_writes();
    let moving = records.read_with(cx, |records, cx| records.animating_row_count(cx));
    assert_eq!(moving, 2, "one swap moves exactly two rows");
    assert_eq!(
        seeding,
        moving.saturating_mul(2),
        "a seeding frame retires one seed and stores one offset per moving row"
    );

    let mut frames = 0usize;
    loop {
        let moving = records.read_with(cx, |records, cx| records.animating_row_count(cx));
        if moving == 0 {
            break;
        }
        assert!(frames < 200, "the springs never settled");
        take_row_reorder_sample_writes();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let written = take_row_reorder_sample_writes();
        let travelling = records.read_with(cx, |records, cx| records.animating_row_count(cx));
        let settled = moving.saturating_sub(travelling);
        assert_eq!(
            written,
            travelling.saturating_add(settled.saturating_mul(2)),
            "frame {frames}: one write per row still travelling, two per row that settled"
        );
        frames = frames.saturating_add(1);
        cx.executor().advance_clock(Duration::from_millis(50));
    }
    assert!(
        frames > 1,
        "the reorder should have taken more than one frame"
    );

    take_row_reorder_sample_writes();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        take_row_reorder_sample_writes(),
        0,
        "settled rows must stop writing"
    );
}

#[gpui::test]
fn unpainted_projection_changes_do_not_create_phantom_motion(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("unpainted", "Rows", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let make_rows = |order: &[usize]| {
        order
            .iter()
            .map(|row_ix| {
                RecordRow::new(format!("row-{row_ix}"), format!("Row {row_ix}"))
                    .cells([RecordCell::new("name", format!("Row {row_ix}"))])
            })
            .collect::<Vec<_>>()
    };
    let forward: Vec<usize> = (0..12).collect();
    let mut swapped = forward.clone();
    swapped.swap(1, 2);
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_row_reorder_response(Some(Duration::from_millis(180)), cx);
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(
                Progressive::complete(make_rows(&forward).into()),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            // Neither intermediate projection reaches the screen. The
            // second snapshot cancels the first relative to the last
            // painted frame, so there is nowhere for a row to travel.
            records.set_records(
                Progressive::complete(make_rows(&swapped).into()),
                window,
                cx,
            );
            records.set_records(
                Progressive::complete(make_rows(&forward).into()),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    records.read_with(cx, |records, cx| {
        let delegate = records.table.read(cx).delegate();
        assert!(delegate.row_reorder.is_empty());
        assert_eq!(records.animating_row_count(cx), 0);
    });
}

#[test]
fn progressive_state_builders_are_named_and_role_specific() {
    for (id, role, label) in [
        (
            "records-loading",
            Role::ProgressIndicator,
            "Loading records",
        ),
        ("records-empty", Role::Status, "No records"),
        (
            "records-error",
            Role::Alert,
            "Records unavailable: Network unavailable",
        ),
    ] {
        let element = records_state_frame("suppliers", id, role, label.into()).into_element();
        let mut node = accesskit::Node::new(Role::Unknown);
        element.write_a11y_info(&mut node);
        assert_eq!(element.a11y_role(), Some(role));
        assert_eq!(node.label(), Some(label));
    }
}

#[gpui::test]
fn inline_progressive_states_keep_direct_roles_and_names(cx: &mut TestAppContext) {
    cx.update(crate::init);
    cx.update(|cx| {
        for (id, role, label) in [
            (
                "records-loading",
                Role::ProgressIndicator,
                "Loading records",
            ),
            (
                "records-error",
                Role::Alert,
                "Records unavailable: Refresh unavailable",
            ),
        ] {
            let element =
                records_inline_state_frame("suppliers", id, role, label.into(), cx).into_element();
            let mut node = accesskit::Node::new(Role::Unknown);
            element.write_a11y_info(&mut node);
            assert_eq!(element.a11y_role(), Some(role));
            assert_eq!(node.label(), Some(label));
        }
    });
}

#[gpui::test]
fn nonempty_progressive_snapshots_keep_rows_and_lifecycle_status_visible(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("status", "Status", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    let rows: Arc<[RecordRow]> =
        Arc::from([RecordRow::new("stale", "Stale record")
            .cells([RecordCell::new("name", "Stale record")])]);

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(Progressive::running(rows.clone()), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("records-row-6:statusstale").is_some());
    assert!(
        cx.debug_bounds("records-state-6:statusrecords-loading")
            .is_some(),
        "running stale content must retain a named progress indicator"
    );

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(Progressive::failed(rows, "Refresh unavailable"), window, cx);
        });
        window.draw(cx).clear(cx);
    });
    assert!(cx.debug_bounds("records-row-6:statusstale").is_some());
    assert!(
        cx.debug_bounds("records-state-6:statusrecords-error")
            .is_some(),
        "failed stale content must retain a named alert"
    );
}

#[gpui::test]
fn upstream_clearing_its_selection_restores_the_controlled_row(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("selection", "Selection", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns([RecordColumn::new("name", "Name")], window, cx);
            records.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("first", "First").cells([RecordCell::new("name", "First")]),
                    RecordRow::new("second", "Second").cells([RecordCell::new("name", "Second")]),
                ])),
                window,
                cx,
            );
            records.set_selected_row("second", window, cx);
        });
        window.draw(cx).clear(cx);
    });

    cx.update(|_, cx| {
        records.read_with(cx, |records, cx| {
            assert_eq!(records.selected_row_id(), Some("second"));
            assert_eq!(records.table.read(cx).selected_row(), Some(1));
        });
    });

    // Escape reaches upstream's Cancel action, which clears its own
    // selection and emits TableEvent::ClearSelection.
    cx.update(|_, cx| {
        records.update(cx, |records, cx| {
            records
                .table
                .update(cx, |table, cx| table.clear_selection(cx));
        });
    });
    cx.run_until_parked();

    cx.update(|_, cx| {
        records.read_with(cx, |records, cx| {
            assert_eq!(
                records.selected_row_id(),
                Some("second"),
                "the application owns selection; upstream clearing it must not change ours"
            );
            assert_eq!(
                records.table.read(cx).selected_row(),
                Some(1),
                "the rendered row must agree with the controlled value, or Enter activates a row that looks unselected"
            );
        });
    });
}

#[gpui::test]
fn malformed_duplicate_identity_snapshots_are_rejected_atomically(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("identity", "Identity", window, cx));
    let valid_columns: Arc<[RecordColumn]> = Arc::from([
        RecordColumn::new("name", "Name"),
        RecordColumn::new("status", "Status"),
    ]);
    let valid_rows: Arc<[RecordRow]> = Arc::from([
        RecordRow::new("first", "First").cells([
            RecordCell::new("name", "First"),
            RecordCell::new("status", "Ready"),
        ]),
        RecordRow::new("second", "Second").cells([
            RecordCell::new("name", "Second"),
            RecordCell::new("status", "Waiting"),
        ]),
    ]);

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(valid_columns.iter().cloned(), window, cx);
            records.set_records(Progressive::complete(valid_rows.clone()), window, cx);
        });
    });

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(
                [
                    RecordColumn::new("duplicate", "First"),
                    RecordColumn::new("duplicate", "Second"),
                ],
                window,
                cx,
            );
            records.set_records(
                Progressive::complete(Arc::from([
                    RecordRow::new("duplicate-row", "First"),
                    RecordRow::new("duplicate-row", "Second"),
                ])),
                window,
                cx,
            );
        });
    });
    records.read_with(cx, |records, _| {
        assert_eq!(records.columns, valid_columns);
        assert_eq!(records.records.content().as_ref(), valid_rows.as_ref());
    });

    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_records(
                Progressive::complete(Arc::from([RecordRow::new("bad-cells", "Bad cells").cells(
                    [
                        RecordCell::new("duplicate-cell", "First"),
                        RecordCell::new("duplicate-cell", "Second"),
                    ],
                )])),
                window,
                cx,
            );
        });
    });
    records.read_with(cx, |records, _| {
        assert_eq!(records.records.content().as_ref(), valid_rows.as_ref());
    });
}

type CapturedControls = Arc<Mutex<Vec<(Option<Role>, accesskit::Node)>>>;

struct RecordsControlProbe {
    captured: CapturedControls,
}

impl Render for RecordsControlProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let enabled = RecordRow::new("aurora", "Aurora Scoops");
                let disabled = RecordRow::new("offline", "Offline supplier").disabled(true);
                let controls = [
                    record_sort_button(
                        "suppliers",
                        "strength",
                        "Status".into(),
                        ", descending",
                        IconName::ChevronDown,
                        cx,
                    )
                    .on_click(|_, _, _| {})
                    .render(window, cx)
                    .into_element(),
                    record_activation_button("suppliers", "Open", &enabled, window, cx)
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element(),
                    record_activation_button("suppliers", "Open", &disabled, window, cx)
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element(),
                    record_activation_button("diff", "Review", &enabled, window, cx)
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element(),
                ];
                let nodes = controls
                    .into_iter()
                    .map(|control| {
                        let role = control.a11y_role();
                        let mut node = accesskit::Node::new(Role::Unknown);
                        control.write_a11y_info(&mut node);
                        (role, node)
                    })
                    .collect();
                *captured.lock().expect("capture mutex should be available") = nodes;
            },
            |_, _, _, _| {},
        )
    }
}

#[gpui::test]
fn sort_and_activation_controls_expose_direct_accesskit_contracts(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let captured = CapturedControls::default();
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(|_, _| RecordsControlProbe { captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let nodes = result.lock().expect("capture mutex should be available");
    let (sort_role, sort) = &nodes[0];
    assert_eq!(*sort_role, Some(Role::Button));
    assert_eq!(sort.label(), Some("Sort by Status, descending"));
    assert!(sort.supports_action(accesskit::Action::Click));

    let (activation_role, activation) = &nodes[1];
    assert_eq!(*activation_role, Some(Role::Button));
    assert_eq!(activation.label(), Some("Open Aurora Scoops"));
    assert!(activation.supports_action(accesskit::Action::Click));

    let (disabled_role, disabled) = &nodes[2];
    assert_eq!(*disabled_role, Some(Role::Button));
    assert_eq!(disabled.label(), Some("Unavailable: Open Offline supplier"));
    assert!(!disabled.supports_action(accesskit::Action::Click));

    let (review_role, review) = &nodes[3];
    assert_eq!(*review_role, Some(Role::Button));
    assert_eq!(review.label(), Some("Review Aurora Scoops"));
    assert!(review.supports_action(accesskit::Action::Click));
}

/// A sortable column shows its sort control on the frame it first appears.
///
/// The 0.5.0 review found a table whose header carried plain labels on a
/// first mount and grew its sort chevrons only after the component was
/// re-rendered — so a reader could not tell a sortable column from a fixed
/// one until they happened to disturb it.
#[gpui::test]
fn a_sortable_column_shows_its_sort_control_on_the_first_draw(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("first", "First", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(
                [RecordColumn::new("name", "Name").sortable(true)],
                window,
                cx,
            );
            records.set_records(
                Progressive::complete(
                    vec![RecordRow::new("a", "Aurora").cells([RecordCell::new("name", "Aurora")])]
                        .into(),
                ),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });

    let selector = "records-sort-5:firstname";
    let first = cx.debug_bounds(selector);
    assert!(
        first.is_some(),
        "the sort control must be painted on the first draw, not after a later one"
    );

    // And it must not appear only because a second pass rescued it.
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        cx.debug_bounds(selector).map(|bounds| bounds.size),
        first.map(|bounds| bounds.size),
        "the sort control must not change shape between the first draw and the next"
    );
}

/// A narrow column keeps its sort glyph and truncates its heading instead.
///
/// The glyph is the only thing that says a column can be sorted. A heading
/// that refused to shrink pushed it past the cell's edge, where it was
/// clipped — so at a width that squeezed the header, a sortable column
/// read as a fixed one. The heading gives way; the glyph does not.
#[gpui::test]
fn a_squeezed_header_keeps_its_sort_glyph(cx: &mut TestAppContext) {
    cx.update(crate::init);
    let (records, cx) =
        cx.add_window_view(|window, cx| RecordsTable::new("tight", "Tight", window, cx));
    let cx: &mut VisualTestContext = cx;
    cx.simulate_resize(size(px(640.), px(300.)));
    cx.update(|window, cx| {
        records.update(cx, |records, cx| {
            records.set_columns(
                [
                    RecordColumn::new("name", "A supplier heading far too long for its column")
                        .sortable(true)
                        .width(px(120.)),
                    RecordColumn::new("region", "Region")
                        .sortable(true)
                        .width(px(120.)),
                ],
                window,
                cx,
            );
            records.set_records(
                Progressive::complete(
                    vec![RecordRow::new("a", "Aurora").cells([
                        RecordCell::new("name", "Aurora"),
                        RecordCell::new("region", "North"),
                    ])]
                    .into(),
                ),
                window,
                cx,
            );
        });
        window.draw(cx).clear(cx);
    });
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let cell = cx
        .debug_bounds("records-sort-5:tightname")
        .expect("the sort control renders");
    let glyph = cx
        .debug_bounds("records-sort-5:tightname-glyph")
        .expect("the sort glyph renders");
    assert!(
        glyph.size.width > px(0.),
        "the glyph must occupy real width, not collapse to nothing"
    );
    assert!(
        glyph.origin.x + glyph.size.width <= cell.origin.x + cell.size.width,
        "the glyph must stay inside its header cell: glyph ends at {}, cell ends at {}",
        glyph.origin.x + glyph.size.width,
        cell.origin.x + cell.size.width
    );
}
