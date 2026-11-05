//! Filterable extension table used by the extensions manager.

use std::sync::Arc;

use gpui::{
    ClickEvent, Context, ElementId, EventEmitter, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, h_flex, scroll::ScrollableElement as _, v_flex};

/// One cell in a filter table row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterCell {
    column: SharedString,
    value: SharedString,
}

impl FilterCell {
    pub fn new(column: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
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

/// One row in a filter table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterRow {
    id: SharedString,
    label: SharedString,
    cells: Vec<FilterCell>,
}

impl FilterRow {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            cells: Vec::new(),
        }
    }

    pub fn cells(mut self, cells: impl IntoIterator<Item = FilterCell>) -> Self {
        self.cells = cells.into_iter().collect();
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn all_cells(&self) -> &[FilterCell] {
        &self.cells
    }

    pub fn cell(&self, column_id: &str) -> Option<&FilterCell> {
        self.cells
            .iter()
            .find(|cell| cell.column.as_ref() == column_id)
    }
}

/// One column definition.
#[derive(Clone, Debug, PartialEq)]
pub struct FilterColumn {
    id: SharedString,
    label: SharedString,
    fill: f32,
}

impl FilterColumn {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            fill: 1.0,
        }
    }

    pub fn fill(mut self, fill: f32) -> Self {
        self.fill = fill;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn fill_weight(&self) -> f32 {
        self.fill
    }
}

/// One filter chip definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FilterDefinition {
    id: SharedString,
    label: SharedString,
    count: usize,
    active: bool,
}

impl FilterDefinition {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, count: usize) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            count,
            active: false,
        }
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn label(&self) -> &SharedString {
        &self.label
    }

    pub fn count(&self) -> usize {
        self.count
    }

    pub fn is_active(&self) -> bool {
        self.active
    }
}

/// An interaction emitted by [`FilterTable`].
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)]
pub enum FilterTableEvent {
    FilterRequested {
        id: SharedString,
        filter_id: SharedString,
    },
    SelectionRequested {
        id: SharedString,
        row_id: SharedString,
    },
    ActivationRequested {
        id: SharedString,
        row_id: SharedString,
    },
    SortRequested {
        id: SharedString,
        column_id: SharedString,
    },
}

/// A filterable table with stable row identity.
pub struct FilterTable {
    id: SharedString,
    label: SharedString,
    empty_label: SharedString,
    activation_label: SharedString,
    filter_labels: (SharedString, SharedString, SharedString),
    filters: Vec<FilterDefinition>,
    columns: Vec<FilterColumn>,
    rows: Arc<[FilterRow]>,
    selected_row: Option<SharedString>,
}

impl FilterTable {
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            empty_label: "No records".into(),
            activation_label: "Open".into(),
            filter_labels: ("Filters".into(), "selected".into(), "not selected".into()),
            filters: Vec::new(),
            columns: Vec::new(),
            rows: Arc::from([]),
            selected_row: None,
        }
    }

    pub fn set_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        let label = label.into();
        if self.label != label {
            self.label = label;
            cx.notify();
        }
    }

    pub fn set_filter_labels(
        &mut self,
        filters: impl Into<SharedString>,
        active: impl Into<SharedString>,
        inactive: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let labels = (filters.into(), active.into(), inactive.into());
        if self.filter_labels != labels {
            self.filter_labels = labels;
            cx.notify();
        }
    }

    pub fn set_activation_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        let label = label.into();
        if self.activation_label != label {
            self.activation_label = label;
            cx.notify();
        }
    }

    pub fn set_empty_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        let label = label.into();
        if self.empty_label != label {
            self.empty_label = label;
            cx.notify();
        }
    }

    pub fn set_filters(
        &mut self,
        filters: impl IntoIterator<Item = FilterDefinition>,
        cx: &mut Context<Self>,
    ) {
        let filters: Vec<FilterDefinition> = filters.into_iter().collect();
        if self.filters != filters {
            self.filters = filters;
            cx.notify();
        }
    }

    pub fn set_columns(
        &mut self,
        columns: impl IntoIterator<Item = FilterColumn>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let columns: Vec<FilterColumn> = columns.into_iter().collect();
        if self.columns != columns {
            self.columns = columns;
            cx.notify();
        }
    }

    pub fn set_rows(&mut self, rows: Arc<[FilterRow]>, cx: &mut Context<Self>) {
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

    pub fn filters(&self) -> &[FilterDefinition] {
        &self.filters
    }

    pub fn selected_row(&self) -> Option<&SharedString> {
        self.selected_row.as_ref()
    }
}

