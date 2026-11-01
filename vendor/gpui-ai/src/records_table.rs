//! Controlled record-grid values and presentation.

use gpui_base::StyledExt as _;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use gpui::{
    AnyElement, App, AppContext as _, Context, Div, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement as _, KeyBinding, ParentElement as _, Pixels, Rems,
    Render, Role, SharedString, Stateful, StatefulInteractiveElement as _, Styled as _,
    Subscription, Window, div, prelude::FluentBuilder as _, rems,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Size,
    spinner::Spinner,
    table::{DataTable, TableEvent, TableState},
    text::TextView,
};

use crate::{
    motion::Shimmer,
    resolved_layout::ResolvedLayoutKey,
    stream::{ProgressState, Progressive},
};

mod delegate;
mod reorder;
mod stable_id;

use delegate::RecordsDelegate;
use stable_id::{StableIdIndex, index_valid_columns, index_valid_rows};

const RECORDS_TABLE_CONTEXT: &str = "GpuiAiRecordsTable";
gpui::actions!(
    gpui_ai_records_table,
    [
        /// Activates the consumer-controlled selected record.
        ActivateRecord
    ]
);

pub(crate) fn init(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("enter", ActivateRecord, Some(RECORDS_TABLE_CONTEXT)),
        KeyBinding::new("space", ActivateRecord, Some(RECORDS_TABLE_CONTEXT)),
    ]);
}

/// Sort direction requested for a record column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordSortDirection {
    /// Sort values from low to high.
    Ascending,
    /// Sort values from high to low.
    Descending,
}

/// When a row's activation control is visible.
///
/// The default reveals it on the row's hover — and always on the
/// control's own keyboard focus, so reachability never depends on the
/// pointer. `Always` keeps it painted for consumers whose rows are
/// action-first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowActionVisibility {
    /// Painted on every row, all the time.
    Always,
    /// Revealed by row hover and by the control's keyboard focus.
    #[default]
    OnHover,
}

/// Where a row's activation control sits in its cell.
///
/// The default aligns it to the trailing edge, so the actions rail down
/// the column; `Inline` keeps it beside the content for rows whose
/// action reads as part of the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RowActionPlacement {
    /// Immediately after the cell's content.
    Inline,
    /// Right-aligned at the trailing edge of the cell.
    #[default]
    End,
}

/// Horizontal alignment for one records column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordColumnAlignment {
    /// Align content to the leading edge.
    #[default]
    Left,
    /// Center content in the column.
    Center,
    /// Align content to the trailing edge.
    Right,
}

/// Visual structure of one record cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RecordCellKind {
    /// A change from one value to another: a tone glyph beside the struck
    /// previous value, an arrow, and the new value.
    Change,
    /// Ordinary readable text.
    #[default]
    Text,
    /// A set of compact categorical tags.
    Tags,
    /// A named semantic status.
    Status,
}

/// Semantic emphasis for a status cell.
///
/// The readable status text always carries meaning; tone only reinforces it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordStatusTone {
    /// Positive or healthy status.
    Positive,
    /// Informational status without positive or negative meaning.
    Neutral,
    /// Status that deserves attention.
    Caution,
    /// Critical or failed status.
    Critical,
}

/// Typed application intent emitted by [`RecordsTable`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordsTableEvent {
    /// Requests that the application select the identified row.
    SelectionRequested {
        /// Stable records-table ID.
        id: SharedString,
        /// Stable application row ID.
        row_id: SharedString,
    },
    /// Requests the row's primary application action.
    ActivationRequested {
        /// Stable records-table ID.
        id: SharedString,
        /// Stable application row ID.
        row_id: SharedString,
    },
    /// Requests a controlled sort projection.
    SortRequested {
        /// Stable records-table ID.
        id: SharedString,
        /// Stable application column ID.
        column_id: SharedString,
        /// Requested direction, or `None` to clear sorting.
        direction: Option<RecordSortDirection>,
    },
}

/// How wide a column asked to be, in whichever unit its caller chose.
///
/// Pixels stay pixels: a caller who measured something owns that number. A rem
/// width is a multiple of the reader's base type size and becomes pixels only
/// at layout, so a column sized for its text keeps that proportion when the
/// reader zooms. Private because the choice is the column's own business —
/// what a table consumes is the resolved width.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum RecordColumnWidth {
    /// No configured width; the table's own default applies.
    #[default]
    Unset,
    /// A device-independent width that does not follow the type scale.
    Pixels(Pixels),
    /// A width in multiples of the window's rem.
    Rems(Rems),
}

impl RecordColumnWidth {
    /// The pixel width for `rem_size`, or `None` to leave the choice upstream.
    fn resolve(self, rem_size: Pixels) -> Option<Pixels> {
        match self {
            Self::Unset => None,
            Self::Pixels(width) => Some(width),
            Self::Rems(width) => Some(width.to_pixels(rem_size)),
        }
    }
}

/// How a table divides the width it is given.
///
/// A table is nearly always given more width than the columns asked for, and
/// what it does with the remainder is the difference between a table and a
/// picture of one. HTML settled this a long time ago: a table fills its box,
/// specified widths are honoured, and the surplus goes to whoever will take
/// it. This is that rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColumnFit {
    /// Columns divide the table's whole width; the default.
    ///
    /// The surplus over what the columns asked for goes to the columns that
    /// asked to grow with [`RecordColumn::fill`], or - when none did - to all
    /// of them in proportion to the widths they already have. Either way the
    /// table ends where its frame ends rather than leaving a band of nothing
    /// beside the last column.
    #[default]
    Fill,
    /// Columns keep exactly the widths they were given.
    ///
    /// The table ends where its columns end, and the reader may drag a column
    /// edge: a width the table computes cannot also be a width the reader
    /// chooses, so dragging belongs to this mode alone.
    Intrinsic,
}

