//! Controlled filter-table values and animated stable-row presentation.

use gpui_base::StyledExt as _;
use std::{collections::HashSet, sync::Arc};

use gpui::{
    App, AppContext as _, Axis, Context, Div, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, ParentElement as _, Render, Role, ScrollHandle, SharedString,
    Stateful, StatefulInteractiveElement as _, Styled as _, Subscription, Window, accesskit, div,
    prelude::FluentBuilder as _,
};
use gpui_base::InteractiveElementExt as _;
use gpui_component::{ActiveTheme as _, scroll::ScrollableMask};

use crate::scrolling::PolicyScrollbarExt as _;
use crate::{
    control::outlined_control_with_label,
    motion::MotionTokens,
    records_table::{
        RecordsTable, RecordsTableEvent, record_columns_have_unique_ids,
        record_rows_have_unique_ids,
    },
    stream::Progressive,
    theme::SemanticStyledExt as _,
};

pub use crate::records_table::{
    RecordCell as FilterCell, RecordColumn as FilterColumn,
    RecordColumnAlignment as FilterColumnAlignment, RecordRow as FilterRow,
    RecordSortDirection as FilterSortDirection,
};

/// One consumer-owned filter choice with stable application identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilterDefinition {
    id: SharedString,
    label: SharedString,
    count: usize,
    active: bool,
    disabled: bool,
}

impl FilterDefinition {
    /// Creates an inactive filter with its current result count.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>, count: usize) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            count,
            active: false,
            disabled: false,
        }
    }

    /// Sets whether this filter is active in the consumer-owned snapshot.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Sets whether this filter rejects activation intent.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable application filter ID.
    pub fn id(&self) -> &str {
        self.id.as_ref()
    }

    /// Returns the visible filter label.
    pub fn label(&self) -> &str {
        self.label.as_ref()
    }

    /// Returns the consumer-supplied result count.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Returns whether this filter is active.
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Returns whether this filter rejects activation intent.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

/// Typed application intent emitted by a filter table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterTableEvent {
    /// Requests a new controlled active state for one stable filter.
    FilterRequested {
        /// Stable filter-table ID.
        id: SharedString,
        /// Stable application filter ID.
        filter_id: SharedString,
        /// Requested active state.
        active: bool,
    },
    /// Requests that the application select the identified row.
    SelectionRequested {
        /// Stable filter-table ID.
        id: SharedString,
        /// Stable application row ID.
        row_id: SharedString,
    },
    /// Requests the primary action for the identified row.
    ActivationRequested {
        /// Stable filter-table ID.
        id: SharedString,
        /// Stable application row ID.
        row_id: SharedString,
    },
    /// Requests a controlled sort projection.
    SortRequested {
        /// Stable filter-table ID.
        id: SharedString,
        /// Stable application column ID.
        column_id: SharedString,
        /// Requested direction, or `None` to clear sorting.
        direction: Option<FilterSortDirection>,
    },
}

/// A controlled, virtualized records table with stable filter controls and reorder motion.
///
/// Applications own filters, filtered row order, selection, sorting, and async
/// work. When the ordered row snapshot changes, rows that remain visible move
/// to their new positions through GPUI's finite keyed transition facility.
/// Reduced-motion mode snaps directly to the new order.
pub struct FilterTable {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    label: SharedString,
    filter_controls_label: SharedString,
    active_label: SharedString,
    inactive_label: SharedString,
    filters: Arc<[FilterDefinition]>,
    columns: Arc<[FilterColumn]>,
    rows: Progressive<Arc<[FilterRow]>>,
    selected_row_id: Option<SharedString>,
    sort_column_id: Option<SharedString>,
    sort_direction: Option<FilterSortDirection>,
    records_table: gpui::Entity<RecordsTable>,
    filter_scroll: ScrollHandle,
    filter_focus: FocusHandle,
    focused_filter_id: Option<SharedString>,
    filter_focus_engaged: bool,
    _records_subscription: Subscription,
    _filter_focus_subscriptions: Vec<Subscription>,
}

