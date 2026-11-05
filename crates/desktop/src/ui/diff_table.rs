//! Multi-file diff review table.

use std::sync::Arc;

use gpui::{
    ClickEvent, Context, ElementId, EventEmitter, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled,
    Window, div, prelude::FluentBuilder as _, px, rems,
};
use gpui_component::{ActiveTheme as _, h_flex, scroll::ScrollableElement as _, v_flex};

/// How a proposed change reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffChangeKind {
    #[default]
    Changed,
    Added,
    Removed,
}

/// A decision on one diff row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DiffProposalState {
    #[default]
    Pending,
    Accepted,
    Rejected,
}

/// The decision control a person can choose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffProposalAction {
    Accept,
    Reject,
}

/// One cell in a diff row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffCell {
    column: SharedString,
    value: SharedString,
}

impl DiffCell {
    pub fn unchanged(column: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            column: column.into(),
            value: value.into(),
        }
    }

    pub fn column(&self) -> &SharedString {
        &self.column
    }

    pub fn value(&self) -> &SharedString {
        &self.value
    }
}

/// One file in a multi-file diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffRow {
    id: SharedString,
    label: SharedString,
    kind: DiffChangeKind,
    cells: Vec<DiffCell>,
    state: DiffProposalState,
}

impl DiffRow {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        kind: DiffChangeKind,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            kind,
            cells: Vec::new(),
            state: DiffProposalState::Pending,
        }
    }

    pub fn cells(mut self, cells: impl IntoIterator<Item = DiffCell>) -> Self {
        self.cells = cells.into_iter().collect();
        self
    }

    pub fn state(mut self, state: DiffProposalState) -> Self {
        self.state = state;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn kind(&self) -> DiffChangeKind {
        self.kind
    }

    pub fn cells_ref(&self) -> &[DiffCell] {
        &self.cells
    }

    pub fn cell(&self, column_id: &str) -> Option<&DiffCell> {
        self.cells
            .iter()
            .find(|cell| cell.column.as_ref() == column_id)
    }

    pub fn state_value(&self) -> DiffProposalState {
        self.state
    }
}

/// One column in a diff table.
#[derive(Clone, Debug, PartialEq)]
pub struct DiffColumn {
    id: SharedString,
    label: SharedString,
    width_in_rems: Option<f32>,
    fixed: bool,
}

impl DiffColumn {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            width_in_rems: None,
            fixed: false,
        }
    }

    pub fn width_in_rems(mut self, width: f32) -> Self {
        self.width_in_rems = Some(width);
        self
    }

    pub fn fixed(mut self, fixed: bool) -> Self {
        self.fixed = fixed;
        self
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn width(&self) -> Option<f32> {
        self.width_in_rems
    }

    pub fn is_fixed(&self) -> bool {
        self.fixed
    }
}

/// User-visible copy for the diff decision controls.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffTableLabels {
    pub accept: SharedString,
    pub reject: SharedString,
}

impl Default for DiffTableLabels {
    fn default() -> Self {
        Self {
            accept: "Accept".into(),
            reject: "Reject".into(),
        }
    }
}

/// An interaction emitted by [`DiffTable`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiffTableEvent {
    DecisionRequested {
        id: SharedString,
        row_id: SharedString,
        action: DiffProposalAction,
    },
}

/// A table of per-file diff decisions.
pub struct DiffTable {
    id: SharedString,
    label: SharedString,
    columns: Vec<DiffColumn>,
    rows: Arc<[DiffRow]>,
    selected_row: Option<SharedString>,
    labels: DiffTableLabels,
}

impl DiffTable {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            columns: Vec::new(),
            rows: Arc::from([]),
            selected_row: None,
            labels: DiffTableLabels::default(),
        }
    }

    pub fn set_labels(&mut self, labels: DiffTableLabels, cx: &mut Context<Self>) {
        if self.labels != labels {
            self.labels = labels;
            cx.notify();
        }
    }

    pub fn set_columns(
        &mut self,
        columns: impl IntoIterator<Item = DiffColumn>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let columns: Vec<DiffColumn> = columns.into_iter().collect();
        if self.columns != columns {
            self.columns = columns;
            cx.notify();
        }
    }

    pub fn set_rows(&mut self, rows: Arc<[DiffRow]>, _window: &mut Window, cx: &mut Context<Self>) {
        if self.rows == rows {
            return;
        }
        self.rows = rows;
        if self
            .selected_row
            .as_ref()
            .is_some_and(|selected| !self.rows.iter().any(|row| row.id() == selected))
        {
            self.selected_row = None;
        }
        cx.notify();
    }

    pub fn set_selected_row(
        &mut self,
        row_id: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row_id = row_id.into();
        if self.selected_row.as_ref() != Some(&row_id) {
            self.selected_row = Some(row_id);
            cx.notify();
        }
    }
}

impl EventEmitter<DiffTableEvent> for DiffTable {}