/// Measures the box the grid is drawn in, so its columns can divide it.
///
/// A canvas rather than a wrapper, so nothing is added to the layout: it
/// reports the box it was placed in and draws nothing. The owning table
/// ignores a width it has already seen, so the frame this asks for on a resize
/// is one frame, not every frame.
fn measured_grid_width(owner: gpui::WeakEntity<RecordsTable>) -> impl gpui::IntoElement {
    gpui::canvas(
        move |bounds, _, cx| {
            let _ = owner.update(cx, |table, cx| table.observe_width(bounds.size.width, cx));
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

/// Every column's pixel width, for a table with this much room.
///
/// The rule, in three lines: a column's base is the width it asked for; the
/// surplus over the sum of those bases belongs to the columns that asked to
/// grow, or to all of them in proportion when none did; and a table with no
/// surplus is a table whose columns are exactly their bases, which is where
/// the horizontal scroll takes over.
///
/// `available` is the room the columns may occupy, which is not the whole of
/// the box the grid was given: see [`RecordsTable::push_column_widths`] for
/// what comes off it first.
///
/// Nothing here can shrink a column, so nothing here can collapse one. That is
/// the whole reason the floor needs no separate setting - the collapse other
/// libraries guard against with a minimum width is not reachable from here.
fn resolve_column_widths(
    columns: &[RecordColumn],
    fit: ColumnFit,
    available: Option<Pixels>,
    rem_size: Pixels,
) -> Arc<[Pixels]> {
    let bases = columns
        .iter()
        .map(|column| {
            column
                .width
                .resolve(rem_size)
                .unwrap_or_else(default_column_width)
        })
        .collect::<Vec<_>>();
    let (ColumnFit::Fill, Some(available)) = (fit, available) else {
        return bases.into();
    };
    let asked_for = bases.iter().fold(Pixels::ZERO, |total, base| total + *base);
    let surplus = available - asked_for;
    if surplus <= Pixels::ZERO {
        return bases.into();
    }
    // Whoever asked, or everyone in proportion. A share of zero is a column
    // saying it will stay where it is while its neighbours grow.
    let asked = columns.iter().any(|column| column.fill.is_some());
    let shares = columns
        .iter()
        .zip(&bases)
        .map(|(column, base)| match (asked, column.fill) {
            (true, Some(share)) => share,
            (true, None) => 0.,
            (false, _) => f32::from(*base),
        })
        .collect::<Vec<_>>();
    let total: f32 = shares.iter().sum();
    if total <= 0. {
        return bases.into();
    }
    bases
        .iter()
        .zip(&shares)
        .map(|(base, share)| *base + surplus * (share / total))
        .collect()
}

/// The width a column that named none is given.
///
/// Read from upstream rather than restated, so the width a column falls back to
/// here and the width the grid would have drawn cannot drift apart.
fn default_column_width() -> Pixels {
    gpui_component::table::Column::default().width
}

/// One stable column in a [`RecordsTable`].
///
/// Visible labels need not be unique. Events and row lookup always use the
/// application-supplied column ID.
#[derive(Debug, Clone, PartialEq)]
pub struct RecordColumn {
    id: SharedString,
    label: SharedString,
    sortable: bool,
    width: RecordColumnWidth,
    /// The share of the table's surplus width this column takes, when it asked
    /// for one. `None` is a column that did not ask.
    fill: Option<f32>,
    alignment: RecordColumnAlignment,
    fixed: bool,
    description: Option<SharedString>,
}

impl RecordColumn {
    /// Creates a column with a stable application ID and visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            sortable: false,
            width: RecordColumnWidth::Unset,
            fill: None,
            alignment: RecordColumnAlignment::Left,
            fixed: false,
            description: None,
        }
    }

    /// Enables or disables sorting for the column.
    pub fn sortable(mut self, sortable: bool) -> Self {
        self.sortable = sortable;
        self
    }

    /// Sets the logical column width.
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = RecordColumnWidth::Pixels(width);
        self
    }

    /// Sets the column width in multiples of the reader's base type size.
    ///
    /// The width resolves against the window's rem at layout time and is
    /// resolved again when the reader zooms, so a column sized for its text
    /// keeps that proportion; a width set with [`RecordColumn::width`] does
    /// not. Columns of one table may mix the two, and the later call wins.
    pub fn width_in_rems(mut self, width: f32) -> Self {
        self.width = RecordColumnWidth::Rems(rems(width));
        self
    }

    /// Takes a share of the width left over once every column has the width
    /// it asked for.
    ///
    /// The width set by [`RecordColumn::width`] is a floor rather than a
    /// ceiling here: the surplus is added to it, so a column that fills can
    /// never collapse and there is no separate minimum to remember. A column
    /// with twice the share of another takes twice as much of the surplus.
    ///
    /// The moment one column asks, it and its fellow askers divide the surplus
    /// and every other column keeps exactly what it asked for - which is how a
    /// table gets one exact column beside one that stretches.
    ///
    /// ```
    /// # use gpui::px;
    /// # use gpui_ai::records_table::RecordColumn;
    /// let exact = RecordColumn::new("cost", "Unit cost").width(px(120.));
    /// let stretches = RecordColumn::new("notes", "Notes").width(px(180.)).fill(1.);
    /// ```
    pub fn fill(mut self, share: f32) -> Self {
        self.fill = Some(share.max(0.));
        self
    }

    /// Sets the column's horizontal content alignment.
    pub fn alignment(mut self, alignment: RecordColumnAlignment) -> Self {
        self.alignment = alignment;
        self
    }

    /// Pins or unpins the column at the leading edge.
    pub fn fixed(mut self, fixed: bool) -> Self {
        self.fixed = fixed;
        self
    }

    /// Adds supplementary accessible column guidance.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Returns the stable application column ID.
    pub fn id(&self) -> &str {
        self.id.as_ref()
    }

    /// Returns the visible column label.
    pub fn label(&self) -> &str {
        self.label.as_ref()
    }

    /// Returns whether the column can request sorting.
    pub fn is_sortable(&self) -> bool {
        self.sortable
    }

    /// Returns the explicitly configured logical column width, if any.
    ///
    /// A width set with [`RecordColumn::width_in_rems`] has no pixel value
    /// until a window resolves it, so it reads as `None` here.
    pub fn configured_width(&self) -> Option<Pixels> {
        match self.width {
            RecordColumnWidth::Pixels(width) => Some(width),
            RecordColumnWidth::Unset | RecordColumnWidth::Rems(_) => None,
        }
    }

    /// Returns the horizontal content alignment.
    pub fn column_alignment(&self) -> RecordColumnAlignment {
        self.alignment
    }

    /// Returns whether the column is pinned to the leading edge.
    pub fn is_fixed(&self) -> bool {
        self.fixed
    }

    /// Returns supplementary accessible guidance, if supplied.
    pub fn accessible_description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

/// One readable record value associated with a stable column ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordCell {
    column_id: SharedString,
    value: SharedString,
    kind: RecordCellKind,
    tags: Arc<[SharedString]>,
    status_tone: Option<RecordStatusTone>,
    change_before: Option<SharedString>,
}

impl RecordCell {
    /// Creates a cell for the identified column.
    pub fn new(column_id: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        Self {
            column_id: column_id.into(),
            value: value.into(),
            kind: RecordCellKind::Text,
            tags: Arc::from([]),
            status_tone: None,
            change_before: None,
        }
    }

