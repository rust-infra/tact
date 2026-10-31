//! Controlled before/after proposals for tabular data.

use gpui_base::StyledExt as _;
use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement as _,
    ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, text::TextView};

use crate::{
    control::outlined_control_with_label,
    records_table::{
        RecordCell, RecordCellProvider, RecordColumn, RecordColumnAlignment, RecordRow,
        RecordSortDirection, RecordStatusTone, RecordsTable, RecordsTableEvent,
        escape_markdown_text, record_columns_have_unique_ids,
    },
    stream::{ProgressState, Progressive},
};

/// A stable, configurable column shared with the virtualized records adapter.
pub type DiffColumn = RecordColumn;

/// Horizontal alignment for one diff column.
pub type DiffColumnAlignment = RecordColumnAlignment;

/// Sort direction requested for a diff column.
pub type DiffSortDirection = RecordSortDirection;

/// The semantic relationship between a proposed value and its prior value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffChangeKind {
    /// A value exists only in the proposed snapshot.
    Added,
    /// A value exists only in the prior snapshot.
    Removed,
    /// Both snapshots contain different values.
    Changed,
    /// Both snapshots contain the same value.
    Unchanged,
}

/// An application-owned proposal decision displayed by a diff row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffProposalState {
    /// The proposal has not been decided.
    Pending,
    /// The application accepted the proposal.
    Accepted,
    /// The application rejected the proposal.
    Rejected,
}

/// A decision intent emitted for an identified proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffProposalAction {
    /// Request acceptance of the proposal.
    Accept,
    /// Request rejection of the proposal.
    Reject,
}

/// One immutable before/after value associated with a stable column ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffCell {
    column_id: SharedString,
    before: Option<SharedString>,
    after: Option<SharedString>,
    change_kind: DiffChangeKind,
}

impl DiffCell {
    /// Creates a value that exists only in the proposed snapshot.
    pub fn added(column_id: impl Into<SharedString>, after: impl Into<SharedString>) -> Self {
        Self {
            column_id: column_id.into(),
            before: None,
            after: Some(after.into()),
            change_kind: DiffChangeKind::Added,
        }
    }

    /// Creates a value that exists only in the prior snapshot.
    pub fn removed(column_id: impl Into<SharedString>, before: impl Into<SharedString>) -> Self {
        Self {
            column_id: column_id.into(),
            before: Some(before.into()),
            after: None,
            change_kind: DiffChangeKind::Removed,
        }
    }

    /// Creates a value that differs between the prior and proposed snapshots.
    pub fn changed(
        column_id: impl Into<SharedString>,
        before: impl Into<SharedString>,
        after: impl Into<SharedString>,
    ) -> Self {
        Self {
            column_id: column_id.into(),
            before: Some(before.into()),
            after: Some(after.into()),
            change_kind: DiffChangeKind::Changed,
        }
    }

    /// Creates a value that is identical in both snapshots.
    pub fn unchanged(column_id: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        let value = value.into();
        Self {
            column_id: column_id.into(),
            before: Some(value.clone()),
            after: Some(value),
            change_kind: DiffChangeKind::Unchanged,
        }
    }

    /// Returns the stable column ID associated with this value.
    pub fn column_id(&self) -> &str {
        self.column_id.as_ref()
    }

    /// Returns the prior readable value, when one exists.
    pub fn before(&self) -> Option<&str> {
        self.before.as_deref()
    }

    /// Returns the proposed readable value, when one exists.
    pub fn after(&self) -> Option<&str> {
        self.after.as_deref()
    }

    /// Returns the semantic relationship between the two snapshots.
    pub fn change_kind(&self) -> DiffChangeKind {
        self.change_kind
    }
}

/// One immutable proposal row keyed by a stable application ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRow {
    id: SharedString,
    label: SharedString,
    change_kind: DiffChangeKind,
    cells: Arc<[DiffCell]>,
    proposal_state: DiffProposalState,
    disabled: bool,
}