impl FilterTable {
    /// Creates an empty filter table with stable identity and an accessible label.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = id.into();
        let label = label.into();
        let filter_controls_label = format!("Filters for {label}").into();
        let records_id = scoped_filter_id("records", &id, "table");
        let records_label = label.clone();
        let records_table = cx.new(|cx| {
            let mut table = RecordsTable::new(records_id, records_label, window, cx);
            // Visible-row reorder rides the policy's reflow role, read at
            // construction: the proven response, replaceable with the
            // policy before the table is built.
            table.set_row_reorder_response(
                Some(MotionTokens::read(cx).reflow_spring().response()),
                cx,
            );
            table
        });
        let records_subscription = cx.subscribe(&records_table, |this, _, event, cx| {
            this.handle_records_event(event, cx);
        });
        let filter_focus = cx.focus_handle();
        let filter_focus_subscriptions = vec![
            cx.on_focus(&filter_focus, window, |this, _, _| {
                this.filter_focus_engaged = true;
                this.reveal_focused_filter();
            }),
            cx.on_blur(&filter_focus, window, |this, _, _| {
                this.filter_focus_engaged = false;
            }),
        ];
        Self {
            style: gpui::StyleRefinement::default(),
            id,
            label,
            filter_controls_label,
            active_label: "active".into(),
            inactive_label: "inactive".into(),
            filters: Arc::from([]),
            columns: Arc::from([]),
            rows: Progressive::pending(Arc::from([])),
            selected_row_id: None,
            sort_column_id: None,
            sort_direction: None,
            records_table,
            filter_scroll: ScrollHandle::new(),
            filter_focus,
            focused_filter_id: None,
            filter_focus_engaged: false,
            _records_subscription: records_subscription,
            _filter_focus_subscriptions: filter_focus_subscriptions,
        }
    }

    /// Replaces the controlled filter definitions.
    ///
    /// A snapshot containing duplicate stable filter IDs is ignored atomically.
    pub fn set_filters(
        &mut self,
        filters: impl IntoIterator<Item = FilterDefinition>,
        cx: &mut Context<Self>,
    ) {
        let filters = filters.into_iter().collect::<Vec<_>>();
        let mut seen = HashSet::with_capacity(filters.len());
        if !filters.iter().all(|filter| seen.insert(filter.id())) {
            return;
        }
        self.filters = filters.into();
        if self.focused_filter_id.as_ref().is_none_or(|focused| {
            !self
                .filters
                .iter()
                .any(|filter| filter.id == *focused && !filter.disabled)
        }) {
            self.focused_filter_id = self
                .filters
                .iter()
                .find(|filter| !filter.disabled)
                .map(|filter| filter.id.clone());
        }
        if self.filter_focus_engaged {
            self.reveal_focused_filter();
        }
        cx.notify();
    }

    /// Returns the current controlled filter definitions.
    pub fn filters(&self) -> &[FilterDefinition] {
        &self.filters
    }

    /// Replaces the accessible table label.
    pub fn set_label(&mut self, label: impl Into<SharedString>, cx: &mut Context<Self>) {
        let label = label.into();
        if self.label == label {
            return;
        }
        self.label = label;
        cx.notify();
    }

    /// Replaces accessible filter-group and filter-state copy.
    pub fn set_filter_labels(
        &mut self,
        controls: impl Into<SharedString>,
        active: impl Into<SharedString>,
        inactive: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.filter_controls_label = controls.into();
        self.active_label = active.into();
        self.inactive_label = inactive.into();
        cx.notify();
    }

    /// Replaces the accessible label for row activation.
    pub fn set_activation_label(
        &mut self,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.records_table
            .update(cx, |table, cx| table.set_activation_label(label, cx));
    }

    /// Replaces the label shown when no rows match.
    pub fn set_empty_label(
        &mut self,
        label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.records_table
            .update(cx, |table, cx| table.set_empty_label(label, cx));
    }

    /// Replaces the controlled column snapshot.
    ///
    /// A snapshot containing duplicate stable column IDs is ignored atomically.
    pub fn set_columns(
        &mut self,
        columns: impl IntoIterator<Item = FilterColumn>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let columns = columns.into_iter().collect::<Vec<_>>();
        if !record_columns_have_unique_ids(&columns) {
            return;
        }
        self.columns = columns.into();
        if self.sort_column_id.as_ref().is_some_and(|sort_column_id| {
            !self
                .columns
                .iter()
                .any(|column| column.id() == sort_column_id.as_ref() && column.is_sortable())
        }) {
            self.sort_column_id = None;
            self.sort_direction = None;
        }
        let columns = self.columns.clone();
        self.records_table.update(cx, |table, cx| {
            table.set_columns(columns.iter().cloned(), window, cx);
        });
        cx.notify();
    }

    /// Scrolls the identified filter control into the horizontal viewport.
    pub fn scroll_to_filter(&mut self, filter_id: &str, cx: &mut Context<Self>) {
        if let Some(index) = self
            .filters
            .iter()
            .position(|filter| filter.id() == filter_id)
        {
            // gpui-component's scrollbar layer occupies the first tracked child.
            self.filter_scroll.scroll_to_item(index.saturating_add(1));
            cx.notify();
        }
    }

    /// Moves keyboard focus to an enabled stable filter and reveals it.
    pub fn focus_filter(&mut self, filter_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .filters
            .iter()
            .position(|filter| filter.id() == filter_id && !filter.is_disabled())
        else {
            return;
        };
        self.focused_filter_id = Some(self.filters[index].id.clone());
        self.filter_focus_engaged = true;
        self.filter_scroll.scroll_to_item(index.saturating_add(1));
        self.filter_focus.focus(window, cx);
        cx.notify();
    }

    /// Replaces the controlled progressive rows in their final filtered order.
    ///
    /// Duplicate row IDs or duplicate cell column IDs make the complete
    /// replacement invalid, so the prior controlled snapshot is retained.
    pub fn set_rows(&mut self, rows: Progressive<Arc<[FilterRow]>>, cx: &mut Context<Self>) {
        if !record_rows_have_unique_ids(rows.content()) {
            return;
        }
        self.rows = rows.clone();
        if self.selected_row_id.as_ref().is_some_and(|selected| {
            !self
                .rows
                .content()
                .iter()
                .any(|row| row.id() == selected.as_ref() && !row.is_disabled())
        }) {
            self.selected_row_id = None;
        }
        self.records_table
            .update(cx, |table, cx| table.set_records_snapshot(rows, cx));
        cx.notify();
    }

    /// Replaces the controlled selected row when the stable ID is enabled.
    pub fn set_selected_row(
        &mut self,
        row_id: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row_id = row_id.into();
        if !self
            .rows
            .content()
            .iter()
            .any(|row| row.id() == row_id.as_ref() && !row.is_disabled())
        {
            return;
        }
        self.selected_row_id = Some(row_id.clone());
        self.records_table.update(cx, |table, cx| {
            table.set_selected_row(row_id, window, cx);
        });
        cx.notify();
    }

    /// Clears the controlled selected row.
    pub fn clear_selected_row(&mut self, cx: &mut Context<Self>) {
        self.selected_row_id = None;
        self.records_table
            .update(cx, |table, cx| table.clear_selected_row(cx));
        cx.notify();
    }

    /// Returns the controlled selected row ID.
    pub fn selected_row_id(&self) -> Option<&str> {
        self.selected_row_id.as_deref()
    }

    /// Replaces the controlled sort projection.
    pub fn set_sort(
        &mut self,
        column_id: impl Into<SharedString>,
        direction: Option<FilterSortDirection>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let column_id = column_id.into();
        if direction.is_some()
            && !self
                .columns
                .iter()
                .any(|column| column.id() == column_id.as_ref() && column.is_sortable())
        {
            return;
        }
        self.sort_column_id = direction.map(|_| column_id.clone());
        self.sort_direction = direction;
        self.records_table.update(cx, |table, cx| {
            table.set_sort(column_id, direction, window, cx);
        });
        cx.notify();
    }

    /// Returns the controlled sort column ID and direction.
    pub fn sort(&self) -> Option<(&str, FilterSortDirection)> {
        self.sort_column_id.as_deref().zip(self.sort_direction)
    }

    /// Scrolls the identified row into view when it exists.
    pub fn scroll_to_row(&mut self, row_id: &str, cx: &mut Context<Self>) {
        self.records_table
            .update(cx, |table, cx| table.scroll_to_row(row_id, cx));
    }

    /// Scrolls the identified column into view when it exists.
    pub fn scroll_to_column(&mut self, column_id: &str, cx: &mut Context<Self>) {
        self.records_table
            .update(cx, |table, cx| table.scroll_to_column(column_id, cx));
    }

    /// Returns the number of stable rows constructed for the current virtual viewport.
    pub fn visible_row_count(&self, cx: &App) -> usize {
        self.records_table.read(cx).visible_row_count(cx)
    }

    /// Returns the bounded number of visible rows carrying active reorder state.
    pub fn animating_row_count(&self, cx: &App) -> usize {
        self.records_table.read(cx).animating_row_count(cx)
    }

    /// Moves keyboard focus to the virtualized grid.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.records_table.focus_handle(cx).focus(window, cx);
    }

    fn request_filter(&mut self, filter_id: SharedString, cx: &mut Context<Self>) {
        let Some(filter) = self
            .filters
            .iter()
            .find(|filter| filter.id == filter_id && !filter.disabled)
        else {
            return;
        };
        cx.emit(FilterTableEvent::FilterRequested {
            id: self.id.clone(),
            filter_id,
            active: !filter.active,
        });
    }

    fn move_filter_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = self
            .filters
            .iter()
            .enumerate()
            .filter(|(_, filter)| !filter.disabled)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if enabled.is_empty() {
            return;
        }
        let current = self
            .focused_filter_id
            .as_ref()
            .and_then(|focused| self.filters.iter().position(|filter| filter.id == *focused))
            .and_then(|index| enabled.iter().position(|enabled| *enabled == index))
            .unwrap_or_default();
        let next = (current as isize + delta).rem_euclid(enabled.len() as isize) as usize;
        let index = enabled[next];
        self.focused_filter_id = Some(self.filters[index].id.clone());
        self.filter_focus_engaged = true;
        self.filter_scroll.scroll_to_item(index.saturating_add(1));
        self.filter_focus.focus(window, cx);
        cx.notify();
    }

    fn reveal_focused_filter(&self) {
        let Some(index) = self.focused_filter_id.as_ref().and_then(|focused| {
            self.filters
                .iter()
                .position(|filter| filter.id == *focused && !filter.disabled)
        }) else {
            return;
        };
        // gpui-component's scrollbar layer occupies the first tracked child.
        self.filter_scroll.scroll_to_item(index.saturating_add(1));
    }

    fn handle_records_event(&mut self, event: &RecordsTableEvent, cx: &mut Context<Self>) {
        match event {
            RecordsTableEvent::SelectionRequested { row_id, .. } => {
                cx.emit(FilterTableEvent::SelectionRequested {
                    id: self.id.clone(),
                    row_id: row_id.clone(),
                });
            }
            RecordsTableEvent::ActivationRequested { row_id, .. } => {
                cx.emit(FilterTableEvent::ActivationRequested {
                    id: self.id.clone(),
                    row_id: row_id.clone(),
                });
            }
            RecordsTableEvent::SortRequested {
                column_id,
                direction,
                ..
            } => cx.emit(FilterTableEvent::SortRequested {
                id: self.id.clone(),
                column_id: column_id.clone(),
                direction: *direction,
            }),
        }
    }
}