    /// Creates a categorical tag cell.
    pub fn tags(
        column_id: impl Into<SharedString>,
        tags: impl IntoIterator<Item = impl Into<SharedString>>,
    ) -> Self {
        let tags: Arc<[SharedString]> = tags.into_iter().map(Into::into).collect::<Vec<_>>().into();
        let value = tags
            .iter()
            .map(AsRef::<str>::as_ref)
            .collect::<Vec<_>>()
            .join(", ");
        Self {
            column_id: column_id.into(),
            value: value.into(),
            kind: RecordCellKind::Tags,
            tags,
            status_tone: None,
            change_before: None,
        }
    }

    /// Creates a cell describing a change from one value to another.
    ///
    /// The change kind is carried by the tone glyph, with the struck
    /// previous value, an arrow, and the new value as one readable run —
    /// never a prose prefix that wraps and clips. An addition passes no
    /// `before`, a removal no `after`; the accessible value reads as a
    /// sentence either way.
    pub fn change(
        column_id: impl Into<SharedString>,
        before: Option<SharedString>,
        after: Option<SharedString>,
        tone: RecordStatusTone,
    ) -> Self {
        Self {
            column_id: column_id.into(),
            value: after.unwrap_or_default(),
            kind: RecordCellKind::Change,
            tags: Arc::from([]),
            status_tone: Some(tone),
            change_before: before,
        }
    }

    /// The previous value of a change cell, when the change kept one.
    pub fn change_before(&self) -> Option<&SharedString> {
        self.change_before.as_ref()
    }

    /// The value assistive technology reads for this cell.
    ///
    /// A change cell reads as a sentence — "was A, now B" — instead of
    /// leaking its visual arrow and strikethrough.
    pub fn accessible_value(&self) -> SharedString {
        match (&self.kind, &self.change_before) {
            (RecordCellKind::Change, Some(before)) if self.value.is_empty() => {
                format!("removed {before}").into()
            }
            (RecordCellKind::Change, Some(before)) => {
                format!("was {before}, now {}", self.value).into()
            }
            (RecordCellKind::Change, None) => format!("added {}", self.value).into(),
            _ => self.value.clone(),
        }
    }

    /// Creates a readable semantic status cell.
    pub fn status(
        column_id: impl Into<SharedString>,
        value: impl Into<SharedString>,
        tone: RecordStatusTone,
    ) -> Self {
        Self {
            column_id: column_id.into(),
            value: value.into(),
            kind: RecordCellKind::Status,
            tags: Arc::from([]),
            status_tone: Some(tone),
            change_before: None,
        }
    }

    /// Returns the stable column ID associated with this value.
    pub fn column_id(&self) -> &str {
        self.column_id.as_ref()
    }

    /// Returns the readable cell value.
    pub fn value(&self) -> &str {
        self.value.as_ref()
    }

    /// Returns the cell's typed visual structure.
    pub fn kind(&self) -> RecordCellKind {
        self.kind
    }

    /// Returns categorical tags for a tag cell.
    pub fn tag_values(&self) -> &[SharedString] {
        &self.tags
    }

    /// Returns the semantic status tone for a status cell.
    pub fn status_tone(&self) -> Option<RecordStatusTone> {
        self.status_tone
    }
}

/// One immutable record row keyed by a stable application ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordRow {
    id: SharedString,
    label: SharedString,
    cells: Vec<RecordCell>,
    disabled: bool,
}

impl RecordRow {
    /// Creates an empty row with a stable ID and accessible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            cells: Vec::new(),
            disabled: false,
        }
    }

    /// Replaces the row's immutable cell snapshot.
    pub fn cells(mut self, cells: impl IntoIterator<Item = RecordCell>) -> Self {
        self.cells = cells.into_iter().collect();
        self
    }

    /// Sets whether the row rejects selection and activation intent.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Returns the stable application row ID.
    pub fn id(&self) -> &str {
        self.id.as_ref()
    }

    /// Returns the row's accessible label.
    pub fn label(&self) -> &str {
        self.label.as_ref()
    }

    /// Returns the cell associated with `column_id`, if present.
    pub fn cell(&self, column_id: &str) -> Option<&RecordCell> {
        self.cells
            .iter()
            .find(|cell| cell.column_id.as_ref() == column_id)
    }

    /// Returns the immutable cell snapshot in consumer order.
    pub fn all_cells(&self) -> &[RecordCell] {
        &self.cells
    }

    /// Returns whether the row rejects selection and activation intent.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }
}

pub(crate) fn record_columns_have_unique_ids(columns: &[RecordColumn]) -> bool {
    let mut seen = HashSet::with_capacity(columns.len());
    columns.iter().all(|column| seen.insert(column.id()))
}