impl DiffRow {
    /// Creates an empty proposal row with stable identity and semantic change kind.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        change_kind: DiffChangeKind,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            change_kind,
            cells: Arc::from([]),
            proposal_state: DiffProposalState::Pending,
            disabled: false,
        }
    }

    /// Replaces the immutable before/after cell snapshot.
    pub fn cells(mut self, cells: impl IntoIterator<Item = DiffCell>) -> Self {
        self.cells = cells.into_iter().collect::<Vec<_>>().into();
        self
    }

    /// Sets the application-owned proposal decision shown by this row.
    pub fn state(mut self, proposal_state: DiffProposalState) -> Self {
        self.proposal_state = proposal_state;
        self
    }

    /// Sets whether the proposal rejects selection and action intent.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable application proposal ID.
    pub fn id(&self) -> &str {
        self.id.as_ref()
    }

    /// Returns the accessible row label.
    pub fn label(&self) -> &str {
        self.label.as_ref()
    }

    /// Returns the row-level semantic change kind.
    pub fn change_kind(&self) -> DiffChangeKind {
        self.change_kind
    }

    /// Returns the value associated with `column_id`, if present.
    pub fn cell(&self, column_id: &str) -> Option<&DiffCell> {
        self.cells
            .iter()
            .find(|cell| cell.column_id.as_ref() == column_id)
    }

    /// Returns the immutable cell snapshot in consumer order.
    pub fn all_cells(&self) -> &[DiffCell] {
        &self.cells
    }

    /// Returns the application-owned proposal decision.
    pub fn proposal_state(&self) -> DiffProposalState {
        self.proposal_state
    }

    /// Returns whether the proposal rejects selection and action intent.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

fn diff_rows_have_unique_ids(rows: &[DiffRow]) -> bool {
    let mut row_ids = HashSet::with_capacity(rows.len());
    rows.iter().all(|row| {
        let mut cell_ids = HashSet::with_capacity(row.cells.len());
        row_ids.insert(row.id())
            && row
                .cells
                .iter()
                .all(|cell| cell_ids.insert(cell.column_id()))
    })
}

/// Typed application intent emitted by [`DiffTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiffTableEvent {
    /// Requests that the application select the identified proposal.
    SelectionRequested {
        /// Stable diff-table ID.
        id: SharedString,
        /// Stable application proposal ID.
        row_id: SharedString,
    },
    /// Requests that the application reveal or inspect a proposal.
    ReviewRequested {
        /// Stable diff-table ID.
        id: SharedString,
        /// Stable application proposal ID.
        row_id: SharedString,
    },
    /// Requests a controlled sort projection.
    SortRequested {
        /// Stable diff-table ID.
        id: SharedString,
        /// Stable application column ID.
        column_id: SharedString,
        /// Requested direction, or `None` to clear sorting.
        direction: Option<DiffSortDirection>,
    },
    /// Requests a controlled decision for an identified proposal.
    DecisionRequested {
        /// Stable diff-table ID.
        id: SharedString,
        /// Stable application proposal ID.
        row_id: SharedString,
        /// Requested accept or reject intent.
        action: DiffProposalAction,
    },
}

/// A controlled, virtualized table of proposed before/after values.
///
/// Applications own columns, proposals, progress, sorting, selection, and
/// proposal decisions. The entity composes [`RecordsTable`] for focus,
/// virtualization, and scrolling, and retains no application work.
pub struct DiffTable {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    label: SharedString,
    columns: Arc<[DiffColumn]>,
    rows: Progressive<Arc<[DiffRow]>>,
    selected_row_id: Option<SharedString>,
    sort_column_id: Option<SharedString>,
    sort_direction: Option<DiffSortDirection>,
    review_column_id: SharedString,
    decision_column_id: SharedString,
    projected_cells: Arc<AtomicUsize>,
    records_table: gpui::Entity<RecordsTable>,
    _records_subscription: Subscription,
}

impl DiffTable {
    /// Creates an empty diff table with a stable ID and accessible label.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = id.into();
        let label = label.into();
        let records_id = scoped_diff_id("records", &id, "table");
        let review_column_id = scoped_diff_id("review-column", &id, "action");
        let decision_column_id = scoped_diff_id("decision-column", &id, "state");
        let records_label = label.clone();
        let records_table = cx.new(|cx| {
            let mut table = RecordsTable::new(records_id, records_label, window, cx);
            table.set_activation_label("Review", cx);
            table
        });
        let records_subscription = cx.subscribe(&records_table, |this, _, event, cx| {
            this.handle_records_event(event, cx);
        });