fn style_column(
    mut column: gpui::Stateful<gpui::Div>,
    definition: &DiffColumn,
) -> gpui::Stateful<gpui::Div> {
    column = column.min_w_0();
    match (definition.is_fixed(), definition.width()) {
        (true, Some(width)) => column.flex_none().w(rems(width)),
        (true, None) => column.flex_none(),
        (false, Some(width)) => column.flex_basis(rems(width)).flex_grow(1.0),
        (false, None) => column.flex_1(),
    }
}

impl Render for DiffTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Grid)
            .aria_label(self.label.clone())
            .w_full()
            .h_full()
            .min_h_0()
            .min_w_0()
            .gap_1();
        let mut header = h_flex()
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border);
        for column in &self.columns {
            let id = column.id().clone();
            let selector = format!("diff-table-header-{}-{id}", self.id);
            header = header.child(style_column(
                div()
                    .id(ElementId::Name(selector.clone().into()))
                    .debug_selector(move || selector)
                    .text_size(px(10.))
                    .truncate()
                    .text_color(cx.theme().muted_foreground)
                    .child(column.label().clone()),
                column,
            ));
        }
        header = header.child(div().flex_none().w(px(112.)));
        body = body.child(header);
        let mut rows = v_flex()
            .id(ElementId::Name(format!("{}-rows", self.id).into()))
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .gap_1();
        for row in self.rows.iter() {
            let row_id = row.id().clone();
            let table_id = self.id.clone();
            let selected = self.selected_row.as_ref() == Some(row.id());
            let accent = match row.kind() {
                DiffChangeKind::Added => cx.theme().success,
                DiffChangeKind::Removed => cx.theme().danger,
                DiffChangeKind::Changed => cx.theme().foreground,
            };
            let accept_selector = format!(
                "diff-table-accept-{}:{}{}",
                table_id.len(),
                table_id,
                row_id
            );
            let reject_selector = format!(
                "diff-table-reject-{}:{}{}",
                table_id.len(),
                table_id,
                row_id
            );
            let accept_event = DiffTableEvent::DecisionRequested {
                id: self.id.clone(),
                row_id: row_id.clone(),
                action: DiffProposalAction::Accept,
            };
            let reject_event = DiffTableEvent::DecisionRequested {
                id: self.id.clone(),
                row_id: row_id.clone(),
                action: DiffProposalAction::Reject,
            };
            let mut row_element = h_flex()
                .id(ElementId::Name(
                    format!("diff-table-row-{}", row.id()).into(),
                ))
                .debug_selector({
                    let row_id = row.id().clone();
                    move || format!("diff-table-row-{row_id}")
                })
                .role(Role::Row)
                .aria_label(row.label().clone())
                .aria_selected(selected)
                .focusable()
                .tab_stop(true)
                .w_full()
                .items_center()
                .gap_2()
                .rounded(px(5.))
                .px_2()
                .py_1()
                .when(selected, |row| row.bg(cx.theme().accent));
            for column in &self.columns {
                let column_id = column.id().clone();
                let selector = format!("diff-table-cell-{}-{}-{column_id}", self.id, row.id());
                let value = row
                    .cell(column_id.as_ref())
                    .map(|cell| cell.value().clone())
                    .unwrap_or_default();
                row_element = row_element.child(style_column(
                    div()
                        .id(ElementId::Name(selector.clone().into()))
                        .debug_selector(move || selector)
                        .truncate()
                        .text_xs()
                        .text_color(accent)
                        .child(value),
                    column,
                ));
            }
            row_element = row_element
                .child(
                    h_flex()
                        .flex_none()
                        .w(px(112.))
                        .justify_end()
                        .gap_1()
                        .child(
                            div()
                                .id(ElementId::Name(accept_selector.clone().into()))
                                .debug_selector(move || accept_selector.clone())
                                .role(Role::Button)
                                .aria_label(self.labels.accept.clone())
                                .cursor_pointer()
                                .rounded(px(4.))
                                .px_2()
                                .py(px(2.))
                                .text_size(px(10.))
                                .text_color(match row.state_value() {
                                    DiffProposalState::Accepted => cx.theme().success,
                                    _ => cx.theme().link,
                                })
                                .hover(|style| style.bg(cx.theme().muted))
                                .child(self.labels.accept.clone())
                                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                    cx.emit(accept_event.clone());
                                })),
                        )
                        .child(
                            div()
                                .id(ElementId::Name(reject_selector.clone().into()))
                                .debug_selector(move || reject_selector.clone())
                                .role(Role::Button)
                                .aria_label(self.labels.reject.clone())
                                .cursor_pointer()
                                .rounded(px(4.))
                                .px_2()
                                .py(px(2.))
                                .text_size(px(10.))
                                .text_color(match row.state_value() {
                                    DiffProposalState::Rejected => cx.theme().danger,
                                    _ => cx.theme().muted_foreground,
                                })
                                .hover(|style| style.bg(cx.theme().muted))
                                .child(self.labels.reject.clone())
                                .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                    cx.emit(reject_event.clone());
                                })),
                        ),
                )
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.selected_row = Some(row_id.clone());
                    cx.notify();
                }));
            rows = rows.child(row_element);
        }
        body = body.child(rows);
        body
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    use gpui::{
        AppContext as _, Modifiers, Render, ScrollDelta, Subscription, TestAppContext, TouchPhase,
        div, point, px, size,
    };

    use super::*;

    struct DiffProbe {
        table: gpui::Entity<DiffTable>,
        events: Rc<RefCell<Vec<DiffTableEvent>>>,
        _subscription: Subscription,
    }

    impl Render for DiffProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(720.)).h(px(260.)).child(self.table.clone())
        }
    }

    fn diff_probe(
        cx: &mut TestAppContext,
    ) -> (gpui::Entity<DiffProbe>, &mut gpui::VisualTestContext) {
        cx.update(gpui_component::init);
        let events = Rc::new(RefCell::new(Vec::new()));
        let events_for_window = events.clone();
        let (probe, window) = cx.add_window_view(move |window, cx| {
            let table = cx.new(|cx| {
                let mut table = DiffTable::new("reader-diff", "Changed files", window, cx);
                table.set_labels(
                    DiffTableLabels {
                        accept: "接受".into(),
                        reject: "拒绝".into(),
                    },
                    cx,
                );
                table.set_columns(
                    [
                        DiffColumn::new("file", "文件").width_in_rems(20.),
                        DiffColumn::new("added", "+").fixed(true).width_in_rems(3.5),
                        DiffColumn::new("removed", "−")
                            .fixed(true)
                            .width_in_rems(3.5),
                    ],
                    window,
                    cx,
                );
                let rows: Arc<[DiffRow]> = (0..30)
                    .map(|index| {
                        DiffRow::new(
                            format!("file-{index}"),
                            format!("path-{index}"),
                            DiffChangeKind::Changed,
                        )
                        .cells([
                            DiffCell::unchanged("file", format!("path-{index}")),
                            DiffCell::unchanged("added", index.to_string()),
                            DiffCell::unchanged("removed", "0"),
                        ])
                    })
                    .collect::<Vec<_>>()
                    .into();
                table.set_rows(rows, window, cx);
                table.set_selected_row("file-0", window, cx);
                table
            });
            let subscription =
                cx.subscribe_in(&table, window, move |_, _, event: &DiffTableEvent, _, _| {
                    events_for_window.borrow_mut().push(event.clone());
                });
            DiffProbe {
                table,
                events,
                _subscription: subscription,
            }
        });
        window.simulate_resize(size(px(800.), px(600.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        (probe, window)
    }

    #[gpui::test]
    fn columns_align_row_selects_and_decisions_emit(cx: &mut TestAppContext) {
        let (probe, window) = diff_probe(cx);
        for (column, header_selector, cell_selector) in [
            (
                "file",
                "diff-table-header-reader-diff-file",
                "diff-table-cell-reader-diff-file-1-file",
            ),
            (
                "added",
                "diff-table-header-reader-diff-added",
                "diff-table-cell-reader-diff-file-1-added",
            ),
            (
                "removed",
                "diff-table-header-reader-diff-removed",
                "diff-table-cell-reader-diff-file-1-removed",
            ),
        ] {
            let header = window.debug_bounds(header_selector).expect("diff header");
            let cell = window.debug_bounds(cell_selector).expect("diff cell");
            assert_eq!(header.origin.x, cell.origin.x, "{column} x alignment");
            assert_eq!(
                header.size.width, cell.size.width,
                "{column} width alignment"
            );
        }
        let row = window
            .debug_bounds("diff-table-row-file-1")
            .expect("diff row");
        window.simulate_click(row.center(), Modifiers::default());
        window.run_until_parked();
        let selection = window.update(|_, cx| probe.read(cx).table.read(cx).selected_row.clone());
        assert_eq!(selection.as_deref(), Some("file-1"));
        let accept = window
            .debug_bounds("diff-table-accept-11:reader-difffile-1")
            .expect("accept control");
        window.simulate_click(accept.center(), Modifiers::default());
        window.run_until_parked();
        let events = window.update(|_, cx| probe.read(cx).events.borrow().clone());
        assert!(events.iter().any(|event| matches!(
            event,
            DiffTableEvent::DecisionRequested {
                row_id,
                action: DiffProposalAction::Accept,
                ..
            } if row_id.as_ref() == "file-1"
        )));
        let labels = window.update(|_, cx| probe.read(cx).table.read(cx).labels.clone());
        assert_eq!(labels.accept.as_ref(), "接受");
        assert_eq!(labels.reject.as_ref(), "拒绝");
    }

    #[gpui::test]
    fn long_diff_scrolls_inside_its_host(cx: &mut TestAppContext) {
        let (_probe, window) = diff_probe(cx);
        let row = window
            .debug_bounds("diff-table-row-file-0")
            .expect("first diff row");
        let last_before = window
            .debug_bounds("diff-table-row-file-29")
            .expect("last diff row before scroll");
        window.simulate_event(gpui::ScrollWheelEvent {
            position: row.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-240.))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let last_after = window
            .debug_bounds("diff-table-row-file-29")
            .expect("last diff row after scroll");
        assert!(
            last_after.origin.y < last_before.origin.y,
            "long diff tables must scroll inside their fixed-height host"
        );
    }
}