impl EventEmitter<FilterTableEvent> for FilterTable {}

impl Focusable for FilterTable {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.records_table.focus_handle(cx)
    }
}

impl gpui::Styled for FilterTable {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for FilterTable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let result_label: SharedString = self.rows.content().len().to_string().into();
        let owner = cx.weak_entity();
        let navigation_owner = owner.clone();
        div()
            .id(scoped_filter_id("root", &self.id, "surface"))
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens.spacing.xs)
            .role(Role::Group)
            .aria_label(self.label.clone())
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex_none()
                    .child(
                        filter_controls_frame(&self.id, &self.filter_controls_label)
                            .w_full()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(tokens.spacing.xs)
                            .overflow_x_scroll()
                            .lock_scroll_axis()
                            .track_scroll(&self.filter_scroll)
                            .policy_horizontal_scrollbar(&self.filter_scroll, cx)
                            .children(self.filters.iter().map(|filter| {
                                let filter_id = filter.id.clone();
                                let handler_owner = owner.clone();
                                let is_focused =
                                    self.focused_filter_id.as_ref() == Some(&filter.id);
                                filter_control(
                                    &self.id,
                                    filter,
                                    &self.active_label,
                                    &self.inactive_label,
                                    window,
                                    cx,
                                )
                                    .tab_stop(is_focused)
                                    .when(is_focused, |button| {
                                        button.track_focus(&self.filter_focus)
                                    })
                                    .on_click(move |_, _, cx| {
                                        let _ = handler_owner.update(cx, |table, cx| {
                                            table.request_filter(filter_id.clone(), cx);
                                        });
                                    })
                            }))
                            .on_key_down(move |event, window, cx| {
                                let delta = match event.keystroke.key.as_str() {
                                    "left" => -1,
                                    "right" => 1,
                                    _ => return,
                                };
                                let _ = navigation_owner.update(cx, |table, cx| {
                                    table.move_filter_focus(delta, window, cx);
                                });
                                cx.stop_propagation();
                            }),
                    )
                    .child(
                        ScrollableMask::new(Axis::Horizontal, &self.filter_scroll)
                            .id((gpui::ElementId::from(self.id.clone()), "filter-scroll-mask")),
                    ),
            )
            .child(
                filter_results_frame(&self.id, result_label.clone())
                    .flex_none()
                    .text_token(tokens.typography.xs)
                    .text_color(cx.theme().muted_foreground)
                    .child(result_label),
            )
            .child(div().flex_1().min_h_0().child(self.records_table.clone()))
            .refine_style(&self.style)
    }
}