        Self {
            style: gpui::StyleRefinement::default(),
            id,
            label,
            columns: Arc::from([]),
            rows: Progressive::pending(Arc::from([])),
            selected_row_id: None,
            sort_column_id: None,
            sort_direction: None,
            review_column_id,
            decision_column_id,
            projected_cells: Arc::new(AtomicUsize::new(0)),
            records_table,
            _records_subscription: records_subscription,
        }
    }

    /// Replaces the controlled column snapshot without rebuilding table state.
    ///
    /// A snapshot containing duplicate stable column IDs is ignored atomically.
    pub fn set_columns(
        &mut self,
        columns: impl IntoIterator<Item = DiffColumn>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let columns = columns.into_iter().collect::<Vec<_>>();
        // The injected columns carry authored widths: the upstream default
        // (100px) truncated "Pending" into "Pending dec…".
        let mut record_columns = vec![
            RecordColumn::new(self.review_column_id.clone(), "Review")
                .fixed(true)
                .width_in_rems(6.5),
            RecordColumn::new(self.decision_column_id.clone(), "Decision")
                .fixed(true)
                .width_in_rems(8.),
        ];
        record_columns.extend(columns.iter().cloned());
        if !record_columns_have_unique_ids(&record_columns) {
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
        self.records_table.update(cx, |table, cx| {
            table.set_columns(record_columns, window, cx);
        });
        cx.notify();
    }

    /// Replaces the controlled progressive proposal snapshot.
    ///
    /// Duplicate proposal IDs or duplicate cell column IDs make the complete
    /// replacement invalid, so the prior controlled snapshot is retained.
    pub fn set_rows(
        &mut self,
        rows: Progressive<Arc<[DiffRow]>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !diff_rows_have_unique_ids(rows.content()) {
            return;
        }
        self.rows = rows;
        if self.selected_row_id.as_ref().is_some_and(|selected| {
            !self
                .rows
                .content()
                .iter()
                .any(|row| row.id == *selected && !row.disabled)
        }) {
            self.selected_row_id = None;
        }
        self.projected_cells.store(0, Ordering::Relaxed);
        let records = diff_record_skeletons(&self.rows);
        let provider: Arc<dyn RecordCellProvider> = Arc::new(DiffCellProvider {
            rows: self.rows.content().clone(),
            decision_column_id: self.decision_column_id.clone(),
            projected_cells: self.projected_cells.clone(),
        });
        self.records_table.update(cx, |table, cx| {
            table.set_records_snapshot_with_cell_provider(records, Some(provider), cx);
        });
        cx.notify();
    }

    #[cfg(test)]
    fn projected_cell_count(&self) -> usize {
        self.projected_cells.load(Ordering::Relaxed)
    }

    /// Replaces the controlled selected proposal when the ID is enabled.
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
            .any(|row| row.id == row_id && !row.disabled)
        {
            return;
        }
        self.selected_row_id = Some(row_id.clone());
        self.records_table.update(cx, |table, cx| {
            table.set_selected_row(row_id, window, cx);
        });
        cx.notify();
    }

    /// Clears the controlled selected proposal.
    pub fn clear_selected_row(&mut self, cx: &mut Context<Self>) {
        self.selected_row_id = None;
        self.records_table
            .update(cx, |table, cx| table.clear_selected_row(cx));
        cx.notify();
    }

    /// Returns the controlled selected proposal ID.
    pub fn selected_row_id(&self) -> Option<&str> {
        self.selected_row_id.as_deref()
    }

    /// Replaces the controlled sort snapshot.
    pub fn set_sort(
        &mut self,
        column_id: impl Into<SharedString>,
        direction: Option<DiffSortDirection>,
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
    pub fn sort(&self) -> Option<(&str, DiffSortDirection)> {
        self.sort_column_id.as_deref().zip(self.sort_direction)
    }

    /// Scrolls the identified proposal into view when it exists.
    pub fn scroll_to_row(&mut self, row_id: &str, cx: &mut Context<Self>) {
        self.records_table
            .update(cx, |table, cx| table.scroll_to_row(row_id, cx));
    }

    /// Scrolls the identified column into view when it exists.
    pub fn scroll_to_column(&mut self, column_id: &str, cx: &mut Context<Self>) {
        self.records_table
            .update(cx, |table, cx| table.scroll_to_column(column_id, cx));
    }

    /// Moves keyboard focus to the virtualized diff grid.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.records_table.focus_handle(cx).focus(window, cx);
    }

    fn handle_records_event(&mut self, event: &RecordsTableEvent, cx: &mut Context<Self>) {
        match event {
            RecordsTableEvent::SelectionRequested { row_id, .. } => {
                cx.emit(DiffTableEvent::SelectionRequested {
                    id: self.id.clone(),
                    row_id: row_id.clone(),
                });
            }
            RecordsTableEvent::ActivationRequested { row_id, .. } => {
                cx.emit(DiffTableEvent::ReviewRequested {
                    id: self.id.clone(),
                    row_id: row_id.clone(),
                });
            }
            RecordsTableEvent::SortRequested {
                column_id,
                direction,
                ..
            } => {
                cx.emit(DiffTableEvent::SortRequested {
                    id: self.id.clone(),
                    column_id: column_id.clone(),
                    direction: *direction,
                });
            }
        }
    }

    fn request_decision(
        &mut self,
        row_id: SharedString,
        action: DiffProposalAction,
        cx: &mut Context<Self>,
    ) {
        if self
            .rows
            .content()
            .iter()
            .any(|row| row.id == row_id && !row.disabled)
        {
            cx.emit(DiffTableEvent::DecisionRequested {
                id: self.id.clone(),
                row_id,
                action,
            });
        }
    }
}