pub(crate) fn record_rows_have_unique_ids(rows: &[RecordRow]) -> bool {
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

pub(crate) trait RecordCellProvider: Send + Sync {
    fn cell(&self, row_ix: usize, column_id: &str) -> Option<RecordCell>;
}

pub(crate) fn escape_markdown_text(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        if matches!(
            character,
            '\\' | '`'
                | '*'
                | '_'
                | '{'
                | '}'
                | '['
                | ']'
                | '<'
                | '>'
                | '('
                | ')'
                | '#'
                | '+'
                | '-'
                | '.'
                | '!'
                | '|'
                | '~'
        ) {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// The strip along the bottom of a table.
///
/// Outside the table's own border rather than inside it: the border belongs to
/// the scrolling body, and a summary that scrolled away with the rows would be
/// a row. Sitting under it it reads as the third part of the table - and gives
/// a frame taller than its rows a bottom that means something.
///
/// Inset to the same column as the first cell above it, so the count sits
/// under what it counts.
fn records_table_footer(
    id: &SharedString,
    footer: SharedString,
    cx: &App,
) -> impl gpui::IntoElement {
    let tokens = cx.theme().semantic_tokens();
    let debug_id = id.clone();
    div()
        .debug_selector(move || format!("records-table-footer-{debug_id}"))
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .pt(tokens.spacing.xs)
        .px(tokens.spacing.sm)
        .child(crate::surface::hint(footer, cx))
}

fn records_table_frame(id: SharedString, label: SharedString) -> Stateful<Div> {
    let debug_id = id.clone();
    div()
        .id(format!("records-table-{id}"))
        .debug_selector(move || format!("records-table-{debug_id}"))
        .size_full()
        .role(Role::Table)
        .aria_label(label)
}

fn scoped_records_id(kind: &str, component_id: &str, local_id: &str) -> String {
    format!(
        "records-{kind}-{}:{component_id}{local_id}",
        component_id.len()
    )
}

/// A glyph that restates the state family without color: a spinner for
/// in-flight work, a cross for failures, a dash for nothing to show.
fn records_state_glyph(role: Role) -> AnyElement {
    match role {
        Role::ProgressIndicator => Spinner::new().xsmall().into_any_element(),
        Role::Alert => Icon::new(IconName::CircleX).small().into_any_element(),
        _ => Icon::new(IconName::Inbox).small().into_any_element(),
    }
}

/// Loading labels shimmer; settled states stay selectable prose.
fn records_state_text(scoped_id: &str, role: Role, label: SharedString) -> AnyElement {
    if role == Role::ProgressIndicator {
        Shimmer::new(format!("{scoped_id}-shimmer"), label).into_any_element()
    } else {
        TextView::markdown(
            format!("{scoped_id}-text"),
            escape_markdown_text(label.as_ref()),
        )
        .selectable(true)
        .into_any_element()
    }
}

fn records_inline_state_frame(
    component_id: &str,
    id: &str,
    role: Role,
    label: SharedString,
    cx: &App,
) -> Stateful<Div> {
    let scoped_id = scoped_records_id("state", component_id, id);
    let tokens = cx.theme().semantic_tokens();
    div()
        .id(scoped_id.clone())
        .debug_selector({
            let scoped_id = scoped_id.clone();
            move || scoped_id.clone()
        })
        .w_full()
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .py(tokens.spacing.xxs)
        .gap(tokens.spacing.xs)
        .role(role)
        .aria_label(label.clone())
        .text_color(match role {
            Role::Alert => cx.theme().danger,
            _ => cx.theme().muted_foreground,
        })
        .child(records_state_glyph(role))
        .child(records_state_text(&scoped_id, role, label))
}

/// A controlled, virtualized records grid built on gpui-component's table.
///
/// Applications own columns, records, progress, sorting, and the selected row
/// ID. The entity retains only upstream focus and scrolling state.
pub struct RecordsTable {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    label: SharedString,
    /// A summary strip along the bottom of the frame, when the application
    /// gives one.
    ///
    /// A table's third part, after its header and its body. It is what stops a
    /// frame taller than its rows from reading as rows that have been cut off:
    /// with a bottom that says something - a count, a total - the space above
    /// it is a body with room left in it, which is what it is.
    footer: Option<SharedString>,
    /// How the table divides the width it is given.
    fit: ColumnFit,
    /// The height every row is drawn at, when the application names one.
    row_height: Option<Pixels>,
    /// The width the grid was last drawn at, which is the only thing a filling
    /// column has to divide.
    ///
    /// Kept apart from [`ResolvedLayoutKey`], which is for what the *window*
    /// resolves: a rem change invalidates every measured row height in the
    /// crate, while this invalidates one table's column widths.
    measured_width: Option<Pixels>,
    /// The width the columns in force were divided from, so a frame can tell
    /// whether the division it is looking at is the current one.
    divided_width: Option<Pixels>,
    columns: Arc<[RecordColumn]>,
    records: Progressive<Arc<[RecordRow]>>,
    /// Rebuilt with `records`; never read against any other snapshot.
    rows_by_id: StableIdIndex,
    /// Rebuilt with `columns`; never read against any other snapshot.
    columns_by_id: StableIdIndex,
    selected_row_id: Option<SharedString>,
    sort_column_id: Option<SharedString>,
    sort_direction: Option<RecordSortDirection>,
    activation_label: SharedString,
    pending_suppressed_selection_events: usize,
    pending_pointer_row_id: Option<SharedString>,
    viewport_row_anchor_id: Option<SharedString>,
    viewport_column_anchor_id: Option<SharedString>,
    resolved_layout: ResolvedLayoutKey,
    table: gpui::Entity<TableState<RecordsDelegate>>,
    _table_subscription: Subscription,
}

impl RecordsTable {
    /// Creates an empty records table with a stable ID and accessible label.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let id = id.into();
        let owner = cx.weak_entity();
        let delegate_id = id.clone();
        // The first frame already has a rem to resolve against, so a column
        // width given in rems is right on the frame it first appears rather
        // than being corrected after one.
        let rem_size = window.rem_size();
        let table = cx.new(|cx| {
            TableState::new(
                RecordsDelegate::empty(owner, delegate_id, rem_size),
                window,
                cx,
            )
            .loop_selection(false)
            .col_selectable(false)
            .col_movable(false)
            .row_selectable(true)
            .sortable(false)
        });
        let table_subscription = cx.subscribe(&table, |this, _, event, cx| {
            this.handle_table_event(event, cx);
        });
        let mut resolved_layout = ResolvedLayoutKey::default();
        resolved_layout.observe(rem_size);

        Self {
            style: gpui::StyleRefinement::default(),
            id,
            label: label.into(),
            footer: None,
            fit: ColumnFit::default(),
            row_height: None,
            measured_width: None,
            divided_width: None,
            columns: Arc::from([]),
            records: Progressive::pending(Arc::from([])),
            rows_by_id: StableIdIndex::default(),
            columns_by_id: StableIdIndex::default(),
            selected_row_id: None,
            sort_column_id: None,
            sort_direction: None,
            activation_label: "Open".into(),
            pending_suppressed_selection_events: 0,
            pending_pointer_row_id: None,
            viewport_row_anchor_id: None,
            viewport_column_anchor_id: None,
            resolved_layout,
            table,
            _table_subscription: table_subscription,
        }
    }

    /// Sets when every row's activation control is visible.
    pub fn set_row_action_visibility(
        &mut self,
        visibility: RowActionVisibility,
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().row_action_visibility = visibility;
            cx.notify();
        });
    }

    /// Sets where every row's activation control sits in its cell.
    pub fn set_row_action_placement(
        &mut self,
        placement: RowActionPlacement,
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().row_action_placement = placement;
            cx.notify();
        });
    }

    /// Replaces the visible and accessible verb used by row activation controls.
    ///
    /// The verb reaches the screen through the delegate, and this entity's own
    /// `render` never reads it, so only the table is notified: the window is
    /// invalidated either way, and a second notification would wake
    /// application observers of a value they already own.
    pub fn set_activation_label(
        &mut self,
        activation_label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        self.activation_label = activation_label.into();
        let activation_label = self.activation_label.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().activation_label = activation_label;
            cx.notify();
        });
    }

    /// Replaces the label shown when the table has no matching records.
    pub fn set_empty_label(
        &mut self,
        empty_label: impl Into<SharedString>,
        cx: &mut Context<Self>,
    ) {
        let empty_label = empty_label.into();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().empty_label = empty_label;
            cx.notify();
        });
    }

    pub(crate) fn set_row_reorder_response(
        &mut self,
        response: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        self.table.update(cx, |table, cx| {
            table.delegate_mut().row_reorder.set_response(response);
            cx.notify();
        });
    }

    pub(crate) fn visible_row_count(&self, cx: &App) -> usize {
        self.table.read(cx).delegate().row_reorder.visible_len()
    }

    pub(crate) fn animating_row_count(&self, cx: &App) -> usize {
        self.table.read(cx).delegate().row_reorder.animating_len()
    }

    /// Sets the summary strip along the bottom of the table, or clears it.
    ///
    /// Controlled like every other input here: the application owns the text,
    /// and the table renders whatever it is given. A row count is the usual
    /// one; a total or a filter summary is as good.
    ///
    /// ```
    /// # use gpui_ai::records_table::RecordsTable;
    /// # fn example(table: &mut RecordsTable, cx: &mut gpui::Context<RecordsTable>) {
    /// table.set_footer(Some("4 suppliers".into()), cx);
    /// # }
    /// ```
    pub fn set_footer(&mut self, footer: Option<SharedString>, cx: &mut Context<Self>) {
        if self.footer != footer {
            self.footer = footer;
            cx.notify();
        }
    }

    /// Sets how the table divides the width it is given.
    ///
    /// [`ColumnFit::Fill`] by default, which is what a reader expects of a
    /// table: it ends where its frame ends. [`ColumnFit::Intrinsic`] keeps
    /// every column at exactly the width it was given and hands column widths
    /// back to the reader's own drag.
    ///
    /// ```
    /// # use gpui_ai::records_table::{ColumnFit, RecordsTable};
    /// # fn example(table: &mut RecordsTable, cx: &mut gpui::Context<RecordsTable>) {
    /// table.set_column_fit(ColumnFit::Intrinsic, cx);
    /// # }
    /// ```
    pub fn set_column_fit(&mut self, fit: ColumnFit, cx: &mut Context<Self>) {
        if self.fit == fit {
            return;
        }
        self.fit = fit;
        self.divide_columns(cx);
    }

    /// Returns how the table divides the width it is given.
    pub fn column_fit(&self) -> ColumnFit {
        self.fit
    }

    /// Sets the height every row is drawn at, or restores the table's own.
    ///
    /// A row's height is the table's rhythm, and an application that has
    /// measured its own content may name it exactly. `None` returns the rows
    /// to the height the size policy gives them.
    ///
    /// ```
    /// # use gpui::px;
    /// # use gpui_ai::records_table::RecordsTable;
    /// # fn example(table: &mut RecordsTable, cx: &mut gpui::Context<RecordsTable>) {
    /// table.set_row_height(Some(px(44.)), cx);
    /// # }
    /// ```
    pub fn set_row_height(&mut self, row_height: Option<Pixels>, cx: &mut Context<Self>) {
        if self.row_height == row_height {
            return;
        }
        self.row_height = row_height;
        cx.notify();
    }

    /// Returns the height every row is drawn at, when one was named.
    pub fn row_height(&self) -> Option<Pixels> {
        self.row_height
    }

    /// Hands the grid the width every column should now be drawn at.
    ///
    /// Upstream caches column widths in pixels when it builds its column
    /// groups, so a width that changed only reaches layout through a refresh.
    fn push_column_widths(&mut self, cx: &mut Context<Self>) {
        let columns = self.columns.clone();
        let (fit, measured) = (self.fit, self.measured_width);
        self.table.update(cx, |table, cx| {
            // The overlaid bar's lane comes off the top. It has to: a bar that
            // takes no layout space would otherwise sit over the last column's
            // content, and columns that divide the box exactly leave the grid
            // balanced on the edge of needing a horizontal scrollbar, which it
            // then decides again every frame and never settles. Under a gutter
            // policy this is zero, because the gutter has already taken it.
            let lane = crate::scrolling::control_lane(cx);
            let available = measured.map(|measured| measured - lane);
            let rem_size = table.delegate().rem_size;
            table.delegate_mut().column_widths =
                resolve_column_widths(&columns, fit, available, rem_size);
            table.delegate_mut().fit = fit;
            table.refresh(cx);
            cx.notify();
        });
    }

    /// Records the width the grid was drawn at.
    ///
    /// Called from the grid's own prepaint, where it may record and nothing
    /// else. Dividing the columns here would rebuild the grid's column groups
    /// in the middle of the frame that is measuring them, and the frame never
    /// settles: nineteen thousand prepaints for one draw, which is how this was
    /// found. The division is deferred out of the frame instead, the same way
    /// a new rem is, and the width that was divided is remembered so the next
    /// frame knows there is nothing left to do.
    fn observe_width(&mut self, width: Pixels, cx: &mut Context<Self>) {
        if self.measured_width == Some(width) {
            return;
        }
        self.measured_width = Some(width);
        cx.notify();
    }

    /// Whether the columns have been divided from the width last measured.
    fn columns_are_divided(&self) -> bool {
        self.fit == ColumnFit::Intrinsic || self.divided_width == self.measured_width
    }

    /// Divides the columns from the width last measured.
    fn divide_columns(&mut self, cx: &mut Context<Self>) {
        self.divided_width = self.measured_width;
        self.push_column_widths(cx);
    }

    /// Replaces the controlled column snapshot without rebuilding table state.
    ///
    /// A snapshot containing duplicate stable column IDs is ignored atomically.
    /// Columns are drawn entirely by the table, so only the table is notified.
    pub fn set_columns(
        &mut self,
        columns: impl IntoIterator<Item = RecordColumn>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let columns = columns.into_iter().collect::<Vec<_>>();
        let Some(columns_by_id) = index_valid_columns(&columns) else {
            return;
        };
        let anchor_column_id = self.viewport_column_anchor_id.clone().or_else(|| {
            let fixed_columns = self.columns.iter().filter(|column| column.fixed).count();
            self.columns
                .get(
                    self.table
                        .read(cx)
                        .visible_range()
                        .cols()
                        .start
                        .saturating_add(fixed_columns),
                )
                .map(|column| column.id.clone())
        });
        self.columns = columns.into();
        self.columns_by_id = columns_by_id;
        if self.sort_column_id.as_ref().is_some_and(|sort_column_id| {
            !self
                .columns_by_id
                .position(sort_column_id)
                .is_some_and(|col_ix| self.columns[col_ix].sortable)
        }) {
            self.sort_column_id = None;
            self.sort_direction = None;
        }
        let columns = self.columns.clone();
        let sort_column_id = self.sort_column_id.clone();
        let sort_direction = self.sort_direction;
        let anchor_column_ix = anchor_column_id
            .as_ref()
            .and_then(|anchor| self.columns_by_id.position(anchor));
        self.table.update(cx, |table, cx| {
            table.delegate_mut().columns = columns;
            table.delegate_mut().sort_column_id = sort_column_id;
            table.delegate_mut().sort_direction = sort_direction;
            table.refresh(cx);
            if let Some(column_ix) = anchor_column_ix {
                table.scroll_to_col(column_ix, cx);
            }
            cx.notify();
        });
        // New columns, new division: the widths cached above belong to the
        // snapshot that just left.
        self.divide_columns(cx);
    }

    /// Replaces the controlled progressive record snapshot.
    ///
    /// A snapshot containing duplicate row IDs or duplicate cell column IDs
    /// within one row is ignored atomically.
    pub fn set_records(
        &mut self,
        records: Progressive<Arc<[RecordRow]>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let _ = self.set_records_snapshot(records, cx);
    }

    pub(crate) fn set_records_snapshot(
        &mut self,
        records: Progressive<Arc<[RecordRow]>>,
        cx: &mut Context<Self>,
    ) -> bool {
        self.set_records_snapshot_with_cell_provider(records, None, cx)
    }

    pub(crate) fn set_records_snapshot_with_cell_provider(
        &mut self,
        records: Progressive<Arc<[RecordRow]>>,
        cell_provider: Option<Arc<dyn RecordCellProvider>>,
        cx: &mut Context<Self>,
    ) -> bool {
        // The validating pass answers every "where is this ID now?" below:
        // anchor recovery, the reorder window, selection, and the scroll
        // restore. The retained index still describes the outgoing snapshot
        // until this one replaces it, which is what makes the displacement
        // math a pair of lookups instead of a pair of scans.
        let Some(accepted_rows_by_id) = index_valid_rows(records.content()) else {
            return false;
        };
        let anchor_row_id = self.viewport_row_anchor_id.clone().or_else(|| {
            self.records
                .content()
                .get(self.table.read(cx).visible_range().rows().start)
                .map(|row| row.id.clone())
        });
        let old_visible_range = self.table.read(cx).visible_range().rows().clone();
        let row_reorder_enabled = self.table.read(cx).delegate().row_reorder.is_enabled();
        let visible_row_ids = if row_reorder_enabled {
            let rendered = self
                .table
                .read(cx)
                .delegate()
                .row_reorder
                .visible_ids()
                .clone();
            if rendered.is_empty() {
                self.records
                    .content()
                    .get(old_visible_range.clone())
                    .unwrap_or_default()
                    .iter()
                    .map(|row| row.id.clone())
                    .collect::<Vec<_>>()
            } else {
                rendered.into_iter().collect()
            }
        } else {
            Vec::new()
        };
        // Where the viewport lands in the accepted snapshot: the anchor row if
        // it survived, else the earliest surviving row that was on screen. The
        // reorder window below and the scroll restore afterwards are the same
        // recovery, so they share one answer rather than repeating it.
        let anchor_row_ix = anchor_row_id
            .as_ref()
            .and_then(|anchor| accepted_rows_by_id.position(anchor))
            .or_else(|| {
                visible_row_ids
                    .iter()
                    .filter_map(|row_id| accepted_rows_by_id.position(row_id))
                    .min()
            });
        let reorder_motion = if row_reorder_enabled && crate::motion::motion_is_full(cx) {
            let old_visible_start = old_visible_range.start;
            let visible_len = old_visible_range.len().max(visible_row_ids.len()).max(1);
            let new_visible_start = anchor_row_ix
                .unwrap_or(old_visible_start.min(records.content().len().saturating_sub(1)));
            let new_visible_end = new_visible_start
                .saturating_add(visible_len)
                .min(records.content().len());
            // How far a row that stays on screen travels between the two
            // snapshots is viewport geometry, and it is all the reorder owner
            // needs: the spring decisions the travel implies are its own.
            let travelled = visible_row_ids.iter().filter_map(|row_id| {
                let old_ix = self.rows_by_id.position(row_id)?;
                let new_ix = accepted_rows_by_id.position(row_id)?;
                if !(new_visible_start..new_visible_end).contains(&new_ix) {
                    return None;
                }
                let old_position = old_ix.saturating_sub(old_visible_start);
                let new_position = new_ix.saturating_sub(new_visible_start);
                let displacement =
                    Size::Medium.table_row_height() * (old_position as f32 - new_position as f32);
                Some((row_id, displacement))
            });
            self.table
                .read(cx)
                .delegate()
                .row_reorder
                .project(travelled)
        } else {
            HashMap::new()
        };
        self.records = records;
        self.rows_by_id = accepted_rows_by_id;
        // A selected row that the snapshot dropped, or now disables, is no
        // longer selectable, so the controlled value clears with it.
        let desired_row_ix = self
            .selected_row_id
            .as_ref()
            .and_then(|selected| self.rows_by_id.position(selected))
            .filter(|row_ix| !self.records.content()[*row_ix].disabled);
        if self.selected_row_id.is_some() && desired_row_ix.is_none() {
            self.selected_row_id = None;
        }

        let records = self.records.clone();
        let selected_row_id = self.selected_row_id.clone();
        if desired_row_ix.is_some() {
            self.pending_suppressed_selection_events =
                self.pending_suppressed_selection_events.saturating_add(1);
        }
        self.table.update(cx, |table, cx| {
            let delegate = table.delegate_mut();
            delegate.records = records;
            delegate.cell_provider = cell_provider;
            delegate.selected_row_id = selected_row_id.clone();
            delegate.row_reorder.accept(reorder_motion);
            table.refresh(cx);
            if let Some(row_ix) = desired_row_ix {
                table.set_selected_row(row_ix, cx);
            } else {
                table.clear_selection(cx);
            }
            if let Some(row_ix) = anchor_row_ix {
                table.scroll_to_row(row_ix, cx);
            }
            cx.notify();
        });
        // Both entities really do change here: the table draws the rows, and
        // this entity draws the progress and failure banner beside them from
        // the same snapshot's lifecycle state.
        cx.notify();
        true
    }

    /// Replaces the controlled selected row when the ID exists.
    ///
    /// The selection is drawn by the table's own rows, so only the table is
    /// notified; the reader for the controlled value is a plain accessor.
    pub fn set_selected_row(
        &mut self,
        row_id: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row_id = row_id.into();
        let Some(row_ix) = self
            .rows_by_id
            .position(&row_id)
            .filter(|row_ix| !self.records.content()[*row_ix].disabled)
        else {
            return;
        };

        self.selected_row_id = Some(row_id);
        self.pending_suppressed_selection_events =
            self.pending_suppressed_selection_events.saturating_add(1);
        let selected_row_id = self.selected_row_id.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().selected_row_id = selected_row_id;
            table.set_selected_row(row_ix, cx);
        });
    }

    /// Clears the controlled selected-row snapshot.
    pub fn clear_selected_row(&mut self, cx: &mut Context<Self>) {
        self.selected_row_id = None;
        self.table.update(cx, |table, cx| {
            table.delegate_mut().selected_row_id = None;
            table.clear_selection(cx);
        });
    }

    /// Returns the controlled selected row ID.
    pub fn selected_row_id(&self) -> Option<&str> {
        self.selected_row_id.as_deref()
    }

    /// Replaces the controlled sort snapshot.
    ///
    /// Passing `None` clears sorting. A non-sortable or unknown column ID is
    /// ignored so stale application snapshots cannot corrupt table state.
    /// The sort marker lives in the table's own header, so only the table is
    /// notified.
    pub fn set_sort(
        &mut self,
        column_id: impl Into<SharedString>,
        direction: Option<RecordSortDirection>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let column_id = column_id.into();
        if direction.is_some()
            && !self
                .columns_by_id
                .position(&column_id)
                .is_some_and(|col_ix| self.columns[col_ix].sortable)
        {
            return;
        }

        self.sort_column_id = direction.map(|_| column_id.clone());
        self.sort_direction = direction;
        let sort_column_id = self.sort_column_id.clone();
        self.table.update(cx, |table, cx| {
            table.delegate_mut().sort_column_id = sort_column_id;
            table.delegate_mut().sort_direction = direction;
            cx.notify();
        });
    }

    /// Returns the controlled sort column ID and direction.
    pub fn sort(&self) -> Option<(&str, RecordSortDirection)> {
        self.sort_column_id.as_deref().zip(self.sort_direction)
    }

    /// Scrolls the identified record into view when it exists.
    pub fn scroll_to_row(&mut self, row_id: &str, cx: &mut Context<Self>) {
        let Some(row_ix) = self.rows_by_id.position(row_id) else {
            return;
        };
        self.table
            .update(cx, |table, cx| table.scroll_to_row(row_ix, cx));
        self.viewport_row_anchor_id = Some(row_id.into());
    }

    /// Scrolls the identified column into view when it exists.
    pub fn scroll_to_column(&mut self, column_id: &str, cx: &mut Context<Self>) {
        let Some(col_ix) = self.columns_by_id.position(column_id) else {
            return;
        };
        self.table
            .update(cx, |table, cx| table.scroll_to_col(col_ix, cx));
        self.viewport_column_anchor_id = Some(column_id.into());
    }

    /// Moves keyboard focus to the records grid.
    pub fn focus(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.table.focus_handle(cx).focus(window, cx);
    }

    /// Re-resolves rem-scaled column widths after the window's rem changed.
    ///
    /// Upstream caches each column's width in pixels when the table refreshes,
    /// and no record snapshot reports a zoom, so without this a table sized in
    /// rems keeps the widths it resolved at the reader's previous type scale.
    fn resolve_layout(&mut self, rem_size: Pixels, cx: &mut Context<Self>) {
        if !self.resolved_layout.observe(rem_size) {
            return;
        }
        // Refreshing rebuilds every column's cached width, so a table whose
        // columns are all in pixels keeps whatever widths it has — including
        // any the reader dragged — while still carrying the new rem forward
        // for a column snapshot that arrives later.
        let widths_follow_the_rem = self
            .columns
            .iter()
            .any(|column| matches!(column.width, RecordColumnWidth::Rems(_)));
        self.table.update(cx, |table, _| {
            table.delegate_mut().rem_size = rem_size;
        });
        // A filling table divides again whatever its columns are measured in:
        // the bases moved, so the surplus did.
        if widths_follow_the_rem || self.fit == ColumnFit::Fill {
            self.divide_columns(cx);
        }
    }

    fn handle_table_event(&mut self, event: &TableEvent, cx: &mut Context<Self>) {
        match event {
            TableEvent::SelectRow(_) if self.pending_suppressed_selection_events > 0 => {
                self.pending_suppressed_selection_events =
                    self.pending_suppressed_selection_events.saturating_sub(1);
            }
            TableEvent::SelectRow(row_ix) => {
                if let Some(row) = self.records.content().get(*row_ix) {
                    let from_pointer = self.pending_pointer_row_id.take().as_ref() == Some(&row.id);
                    if !row.disabled {
                        cx.emit(RecordsTableEvent::SelectionRequested {
                            id: self.id.clone(),
                            row_id: row.id.clone(),
                        });
                    } else if !from_pointer {
                        self.request_enabled_beyond(*row_ix, cx);
                    }
                }
                self.defer_controlled_selection_sync(cx);
            }
            TableEvent::ClearSelection => {
                // Escape reaches upstream's Cancel action, which drops its own
                // selection and tells us here. Selection is application-owned,
                // so re-assert the controlled value instead of letting the
                // rendered row and `selected_row_id` disagree — otherwise the
                // highlight vanishes while Enter still activates the old row.
                self.defer_controlled_selection_sync(cx);
            }
            TableEvent::DoubleClickedRow(row_ix) => {
                if let Some(row) = self.records.content().get(*row_ix)
                    && !row.disabled
                {
                    cx.emit(RecordsTableEvent::ActivationRequested {
                        id: self.id.clone(),
                        row_id: row.id.clone(),
                    });
                }
            }
            _ => {}
        }
    }

    fn defer_controlled_selection_sync(&self, cx: &mut Context<Self>) {
        let owner = cx.weak_entity();
        cx.defer(move |cx| {
            let _ = owner.update(cx, |table, cx| table.sync_controlled_selection(cx));
        });
    }

    fn sync_controlled_selection(&mut self, cx: &mut Context<Self>) {
        let desired_row_ix = self
            .selected_row_id
            .as_ref()
            .and_then(|selected| self.rows_by_id.position(selected));
        let current_row_ix = self.table.read(cx).selected_row();
        if desired_row_ix == current_row_ix {
            return;
        }

        if desired_row_ix.is_some() {
            self.pending_suppressed_selection_events =
                self.pending_suppressed_selection_events.saturating_add(1);
        }
        self.table.update(cx, |table, cx| {
            if let Some(row_ix) = desired_row_ix {
                table.set_selected_row(row_ix, cx);
            } else if table.selected_row().is_some() {
                table.clear_selection(cx);
            }
        });
    }

    fn request_sort(&mut self, column_id: SharedString, cx: &mut Context<Self>) {
        let direction = if self.sort_column_id.as_ref() == Some(&column_id) {
            match self.sort_direction {
                None => Some(RecordSortDirection::Descending),
                Some(RecordSortDirection::Descending) => Some(RecordSortDirection::Ascending),
                Some(RecordSortDirection::Ascending) => None,
            }
        } else {
            Some(RecordSortDirection::Descending)
        };

        cx.emit(RecordsTableEvent::SortRequested {
            id: self.id.clone(),
            column_id,
            direction,
        });
    }

    fn request_activation(&mut self, row_id: SharedString, cx: &mut Context<Self>) {
        self.pending_pointer_row_id = None;
        if self
            .rows_by_id
            .position(&row_id)
            .is_some_and(|row_ix| !self.records.content()[row_ix].disabled)
        {
            cx.emit(RecordsTableEvent::ActivationRequested {
                id: self.id.clone(),
                row_id,
            });
        }
    }

    fn activate_selected(&mut self, _: &ActivateRecord, _: &mut Window, cx: &mut Context<Self>) {
        let Some(row_id) = self.selected_row_id.clone() else {
            cx.propagate();
            return;
        };
        if self
            .rows_by_id
            .position(&row_id)
            .is_none_or(|row_ix| self.records.content()[row_ix].disabled)
        {
            cx.propagate();
            return;
        }
        cx.emit(RecordsTableEvent::ActivationRequested {
            id: self.id.clone(),
            row_id,
        });
        cx.stop_propagation();
    }

    fn request_enabled_beyond(&self, disabled_index: usize, cx: &mut Context<Self>) {
        let current = self
            .selected_row_id
            .as_ref()
            .and_then(|selected| self.rows_by_id.position(selected));
        let forward = current.is_none_or(|current| disabled_index > current);
        let next = if forward {
            self.records
                .content()
                .iter()
                .enumerate()
                .skip(disabled_index.saturating_add(1))
                .find(|(_, row)| !row.disabled)
        } else {
            self.records
                .content()
                .iter()
                .enumerate()
                .take(disabled_index)
                .rev()
                .find(|(_, row)| !row.disabled)
        };

        let Some((_, row)) = next else {
            return;
        };
        cx.emit(RecordsTableEvent::SelectionRequested {
            id: self.id.clone(),
            row_id: row.id.clone(),
        });
    }
}