fn filter_controls_frame(table_id: &str, label: &str) -> Stateful<Div> {
    let debug_id = scoped_filter_id("controls", table_id, "filters");
    div()
        .id(debug_id.clone())
        .debug_selector(move || debug_id.to_string())
        .role(Role::Group)
        .aria_label(label.to_string())
}

fn filter_results_frame(table_id: &str, result_label: SharedString) -> Stateful<Div> {
    div()
        .id(scoped_filter_id("results", table_id, "status"))
        .role(Role::Status)
        .aria_label(result_label)
}

fn filter_control(
    table_id: &str,
    filter: &FilterDefinition,
    active_label: &str,
    inactive_label: &str,
    window: &mut Window,
    cx: &mut App,
) -> gpui_base::Button {
    let visible_label = format!("{} {}", filter.label, filter.count);
    let state = if filter.active {
        active_label
    } else {
        inactive_label
    };
    let accessibility_label = if filter.disabled {
        format!("Unavailable: {visible_label}, {state}")
    } else {
        format!("{visible_label}, {state}")
    };
    let debug_id = scoped_filter_id("filter", table_id, &filter.id);
    outlined_control_with_label(
        debug_id.clone(),
        accessibility_label,
        visible_label,
        window,
        cx,
    )
    .debug_selector(move || debug_id.to_string())
    .aria_toggled(if filter.active {
        accesskit::Toggled::True
    } else {
        accesskit::Toggled::False
    })
    .disabled(filter.disabled)
    .when(filter.active, |button| {
        button
            .bg(cx.theme().primary.opacity(0.12))
            .border_color(cx.theme().primary)
            .text_color(cx.theme().primary)
    })
}