impl EventEmitter<FilterTableEvent> for FilterTable {}

fn click_count(event: &ClickEvent) -> usize {
    match event {
        ClickEvent::Mouse(event) => event.up.click_count,
        ClickEvent::Keyboard(_) | ClickEvent::Touch(_) => 1,
    }
}

impl Render for FilterTable {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Grid)
            .aria_label(self.label.clone())
            .w_full()
            .h_full()
            .min_h_0()
            .min_w_0()
            .gap_2();
        if !self.filters.is_empty() {
            body = body.child(
                h_flex()
                    .id(ElementId::Name(format!("{}-filters", self.id).into()))
                    .role(Role::Group)
                    .aria_label(self.filter_labels.0.clone())
                    .w_full()
                    .flex_wrap()
                    .items_center()
                    .gap_1()
                    .children(self.filters.iter().map(|filter| {
                        let id = self.id.clone();
                        let filter_id = filter.id().clone();
                        if filter.is_active() {
                            h_flex()
                                .id(ElementId::Name(
                                    format!("filter-table-chip-{filter_id}").into(),
                                ))
                                .role(Role::Button)
                                .aria_label(filter.label().clone())
                                .aria_description(self.filter_labels.1.clone())
                                .aria_selected(true)
                                .items_center()
                                .gap_1()
                                .rounded_full()
                                .border_1()
                                .border_color(cx.theme().primary)
                                .bg(cx.theme().accent)
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .child(div().text_xs().child(filter.label().clone()))
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(filter.count().to_string()),
                                )
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.emit(FilterTableEvent::FilterRequested {
                                        id: id.clone(),
                                        filter_id: filter_id.clone(),
                                    });
                                    this.selected_row = None;
                                    cx.notify();
                                }))
                                .into_any_element()
                        } else {
                            h_flex()
                                .id(ElementId::Name(
                                    format!("filter-table-chip-{filter_id}").into(),
                                ))
                                .role(Role::Button)
                                .aria_label(filter.label().clone())
                                .aria_description(self.filter_labels.2.clone())
                                .aria_selected(false)
                                .items_center()
                                .gap_1()
                                .rounded_full()
                                .border_1()
                                .border_color(cx.theme().border)
                                .bg(cx.theme().popover)
                                .px_3()
                                .py_1()
                                .cursor_pointer()
                                .child(div().text_xs().child(filter.label().clone()))
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_size(px(10.))
                                        .text_color(cx.theme().muted_foreground)
                                        .child(filter.count().to_string()),
                                )
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    cx.emit(FilterTableEvent::FilterRequested {
                                        id: id.clone(),
                                        filter_id: filter_id.clone(),
                                    });
                                    this.selected_row = None;
                                    cx.notify();
                                }))
                                .into_any_element()
                        }
                    })),
            );
        }
        let mut header = h_flex()
            .w_full()
            .gap_2()
            .px_2()
            .py_1()
            .border_b_1()
            .border_color(cx.theme().border);
        for column in &self.columns {
            let column_id = column.id().clone();
            let selector = format!("filter-table-header-{}-{column_id}", self.id);
            header = header.child(
                div()
                    .id(ElementId::Name(selector.clone().into()))
                    .debug_selector(move || selector)
                    .flex_basis(px(0.))
                    .flex_grow(column.fill_weight())
                    .min_w_0()
                    .truncate()
                    .text_size(px(10.))
                    .text_color(cx.theme().muted_foreground)
                    .child(column.label().clone()),
            );
        }
        header = header.child(
            div()
                .flex_none()
                .w(px(72.))
                .text_size(px(10.))
                .text_color(cx.theme().muted_foreground)
                .child(self.activation_label.clone()),
        );
        body = body.child(header);
        let mut row_list = v_flex()
            .id(ElementId::Name(format!("{}-rows", self.id).into()))
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .gap_1();
        if self.rows.is_empty() {
            row_list = row_list.child(
                div()
                    .p_3()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.empty_label.clone()),
            );
        }
        for row in self.rows.iter() {
            let select_id = self.id.clone();
            let select_row = row.id().clone();
            let activate_id = self.id.clone();
            let activate_row = row.id().clone();
            let open_id = self.id.clone();
            let open_row = row.id().clone();
            let key_id = self.id.clone();
            let key_row = row.id().clone();
            let key_activate_id = self.id.clone();
            let key_activate_row = row.id().clone();
            let selected = self.selected_row.as_ref() == Some(row.id());
            let mut row_element = h_flex()
                .id(ElementId::Name(
                    format!("filter-table-row-{}", row.id()).into(),
                ))
                .debug_selector({
                    let row_id = row.id().clone();
                    move || format!("filter-table-row-{}", row_id)
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
                .when(selected, |row| row.bg(cx.theme().accent))
                .hover(|style| style.bg(cx.theme().muted));
            for column in &self.columns {
                let column_id = column.id().clone();
                let selector = format!("filter-table-cell-{}-{}-{}", self.id, row.id(), column_id);
                let value = row
                    .cell(column_id.as_ref())
                    .map(|cell| cell.value().clone())
                    .unwrap_or_default();
                row_element = row_element.child(
                    div()
                        .id(ElementId::Name(selector.clone().into()))
                        .debug_selector(move || selector)
                        .flex_basis(px(0.))
                        .flex_grow(column.fill_weight())
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .child(value),
                );
            }
            let open_selector = format!("filter-table-open-{}", row.id());
            row_element = row_element
                .child(
                    div()
                        .id(ElementId::Name(open_selector.clone().into()))
                        .debug_selector(move || open_selector)
                        .role(Role::Button)
                        .aria_label(self.activation_label.clone())
                        .flex_none()
                        .w(px(72.))
                        .text_size(px(10.))
                        .text_color(cx.theme().link)
                        .cursor_pointer()
                        .child(self.activation_label.clone())
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.selected_row = Some(open_row.clone());
                            cx.emit(FilterTableEvent::SelectionRequested {
                                id: open_id.clone(),
                                row_id: open_row.clone(),
                            });
                            cx.emit(FilterTableEvent::ActivationRequested {
                                id: open_id.clone(),
                                row_id: open_row.clone(),
                            });
                            cx.stop_propagation();
                            cx.notify();
                        })),
                )
                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                    this.selected_row = Some(select_row.clone());
                    cx.emit(FilterTableEvent::SelectionRequested {
                        id: select_id.clone(),
                        row_id: select_row.clone(),
                    });
                    if click_count(event) >= 2 {
                        cx.emit(FilterTableEvent::ActivationRequested {
                            id: activate_id.clone(),
                            row_id: activate_row.clone(),
                        });
                    }
                    cx.notify();
                }))
                .on_key_down(cx.listener(move |this, event: &KeyDownEvent, _, cx| {
                    if event.keystroke.key == "enter" || event.keystroke.key == "space" {
                        this.selected_row = Some(key_row.clone());
                        cx.emit(FilterTableEvent::SelectionRequested {
                            id: key_id.clone(),
                            row_id: key_row.clone(),
                        });
                        cx.emit(FilterTableEvent::ActivationRequested {
                            id: key_activate_id.clone(),
                            row_id: key_activate_row.clone(),
                        });
                        cx.stop_propagation();
                        cx.notify();
                    }
                }));
            row_list = row_list.child(row_element);
        }
        body = body.child(row_list);
        body
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, sync::Arc};

    use gpui::{
        AppContext as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, Render, ScrollDelta,
        Subscription, TestAppContext, TouchPhase, div, point, px, size,
    };

    use super::*;

    struct TableProbe {
        table: gpui::Entity<FilterTable>,
        events: Rc<RefCell<Vec<FilterTableEvent>>>,
        _subscription: Subscription,
    }

    impl Render for TableProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(700.)).h(px(320.)).child(self.table.clone())
        }
    }

    fn table_probe(
        cx: &mut TestAppContext,
    ) -> (gpui::Entity<TableProbe>, &mut gpui::VisualTestContext) {
        cx.update(gpui_component::init);
        let events = Rc::new(RefCell::new(Vec::new()));
        let events_for_window = events.clone();
        let (probe, window) = cx.add_window_view(move |window, cx| {
            let table = cx.new(|cx| {
                let mut table = FilterTable::new("filter-table", "Extensions", window, cx);
                table.set_filters(
                    [
                        FilterDefinition::new("mcp", "MCP", 30).active(true),
                        FilterDefinition::new("skills", "Skills", 2),
                    ],
                    cx,
                );
                table.set_columns(
                    [
                        FilterColumn::new("name", "Name").fill(1.0),
                        FilterColumn::new("detail", "Detail").fill(2.0),
                        FilterColumn::new("status", "Status").fill(1.0),
                    ],
                    window,
                    cx,
                );
                let rows: Arc<[FilterRow]> = (0..30)
                    .map(|index| {
                        FilterRow::new(format!("row-{index}"), format!("Row {index}")).cells([
                            FilterCell::new("name", format!("name-{index}")),
                            FilterCell::new("detail", format!("detail-{index}")),
                            FilterCell::new("status", format!("status-{index}")),
                        ])
                    })
                    .collect::<Vec<_>>()
                    .into();
                table.set_rows(rows, cx);
                table
            });
            let subscription = cx.subscribe_in(
                &table,
                window,
                move |_, _, event: &FilterTableEvent, _, _| {
                    events_for_window.borrow_mut().push(event.clone());
                },
            );
            TableProbe {
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
    fn single_click_selects_and_double_click_activates(cx: &mut TestAppContext) {
        let (probe, window) = table_probe(cx);
        let row = window
            .debug_bounds("filter-table-row-row-1")
            .expect("filter row");
        window.simulate_click(row.center(), Modifiers::default());
        window.run_until_parked();
        let events = window.update(|_, cx| probe.read(cx).events.borrow().clone());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FilterTableEvent::SelectionRequested { .. }))
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, FilterTableEvent::ActivationRequested { .. })),
            "a single click must only select"
        );
        window.update(|_, cx| probe.read(cx).events.borrow_mut().clear());
        window.simulate_event(MouseDownEvent {
            position: row.center(),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        window.simulate_event(MouseUpEvent {
            position: row.center(),
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 2,
        });
        window.run_until_parked();
        let events = window.update(|_, cx| probe.read(cx).events.borrow().clone());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FilterTableEvent::ActivationRequested { .. })),
            "a double click must activate"
        );
    }

    #[gpui::test]
    fn open_control_activates_and_columns_align(cx: &mut TestAppContext) {
        let (probe, window) = table_probe(cx);
        let row = window
            .debug_bounds("filter-table-row-row-1")
            .expect("filter row");
        let open = window
            .debug_bounds("filter-table-open-row-1")
            .expect("open action");
        window.simulate_click(open.center(), Modifiers::default());
        window.run_until_parked();
        let events = window.update(|_, cx| probe.read(cx).events.borrow().clone());
        assert!(
            events
                .iter()
                .any(|event| matches!(event, FilterTableEvent::ActivationRequested { .. })),
            "the Open control must activate the row"
        );
        for (column, header_selector, cell_selector) in [
            (
                "name",
                "filter-table-header-filter-table-name",
                "filter-table-cell-filter-table-row-1-name",
            ),
            (
                "detail",
                "filter-table-header-filter-table-detail",
                "filter-table-cell-filter-table-row-1-detail",
            ),
            (
                "status",
                "filter-table-header-filter-table-status",
                "filter-table-cell-filter-table-row-1-status",
            ),
        ] {
            let header = window.debug_bounds(header_selector).expect("column header");
            let cell = window.debug_bounds(cell_selector).expect("column cell");
            assert_eq!(header.origin.x, cell.origin.x, "{column} x alignment");
            assert_eq!(
                header.size.width, cell.size.width,
                "{column} width alignment"
            );
        }
        let last_before = window
            .debug_bounds("filter-table-row-row-29")
            .expect("last row before scroll");
        window.simulate_event(gpui::ScrollWheelEvent {
            position: row.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(-240.))),
            modifiers: Modifiers::default(),
            touch_phase: TouchPhase::Moved,
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        let last_after = window
            .debug_bounds("filter-table-row-row-29")
            .expect("last row after scroll");
        assert!(
            last_after.origin.y < last_before.origin.y,
            "long tables must scroll inside their host"
        );
    }
}