impl EventEmitter<RecordsTableEvent> for RecordsTable {}

impl Focusable for RecordsTable {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.table.focus_handle(cx)
    }
}

impl gpui::Styled for RecordsTable {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for RecordsTable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        // A column width given in rems is only as current as the rem it was
        // resolved against. Reading it here mutates nothing; the reaction is
        // deferred so that render itself neither refreshes nor notifies.
        let rem_size = window.rem_size();
        if !self.resolved_layout.matches(rem_size) {
            cx.defer_in(window, move |table, _, cx| {
                table.resolve_layout(rem_size, cx);
            });
        }
        // And the same for the width the columns divide, for the same reason:
        // reading it here mutates nothing.
        if !self.columns_are_divided() {
            cx.defer_in(window, move |table, _, cx| {
                table.divide_columns(cx);
            });
        }

        let inline_status = (!self.records.content().is_empty())
            .then(|| match self.records.state() {
                ProgressState::Pending | ProgressState::Running => {
                    Some(records_inline_state_frame(
                        &self.id,
                        "records-loading",
                        Role::ProgressIndicator,
                        "Loading records".into(),
                        cx,
                    ))
                }
                ProgressState::Failed(reason) => Some(records_inline_state_frame(
                    &self.id,
                    "records-error",
                    Role::Alert,
                    format!("Records unavailable: {reason}").into(),
                    cx,
                )),
                ProgressState::Complete => None,
            })
            .flatten();
        records_table_frame(self.id.clone(), self.label.clone())
            .flex()
            .flex_col()
            .min_h_0()
            .key_context(RECORDS_TABLE_CONTEXT)
            // The component's own ink. Everything inside the grid is drawn by
            // the table, which colours itself, so nothing here needed saying
            // until the footer put the crate's own text in this frame - and
            // inherited GPUI's default, which is white, and vanished against
            // the surface. A component sets the ink it draws in rather than
            // trusting whatever it was dropped into.
            .text_color(cx.theme().foreground)
            .border_1()
            .border_color(cx.theme().transparent)
            .track_focus(&self.table.focus_handle(cx))
            .focus_visible(|style| style.border_color(cx.theme().ring))
            .on_action(cx.listener(Self::activate_selected))
            .when_some(inline_status, |this, status| this.child(status))
            .child(
                // Upstream pads a striped table with empty rows to cover
                // whatever space is left over, so a table stretched to a
                // container taller than its rows grows a band of blank stripes
                // with the last one clipped by the edge - rows that appear to
                // be there and cannot be reached. Unstriped, the leftover
                // space is plainly leftover space, and the footer below says
                // where the body ends.
                div()
                    .flex_1()
                    .min_h_0()
                    // The frame around the grid is drawn here rather than by
                    // the grid, in the same ink and radius upstream would have
                    // used. It has to be ours because the measurement below is
                    // taken inside it: a column dividing the box the border is
                    // drawn on divides two pixels the grid never gets, and the
                    // table would overflow itself by exactly its own edge.
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    // What a filling column divides: the box the grid is given,
                    // and the only one a column can be a share of.
                    .child(measured_grid_width(cx.weak_entity()))
                    .child(
                        DataTable::new(&self.table)
                            .stripe(false)
                            .bordered(false)
                            .when_some(self.row_height, |grid, height| {
                                grid.with_size(Size::Size(height))
                            }),
                    ),
            )
            .when_some(self.footer.clone(), |frame, footer| {
                // Painted after the grid, not merely placed below it.
                // Upstream draws the grid's horizontal bar in an overlay that
                // hangs past the bottom of the box the grid was given, and a
                // strip laid out underneath is drawn over and never seen. A
                // deferred child keeps its place in the layout and paints last,
                // which is the whole of what this needs.
                frame.child(
                    gpui::deferred(records_table_footer(&self.id, footer, cx)).with_priority(1),
                )
            })
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests;