impl EventEmitter<DiffTableEvent> for DiffTable {}

impl Focusable for DiffTable {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.records_table.focus_handle(cx)
    }
}

impl gpui::Styled for DiffTable {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for DiffTable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let selected = self.selected_row_id.as_ref().and_then(|selected| {
            self.rows
                .content()
                .iter()
                .find(|row| row.id == *selected)
                .cloned()
        });
        let tokens = cx.theme().semantic_tokens();

        div()
            .id(scoped_diff_id("root", &self.id, "surface"))
            .debug_selector({
                let id = self.id.clone();
                move || scoped_diff_id("root", &id, "surface").to_string()
            })
            .size_full()
            .min_h_0()
            .flex()
            .flex_col()
            .gap(tokens.spacing.xs)
            .role(Role::Group)
            .aria_label(self.label.clone())
            .child(div().flex_1().min_h_0().child(self.records_table.clone()))
            .when_some(selected, |this, row| {
                let row_id = row.id.clone();
                let accept_owner = cx.weak_entity();
                let reject_owner = accept_owner.clone();
                let state = proposal_state_label(row.proposal_state);
                let actions_label = format!("Proposal actions for {}. {state}", row.label);
                this.child(
                    div()
                        .id(scoped_diff_id("actions", &self.id, &row.id))
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(tokens.spacing.sm)
                        .role(Role::Group)
                        .aria_label(actions_label.clone())
                        .child(
                            TextView::markdown(
                                scoped_diff_id("decision", &self.id, &row.id),
                                escape_markdown_text(&actions_label),
                            )
                            .selectable(true),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(tokens.spacing.xs)
                                .child(
                                    proposal_action_control(
                                        &self.id,
                                        &row,
                                        DiffProposalAction::Accept,
                                        window,
                                        cx,
                                    )
                                    .on_click({
                                        let row_id = row_id.clone();
                                        move |_, _, cx| {
                                            let _ = accept_owner.update(cx, |table, cx| {
                                                table.request_decision(
                                                    row_id.clone(),
                                                    DiffProposalAction::Accept,
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                                )
                                .child(
                                    proposal_action_control(
                                        &self.id,
                                        &row,
                                        DiffProposalAction::Reject,
                                        window,
                                        cx,
                                    )
                                    .on_click(
                                        move |_, _, cx| {
                                            let _ = reject_owner.update(cx, |table, cx| {
                                                table.request_decision(
                                                    row_id.clone(),
                                                    DiffProposalAction::Reject,
                                                    cx,
                                                );
                                            });
                                        },
                                    ),
                                ),
                        ),
                )
            })
            .refine_style(&self.style)
    }
}

struct DiffCellProvider {
    rows: Arc<[DiffRow]>,
    decision_column_id: SharedString,
    projected_cells: Arc<AtomicUsize>,
}

impl RecordCellProvider for DiffCellProvider {
    fn cell(&self, row_ix: usize, column_id: &str) -> Option<RecordCell> {
        let row = self.rows.get(row_ix)?;
        let cell = if column_id == self.decision_column_id.as_ref() {
            Some(RecordCell::status(
                self.decision_column_id.clone(),
                proposal_state_label(row.proposal_state),
                proposal_state_tone(row.proposal_state),
            ))
        } else {
            row.cell(column_id).map(diff_record_cell)
        };
        if cell.is_some() {
            self.projected_cells.fetch_add(1, Ordering::Relaxed);
        }
        cell
    }
}

fn diff_record_skeletons(rows: &Progressive<Arc<[DiffRow]>>) -> Progressive<Arc<[RecordRow]>> {
    let records: Arc<[RecordRow]> = rows
        .content()
        .iter()
        .map(diff_record_skeleton)
        .collect::<Vec<_>>()
        .into();
    match rows.state() {
        ProgressState::Pending => Progressive::pending(records),
        ProgressState::Running => Progressive::running(records),
        ProgressState::Complete => Progressive::complete(records),
        ProgressState::Failed(reason) => Progressive::failed(records, reason.clone()),
    }
}

fn diff_record_skeleton(row: &DiffRow) -> RecordRow {
    let semantic_label = format!(
        "{}; {}; {}",
        row.label,
        change_kind_label(row.change_kind),
        proposal_state_label(row.proposal_state)
    );
    RecordRow::new(row.id.clone(), semantic_label).disabled(row.disabled)
}

#[cfg(test)]
fn diff_record_row(row: &DiffRow, decision_column_id: &SharedString) -> RecordRow {
    let cells = row
        .cells
        .iter()
        .map(diff_record_cell)
        .chain([RecordCell::status(
            decision_column_id.clone(),
            proposal_state_label(row.proposal_state),
            proposal_state_tone(row.proposal_state),
        )]);
    diff_record_skeleton(row).cells(cells)
}

fn diff_record_cell(cell: &DiffCell) -> RecordCell {
    // Change semantics travel as data — the records renderer draws the
    // tone glyph, the struck previous value, and the new one — and an
    // unchanged cell is simply its value: the prose prefixes that wrapped
    // and clipped ("Changed: Classic → Seasonal") are gone, and three
    // quiet rows no longer shout "Unchanged" four times each.
    match cell.change_kind {
        DiffChangeKind::Added => RecordCell::change(
            cell.column_id.clone(),
            None,
            cell.after.clone(),
            RecordStatusTone::Positive,
        ),
        DiffChangeKind::Removed => RecordCell::change(
            cell.column_id.clone(),
            cell.before.clone(),
            None,
            RecordStatusTone::Critical,
        ),
        DiffChangeKind::Changed => RecordCell::change(
            cell.column_id.clone(),
            cell.before.clone(),
            cell.after.clone(),
            RecordStatusTone::Caution,
        ),
        DiffChangeKind::Unchanged => RecordCell::new(
            cell.column_id.clone(),
            cell.after.clone().unwrap_or_default(),
        ),
    }
}

fn change_kind_label(kind: DiffChangeKind) -> &'static str {
    match kind {
        DiffChangeKind::Added => "Added",
        DiffChangeKind::Removed => "Removed",
        DiffChangeKind::Changed => "Changed",
        DiffChangeKind::Unchanged => "Unchanged",
    }
}

fn proposal_state_label(state: DiffProposalState) -> &'static str {
    match state {
        DiffProposalState::Pending => "Pending",
        DiffProposalState::Accepted => "Accepted",
        DiffProposalState::Rejected => "Rejected",
    }
}

fn proposal_state_tone(state: DiffProposalState) -> RecordStatusTone {
    match state {
        DiffProposalState::Pending => RecordStatusTone::Caution,
        DiffProposalState::Accepted => RecordStatusTone::Positive,
        DiffProposalState::Rejected => RecordStatusTone::Critical,
    }
}

fn proposal_action_control(
    table_id: &str,
    row: &DiffRow,
    action: DiffProposalAction,
    window: &mut Window,
    cx: &mut App,
) -> gpui_base::Button {
    let (kind, verb, decided_label, decided) = match action {
        DiffProposalAction::Accept => (
            "accept",
            "Accept",
            "accepted",
            row.proposal_state == DiffProposalState::Accepted,
        ),
        DiffProposalAction::Reject => (
            "reject",
            "Reject",
            "rejected",
            row.proposal_state == DiffProposalState::Rejected,
        ),
    };
    let disabled = row.disabled || decided;
    let accessibility_label = if row.disabled {
        format!("Unavailable: {verb} {}", row.label)
    } else if decided {
        format!("Already {decided_label}: {}", row.label)
    } else {
        format!("{verb} {}", row.label)
    };
    let debug_id = scoped_diff_id(kind, table_id, &row.id);
    outlined_control_with_label(debug_id.clone(), accessibility_label, verb, window, cx)
        .debug_selector(move || debug_id.to_string())
        .disabled(disabled)
}

fn scoped_diff_id(kind: &str, table_id: &str, item_id: &str) -> SharedString {
    format!("diff-table-{kind}-{}:{table_id}{item_id}", table_id.len()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Element as _, IntoElement as _, RenderOnce as _, TestAppContext, VisualTestContext,
        accesskit, canvas, px, size,
    };
    use std::sync::{Arc, Mutex};

    #[test]
    fn diff_projection_keeps_change_state_in_readable_cell_values() {
        // Change semantics are data, not prose: the accessible value reads
        // as a sentence, the tone carries the kind, and an unchanged cell
        // is simply its value with no tone at all.
        for (cell, accessible, tone) in [
            (
                DiffCell::added("value", "Pistachio"),
                "added Pistachio",
                Some(RecordStatusTone::Positive),
            ),
            (
                DiffCell::removed("value", "Bubblegum"),
                "removed Bubblegum",
                Some(RecordStatusTone::Critical),
            ),
            (
                DiffCell::changed("value", "Mint Chip", "Pistachio"),
                "was Mint Chip, now Pistachio",
                Some(RecordStatusTone::Caution),
            ),
            (DiffCell::unchanged("value", "Classic"), "Classic", None),
        ] {
            let projected = diff_record_cell(&cell);
            assert_eq!(projected.accessible_value().as_ref(), accessible);
            assert_eq!(projected.status_tone(), tone);
        }

        let decision_column_id: SharedString = "decision".into();
        for (state, expected, tone) in [
            (
                DiffProposalState::Pending,
                "Pending",
                RecordStatusTone::Caution,
            ),
            (
                DiffProposalState::Accepted,
                "Accepted",
                RecordStatusTone::Positive,
            ),
            (
                DiffProposalState::Rejected,
                "Rejected",
                RecordStatusTone::Critical,
            ),
        ] {
            let row = DiffRow::new("proposal", "Proposal", DiffChangeKind::Changed).state(state);
            let projected = diff_record_row(&row, &decision_column_id);
            let decision = projected
                .cell("decision")
                .expect("every projected row should expose a visible decision cell");
            assert_eq!(decision.value(), expected);
            assert_eq!(decision.status_tone(), Some(tone));
        }
    }

    #[gpui::test]
    fn thousand_row_diff_projects_cells_only_for_the_virtual_viewport(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (table, cx) =
            cx.add_window_view(|window, cx| DiffTable::new("lazy", "Lazy diff", window, cx));
        let cx: &mut VisualTestContext = cx;
        cx.simulate_resize(size(px(640.), px(300.)));
        let columns = (0..8)
            .map(|index| DiffColumn::new(format!("column-{index}"), format!("Column {index}")))
            .collect::<Vec<_>>();
        let rows = (0..1_000)
            .map(|row| {
                DiffRow::new(
                    format!("row-{row}"),
                    format!("Row {row}"),
                    DiffChangeKind::Changed,
                )
                .cells((0..8).map(|column| {
                    DiffCell::changed(
                        format!("column-{column}"),
                        format!("Before {row}:{column}"),
                        format!("After {row}:{column}"),
                    )
                }))
            })
            .collect::<Vec<_>>();

        cx.update(|window, cx| {
            table.update(cx, |table, cx| {
                table.set_columns(columns, window, cx);
                table.set_rows(Progressive::complete(rows.into()), window, cx);
                assert_eq!(table.projected_cell_count(), 0);
            });
            window.draw(cx).clear(cx);
        });
        let projected = table.read_with(cx, |table, _| table.projected_cell_count());
        assert!(
            projected > 0,
            "visible diff cells should be projected on demand"
        );
        assert!(
            projected < 256,
            "offscreen diff cells must not be projected eagerly; got {projected}"
        );
    }

    #[gpui::test]
    fn deciding_a_proposal_acknowledges_and_a_loaded_diff_is_settled(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (table, cx) =
            cx.add_window_view(|window, cx| DiffTable::new("decide", "Decide diff", window, cx));
        let cx: &mut VisualTestContext = cx;
        cx.simulate_resize(size(px(640.), px(300.)));
        let rows = |state: DiffProposalState| {
            vec![
                DiffRow::new("keep", "Keep", DiffChangeKind::Changed)
                    .cells([DiffCell::changed("value", "Before", "After")]),
                DiffRow::new("decide", "Decide", DiffChangeKind::Changed)
                    .state(state)
                    .cells([DiffCell::changed("value", "Old", "New")]),
            ]
        };
        cx.update(|window, cx| {
            table.update(cx, |table, cx| {
                table.set_columns([DiffColumn::new("value", "Value")], window, cx);
                table.set_rows(
                    Progressive::complete(rows(DiffProposalState::Pending).into()),
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        crate::motion::take_reveal_frame_requests();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a loaded diff presents its decisions settled"
        );

        cx.update(|window, cx| {
            table.update(cx, |table, cx| {
                table.set_rows(
                    Progressive::complete(rows(DiffProposalState::Accepted).into()),
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "an accepted proposal must acknowledge its new decision"
        );
    }

    type CapturedControls = Arc<Mutex<Vec<(Option<Role>, accesskit::Node)>>>;

    struct DiffControlProbe {
        captured: CapturedControls,
    }

    impl Render for DiffControlProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let captured = self.captured.clone();
            canvas(
                move |_, window, cx| {
                    let pending = DiffRow::new("pending", "Pistachio", DiffChangeKind::Changed);
                    let accepted = DiffRow::new("accepted", "Rocky Road", DiffChangeKind::Removed)
                        .state(DiffProposalState::Accepted);
                    let rejected = DiffRow::new("rejected", "Bubblegum", DiffChangeKind::Removed)
                        .state(DiffProposalState::Rejected);
                    let controls = [
                        proposal_action_control(
                            "cleanup",
                            &pending,
                            DiffProposalAction::Accept,
                            window,
                            cx,
                        )
                        .on_click(|_, _, _| {}),
                        proposal_action_control(
                            "cleanup",
                            &pending,
                            DiffProposalAction::Reject,
                            window,
                            cx,
                        )
                        .on_click(|_, _, _| {}),
                        proposal_action_control(
                            "cleanup",
                            &accepted,
                            DiffProposalAction::Accept,
                            window,
                            cx,
                        )
                        .on_click(|_, _, _| {}),
                        proposal_action_control(
                            "cleanup",
                            &rejected,
                            DiffProposalAction::Reject,
                            window,
                            cx,
                        )
                        .on_click(|_, _, _| {}),
                    ];
                    let mut result = Vec::new();
                    for control in controls {
                        let element = control.render(window, cx).into_element();
                        let role = element.a11y_role();
                        let mut node = accesskit::Node::new(Role::Unknown);
                        element.write_a11y_info(&mut node);
                        result.push((role, node));
                    }
                    *captured.lock().expect("capture mutex should be available") = result;
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn proposal_controls_expose_names_roles_and_available_actions(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(Vec::new()));
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(move |_, _| DiffControlProbe { captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let controls = result.lock().expect("capture mutex should be available");
        assert_eq!(controls.len(), 4);
        assert_eq!(controls[0].0, Some(Role::Button));
        assert_eq!(controls[0].1.label(), Some("Accept Pistachio"));
        assert!(controls[0].1.supports_action(accesskit::Action::Click));
        assert_eq!(controls[1].0, Some(Role::Button));
        assert_eq!(controls[1].1.label(), Some("Reject Pistachio"));
        assert!(controls[1].1.supports_action(accesskit::Action::Click));
        assert_eq!(controls[2].0, Some(Role::Button));
        assert_eq!(controls[2].1.label(), Some("Already accepted: Rocky Road"));
        assert!(!controls[2].1.supports_action(accesskit::Action::Click));
        assert_eq!(controls[3].0, Some(Role::Button));
        assert_eq!(controls[3].1.label(), Some("Already rejected: Bubblegum"));
        assert!(!controls[3].1.supports_action(accesskit::Action::Click));
    }
}