fn scoped_filter_id(kind: &str, table_id: &str, item_id: &str) -> SharedString {
    format!("filter-table-{kind}-{}:{table_id}{item_id}", table_id.len()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Element as _, IntoElement as _, RenderOnce as _, TestAppContext, canvas};
    use std::sync::{Arc, Mutex};

    type CapturedFilters = Arc<Mutex<Vec<(Option<Role>, accesskit::Node)>>>;

    struct FilterControlProbe {
        captured: CapturedFilters,
    }

    impl Render for FilterControlProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let captured = self.captured.clone();
            canvas(
                move |_, window, cx| {
                    let controls = [
                        filter_control(
                            "tasks",
                            &FilterDefinition::new("todo", "To do", 2).active(true),
                            "active",
                            "inactive",
                            window,
                            cx,
                        )
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element(),
                        filter_control(
                            "tasks",
                            &FilterDefinition::new("blocked", "Blocked", 0).disabled(true),
                            "active",
                            "inactive",
                            window,
                            cx,
                        )
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element(),
                    ];
                    let frames = [
                        filter_controls_frame("tasks", "Task board"),
                        filter_results_frame("tasks", "2 results".into()),
                    ];
                    let nodes = controls
                        .into_iter()
                        .map(|element| {
                            let role = element.a11y_role();
                            let mut node = accesskit::Node::new(Role::Unknown);
                            element.write_a11y_info(&mut node);
                            (role, node)
                        })
                        .chain(frames.into_iter().map(|element| {
                            let role = element.a11y_role();
                            let mut node = accesskit::Node::new(Role::Unknown);
                            element.write_a11y_info(&mut node);
                            (role, node)
                        }))
                        .collect();
                    *captured.lock().expect("capture mutex should be available") = nodes;
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn filter_controls_expose_name_state_and_available_action(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let captured = CapturedFilters::default();
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(|_, _| FilterControlProbe { captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let controls = result.lock().expect("capture mutex should be available");

        assert_eq!(controls[0].0, Some(Role::Button));
        assert_eq!(controls[0].1.label(), Some("To do 2, active"));
        assert_eq!(controls[0].1.toggled(), Some(accesskit::Toggled::True));
        assert!(controls[0].1.supports_action(accesskit::Action::Click));

        assert_eq!(controls[1].0, Some(Role::Button));
        assert_eq!(
            controls[1].1.label(),
            Some("Unavailable: Blocked 0, inactive")
        );
        assert_eq!(controls[1].1.toggled(), Some(accesskit::Toggled::False));
        assert!(!controls[1].1.supports_action(accesskit::Action::Click));

        assert_eq!(controls[2].0, Some(Role::Group));
        assert_eq!(controls[2].1.label(), Some("Task board"));
        assert_eq!(controls[3].0, Some(Role::Status));
        assert_eq!(controls[3].1.label(), Some("2 results"));
    }
}
