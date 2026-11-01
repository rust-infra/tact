//! Bounded, feature-oriented comparison table values and presentation.

use gpui_base::StyledExt as _;
use std::{collections::HashSet, sync::Arc};

use gpui::{
    AnyElement, App, Axis, Context, ElementId, EventEmitter, FocusHandle, Focusable, Hsla,
    InteractiveElement as _, IntoElement, ListAlignment, ListOffset, ListState, ParentElement as _,
    Pixels, Render, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, Styled as _,
    Window, div, list, prelude::FluentBuilder as _,
};
use gpui_base::InteractiveElementExt as _;
use gpui_component::{
    ActiveTheme as _, Icon, IconName, h_flex, scroll::ScrollableMask, text::TextView,
};

use crate::scrolling::PolicyScrollbarExt as _;
use crate::{
    control::outlined_control_with_wrapping_label,
    motion::{MotionTokens, VisibleAnimationExt as _},
    records_table::escape_markdown_text,
    resolved_layout::ResolvedLayoutKey,
    scrolling::list_scroll_mask,
    stream::{ProgressState, Progressive},
    theme::SemanticStyledExt as _,
};

/// Maximum number of side-by-side items accepted by a comparison snapshot.
pub const MAX_COMPARISON_ITEMS: usize = 12;

/// Maximum number of feature rows accepted by a comparison snapshot.
pub const MAX_COMPARISON_FEATURES: usize = 128;

/// Distance past the viewport, in rem, where feature rows stay measured.
///
/// Rem rather than pixels: a row's height is wrapped text laid out against
/// the window's type scale, so the overdraw that buys one row of slack at a
/// small rem buys none at a large one. Sixteen rem is the transcript's
/// overdraw at a default type scale.
const FEATURE_OVERDRAW_REM: f32 = 16.;

/// Presentation state attached to one comparison item.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum ComparisonItemState {
    /// An ordinary comparison item.
    #[default]
    Default,
    /// An item the application wants to call out as recommended or notable.
    Highlighted,
    /// An item that cannot currently be selected or activated.
    Disabled,
}

/// One consumer-owned item displayed as a comparison column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonItem {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    state: ComparisonItemState,
}

impl ComparisonItem {
    /// Creates an ordinary item with stable identity and a visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            state: ComparisonItemState::Default,
        }
    }

    /// Adds readable supporting copy for the item header.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the consumer-owned item presentation state.
    pub fn state(mut self, state: ComparisonItemState) -> Self {
        self.state = state;
        self
    }

    /// Returns the stable application ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the visible label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns optional readable supporting copy.
    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the current consumer-owned presentation state.
    pub fn item_state(&self) -> ComparisonItemState {
        self.state
    }
}

/// One readable value at the intersection of a feature and comparison item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonValue {
    item_id: SharedString,
    display: SharedString,
    included: Option<bool>,
}

impl ComparisonValue {
    /// Creates a free-form readable value for one stable item ID.
    pub fn new(item_id: impl Into<SharedString>, display: impl Into<SharedString>) -> Self {
        Self {
            item_id: item_id.into(),
            display: display.into(),
            included: None,
        }
    }

    /// Creates an explicit included/not-included value without relying on an icon or color.
    pub fn included(item_id: impl Into<SharedString>, included: bool) -> Self {
        Self {
            item_id: item_id.into(),
            display: if included { "Included" } else { "Not included" }.into(),
            included: Some(included),
        }
    }

    /// Returns the stable item ID this value belongs to.
    pub fn item_id(&self) -> &str {
        &self.item_id
    }

    /// Returns the readable displayed value.
    pub fn display(&self) -> &str {
        &self.display
    }

    /// Returns explicit support state when this is an included/not-included value.
    pub fn is_included(&self) -> Option<bool> {
        self.included
    }
}

/// One feature row with values keyed by stable comparison item ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonFeature {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    values: Arc<[ComparisonValue]>,
}

impl ComparisonFeature {
    /// Creates an empty feature row with stable identity and a visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            values: Arc::from([]),
        }
    }

    /// Adds readable supporting copy for the feature label.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Replaces the immutable item-value snapshot.
    pub fn values(mut self, values: impl IntoIterator<Item = ComparisonValue>) -> Self {
        self.values = values.into_iter().collect::<Vec<_>>().into();
        self
    }

    /// Returns the stable application ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the visible label.
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns optional readable supporting copy.
    pub fn description_text(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the value for a stable item ID.
    pub fn value(&self, item_id: &str) -> Option<&ComparisonValue> {
        self.values.iter().find(|value| value.item_id == item_id)
    }

    /// Returns the immutable values in consumer order.
    pub fn values_snapshot(&self) -> &[ComparisonValue] {
        &self.values
    }
}

/// Structural validation failure for a bounded comparison snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComparisonSnapshotError {
    /// More side-by-side items were supplied than the bounded layout supports.
    TooManyItems {
        /// The largest accepted item count.
        maximum: usize,
    },
    /// More feature rows were supplied than the bounded layout supports.
    TooManyFeatures {
        /// The largest accepted feature count.
        maximum: usize,
    },
    /// Two items reused one stable ID.
    DuplicateItemId(SharedString),
    /// Two features reused one stable ID.
    DuplicateFeatureId(SharedString),
    /// A feature supplied more than one value for the same stable item ID.
    DuplicateValueItemId {
        /// Stable feature ID containing the duplicate.
        feature_id: SharedString,
        /// Reused stable item ID.
        item_id: SharedString,
    },
    /// A feature value referenced an item absent from the snapshot.
    UnknownItemId {
        /// Stable feature ID containing the value.
        feature_id: SharedString,
        /// Unknown stable item ID.
        item_id: SharedString,
    },
}

/// Validated immutable data for one intentionally bounded comparison surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonSnapshot {
    items: Arc<[ComparisonItem]>,
    features: Arc<[ComparisonFeature]>,
}

impl ComparisonSnapshot {
    /// Validates and creates a bounded comparison snapshot.
    pub fn try_new(
        items: impl IntoIterator<Item = ComparisonItem>,
        features: impl IntoIterator<Item = ComparisonFeature>,
    ) -> Result<Self, ComparisonSnapshotError> {
        let items = items.into_iter().collect::<Vec<_>>();
        if items.len() > MAX_COMPARISON_ITEMS {
            return Err(ComparisonSnapshotError::TooManyItems {
                maximum: MAX_COMPARISON_ITEMS,
            });
        }
        let features = features.into_iter().collect::<Vec<_>>();
        if features.len() > MAX_COMPARISON_FEATURES {
            return Err(ComparisonSnapshotError::TooManyFeatures {
                maximum: MAX_COMPARISON_FEATURES,
            });
        }

        let mut item_ids = HashSet::new();
        for item in &items {
            if !item_ids.insert(item.id.clone()) {
                return Err(ComparisonSnapshotError::DuplicateItemId(item.id.clone()));
            }
        }

        let mut feature_ids = HashSet::new();
        for feature in &features {
            if !feature_ids.insert(feature.id.clone()) {
                return Err(ComparisonSnapshotError::DuplicateFeatureId(
                    feature.id.clone(),
                ));
            }
            let mut value_item_ids = HashSet::new();
            for value in feature.values.iter() {
                if !value_item_ids.insert(value.item_id.clone()) {
                    return Err(ComparisonSnapshotError::DuplicateValueItemId {
                        feature_id: feature.id.clone(),
                        item_id: value.item_id.clone(),
                    });
                }
                if !item_ids.contains(&value.item_id) {
                    return Err(ComparisonSnapshotError::UnknownItemId {
                        feature_id: feature.id.clone(),
                        item_id: value.item_id.clone(),
                    });
                }
            }
        }

        Ok(Self {
            items: items.into(),
            features: features.into(),
        })
    }

    /// Returns the immutable items in consumer order.
    pub fn items(&self) -> &[ComparisonItem] {
        &self.items
    }

    /// Returns the immutable features in consumer order.
    pub fn features(&self) -> &[ComparisonFeature] {
        &self.features
    }

    /// Finds a feature by stable application ID.
    pub fn feature(&self, feature_id: &str) -> Option<&ComparisonFeature> {
        self.features
            .iter()
            .find(|feature| feature.id == feature_id)
    }
}

/// Element construction counts for one draw of the virtualized render.
///
/// This surface is bounded at [`MAX_COMPARISON_FEATURES`] rows by
/// [`MAX_COMPARISON_ITEMS`] columns, but the feature list is virtualized: a
/// draw constructs only the rows the viewport shows, so the counts must track
/// that window — never the whole bound, and never a running total across
/// redraws. The eager render this replaced built all 1,536 cells of the
/// maximum shape every draw. Counting is test-only: production builds carry
/// neither the field nor the branch.
#[cfg(test)]
#[derive(Default)]
struct ComparisonConstructionCounts {
    feature_rows: std::cell::Cell<usize>,
    cells: std::cell::Cell<usize>,
}

#[cfg(test)]
impl ComparisonConstructionCounts {
    /// Opens a draw. Counts describe one draw, never a running total.
    fn start_draw(&self) {
        self.feature_rows.set(0);
        self.cells.set(0);
    }

    fn count_feature_row(&self) {
        self.feature_rows.set(self.feature_rows.get() + 1);
    }

    fn count_cell(&self) {
        self.cells.set(self.cells.get() + 1);
    }
}

/// A controlled, intentionally bounded feature-comparison surface.
///
/// The application owns snapshot progress, highlighted/disabled item state,
/// and selected item identity. The entity retains only focus, measured row
/// heights, and overflow presentation state.
///
/// Feature rows are virtualized through `gpui::ListState`: the bound is 128
/// rows by 12 columns, and rendering all of it eagerly cost far more than a
/// frame budget because every mounted row is laid out whether or not it is on
/// screen. `ListState` rather than `gpui_base::v_virtual_list` because rows
/// wrap to variable heights and the latter needs exact heights up front.
/// Column headers sit outside the list, so they no longer scroll away
/// vertically; both they and the rows stay on one horizontally scrolled
/// canvas, which is what keeps a column aligned with its cells.
pub struct ComparisonTable {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    label: SharedString,
    snapshot: Progressive<ComparisonSnapshot>,
    selected_item_id: Option<SharedString>,
    focused_item_id: Option<SharedString>,
    focus_handle: FocusHandle,
    focus_engaged: bool,
    horizontal_scroll: ScrollHandle,
    feature_list: ListState,
    /// Rem size the cached row heights were measured against.
    resolved_layout: ResolvedLayoutKey,
    _focus_subscriptions: Vec<gpui::Subscription>,
    #[cfg(test)]
    construction: ComparisonConstructionCounts,
}

impl ComparisonTable {
    /// Creates an empty controlled comparison table with stable identity.
    pub fn new(
        id: impl Into<SharedString>,
        label: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let empty = ComparisonSnapshot {
            items: Arc::from([]),
            features: Arc::from([]),
        };
        let focus_handle = cx.focus_handle();
        let focus_subscriptions = vec![
            cx.on_focus(&focus_handle, window, |this, _, cx| {
                this.focus_engaged = true;
                this.reveal_focused_item(cx);
            }),
            cx.on_blur(&focus_handle, window, |this, _, _| {
                this.focus_engaged = false;
            }),
        ];
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            label: label.into(),
            snapshot: Progressive::pending(empty),
            selected_item_id: None,
            focused_item_id: None,
            focus_handle,
            focus_engaged: false,
            horizontal_scroll: ScrollHandle::new(),
            feature_list: ListState::new(
                0,
                ListAlignment::Top,
                window.rem_size() * FEATURE_OVERDRAW_REM,
            ),
            resolved_layout: ResolvedLayoutKey::default(),
            _focus_subscriptions: focus_subscriptions,
            #[cfg(test)]
            construction: ComparisonConstructionCounts::default(),
        }
    }

    /// Returns the `(feature rows, value cells)` the most recent draw built.
    #[cfg(test)]
    fn construction_counts(&self) -> (usize, usize) {
        (
            self.construction.feature_rows.get(),
            self.construction.cells.get(),
        )
    }

    /// Replaces the controlled progressive comparison snapshot.
    pub fn set_snapshot(
        &mut self,
        snapshot: Progressive<ComparisonSnapshot>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let previous_feature_ids = self
            .snapshot
            .content()
            .features
            .iter()
            .map(|feature| feature.id.clone())
            .collect::<Vec<_>>();
        self.snapshot = snapshot;
        self.reconcile_feature_list(&previous_feature_ids);
        if self.selected_item_id.as_ref().is_some_and(|selected| {
            !self
                .snapshot
                .content()
                .items
                .iter()
                .any(|item| item.id == *selected && item.state != ComparisonItemState::Disabled)
        }) {
            self.selected_item_id = None;
        }
        if self.focused_item_id.as_ref().is_none_or(|focused| {
            !self
                .snapshot
                .content()
                .items
                .iter()
                .any(|item| item.id == *focused && item.state != ComparisonItemState::Disabled)
        }) {
            self.focused_item_id = self
                .snapshot
                .content()
                .items
                .iter()
                .find(|item| item.state != ComparisonItemState::Disabled)
                .map(|item| item.id.clone());
        }
        if self.focus_engaged {
            self.reveal_focused_item(cx);
        }
        cx.notify();
    }

    /// Points the virtual list at the new feature sequence.
    ///
    /// A snapshot whose feature identities are unchanged is a content update —
    /// a streaming comparison filling in values — so it keeps the reader's
    /// scroll position and only invalidates the heights it may have changed.
    /// Any other snapshot is a different document, and starts at the top.
    fn reconcile_feature_list(&mut self, previous_feature_ids: &[SharedString]) {
        let features = &self.snapshot.content().features;
        let same_sequence = previous_feature_ids.len() == features.len()
            && previous_feature_ids
                .iter()
                .zip(features.iter())
                .all(|(previous, feature)| *previous == feature.id);
        if same_sequence {
            if !features.is_empty() {
                self.feature_list.remeasure_items(0..features.len());
            }
        } else {
            self.feature_list.reset(features.len());
        }
    }

    /// Re-measures feature rows after the window's rem size changed.
    ///
    /// Row heights cache wrapped text laid out at the previous rem, and no
    /// snapshot reports a zoom, so nothing else invalidates them. The anchor
    /// policy is this surface's own: the feature that was first on screen
    /// stays first, because a comparison is read from wherever the reader
    /// left it rather than from either end.
    fn resolve_layout(&mut self, rem_size: Pixels, cx: &mut Context<Self>) {
        if !self.resolved_layout.observe(rem_size) {
            return;
        }
        let offset = self.feature_list.logical_scroll_top();
        let anchor = self
            .snapshot
            .content()
            .features
            .get(offset.item_ix)
            .map(|feature| (feature.id.clone(), offset.offset_in_item));

        self.feature_list.remeasure();
        if let Some((anchor_id, offset_in_item)) = anchor
            && let Some(item_ix) = self
                .snapshot
                .content()
                .features
                .iter()
                .position(|feature| feature.id == anchor_id)
        {
            self.feature_list.scroll_to(ListOffset {
                item_ix,
                offset_in_item,
            });
        }
        cx.notify();
    }

    /// Replaces the controlled selected item when the stable item is enabled.
    pub fn set_selected_item(
        &mut self,
        item_id: impl Into<SharedString>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let item_id = item_id.into();
        if !self
            .snapshot
            .content()
            .items
            .iter()
            .any(|item| item.id == item_id && item.state != ComparisonItemState::Disabled)
        {
            return;
        }
        self.selected_item_id = Some(item_id);
        cx.notify();
    }

    /// Clears the current controlled item selection without changing focus.
    pub fn clear_selected_item(&mut self, cx: &mut Context<Self>) {
        self.selected_item_id = None;
        cx.notify();
    }

    /// Returns the current controlled selected stable item ID.
    pub fn selected_item_id(&self) -> Option<&str> {
        self.selected_item_id.as_deref()
    }

    /// Returns the current progressive comparison snapshot.
    pub fn snapshot(&self) -> &Progressive<ComparisonSnapshot> {
        &self.snapshot
    }

    /// Moves keyboard focus to an enabled stable item and reveals its column.
    pub fn focus_item(&mut self, item_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self
            .snapshot
            .content()
            .items
            .iter()
            .position(|item| item.id == item_id && item.state != ComparisonItemState::Disabled)
        else {
            return;
        };
        self.focused_item_id = Some(self.snapshot.content().items[index].id.clone());
        self.focus_engaged = true;
        self.reveal_item(index, cx);
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn request_selection(&self, item_id: SharedString, cx: &mut Context<Self>) {
        if self
            .snapshot
            .content()
            .items
            .iter()
            .any(|item| item.id == item_id && item.state != ComparisonItemState::Disabled)
        {
            cx.emit(ComparisonTableEvent::SelectionRequested {
                id: self.id.clone(),
                item_id,
            });
        }
    }

    fn move_item_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let enabled = self
            .snapshot
            .content()
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.state != ComparisonItemState::Disabled)
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if enabled.is_empty() {
            return;
        }
        let current = self
            .focused_item_id
            .as_ref()
            .and_then(|focused| {
                self.snapshot
                    .content()
                    .items
                    .iter()
                    .position(|item| item.id == *focused)
            })
            .and_then(|index| enabled.iter().position(|enabled| *enabled == index))
            .unwrap_or_default();
        let next = (current as isize + delta).rem_euclid(enabled.len() as isize) as usize;
        let index = enabled[next];
        self.focused_item_id = Some(self.snapshot.content().items[index].id.clone());
        self.focus_engaged = true;
        self.reveal_item(index, cx);
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn reveal_item(&self, index: usize, cx: &App) {
        let (feature_width, item_width, _) = comparison_layout(cx);
        let current = self.horizontal_scroll.offset();
        self.horizontal_scroll.set_offset(gpui::point(
            -(feature_width + item_width * index as f32),
            current.y,
        ));
    }

    fn reveal_focused_item(&self, cx: &App) {
        let Some(index) =
            self.focused_item_id.as_ref().and_then(|focused| {
                self.snapshot.content().items.iter().position(|item| {
                    item.id == *focused && item.state != ComparisonItemState::Disabled
                })
            })
        else {
            return;
        };
        self.reveal_item(index, cx);
    }

    /// Scrolls the stable feature row into the vertical viewport.
    pub fn scroll_to_feature(&mut self, feature_id: &str, cx: &mut Context<Self>) {
        let Some(index) = self
            .snapshot
            .content()
            .features
            .iter()
            .position(|feature| feature.id == feature_id)
        else {
            return;
        };
        // The list indexes features directly: column headers are no longer
        // items in the scrolled sequence, so nothing is added to the index.
        //
        // A row already whole on screen must not move — revealing a feature
        // the reader is looking at should be a no-op. Anywhere else the row
        // is anchored to the top of the viewport, which is exact whether or
        // not the rows in between have ever been measured. Reaching for a
        // pixel goal instead (`scroll_to_reveal_item`) reads heights the
        // virtual list has not measured yet and lands short of a distant row.
        // An unmeasured row reports no bounds, and that is a scroll.
        let viewport = self.feature_list.viewport_bounds();
        if self
            .feature_list
            .bounds_for_item(index)
            .is_some_and(|row| row.top() >= viewport.top() && row.bottom() <= viewport.bottom())
        {
            return;
        }
        self.feature_list.scroll_to(ListOffset {
            item_ix: index,
            offset_in_item: Pixels::ZERO,
        });
        cx.notify();
    }
}

/// Typed application intent emitted by a comparison table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComparisonTableEvent {
    /// An enabled item was chosen through pointer, keyboard, or accessibility activation.
    SelectionRequested {
        /// Stable comparison table ID.
        id: SharedString,
        /// Stable comparison item ID.
        item_id: SharedString,
    },
}

impl EventEmitter<ComparisonTableEvent> for ComparisonTable {}

impl Focusable for ComparisonTable {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

fn comparison_table_frame(
    table_id: &str,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    let identity: SharedString = format!("comparison-table-root:{table_id}").into();
    div()
        .id(identity.clone())
        .debug_selector(move || identity.to_string())
        .role(Role::Table)
        .aria_label(label.into())
}

fn comparison_layout(cx: &App) -> (gpui::Pixels, gpui::Pixels, gpui::Pixels) {
    let spacing = cx.theme().semantic_tokens().spacing;
    (
        spacing.xxl * 6. + spacing.xl,
        spacing.xxl * 5. + spacing.lg,
        spacing.xxl * 2. + spacing.sm,
    )
}

fn comparison_item_header_frame(
    table_id: &str,
    item: &ComparisonItem,
    selected: bool,
) -> gpui::Stateful<gpui::Div> {
    let debug_id: SharedString = format!("comparison-item-header:{table_id}:{}", item.id).into();
    let state_label = match item.state {
        ComparisonItemState::Default => None,
        ComparisonItemState::Highlighted => Some("Recommended"),
        ComparisonItemState::Disabled => Some("Unavailable"),
    };
    let accessible_label: SharedString = state_label
        .map(|state| format!("{}; {state}", item.label))
        .unwrap_or_else(|| item.label.to_string())
        .into();
    div()
        .id(debug_id.clone())
        .debug_selector(move || debug_id.to_string())
        .role(Role::ColumnHeader)
        .aria_label(accessible_label)
        .aria_selected(selected)
        .when_some(item.description.clone(), |header, description| {
            header.aria_description(description)
        })
}

fn comparison_item_control(
    table_id: &str,
    item: &ComparisonItem,
    window: &mut Window,
    cx: &mut App,
) -> gpui_base::Button {
    let debug_id: SharedString = format!("comparison-item-control:{table_id}:{}", item.id).into();
    outlined_control_with_wrapping_label(
        debug_id.clone(),
        format!("Select {}", item.label),
        item.label.clone(),
        window,
        cx,
    )
    .debug_selector(move || debug_id.to_string())
    .disabled(item.state == ComparisonItemState::Disabled)
    .when_some(item.description.clone(), |button, description| {
        button.aria_description(description)
    })
}

fn comparison_status_frame(
    table_id: &str,
    role: Role,
    label: SharedString,
) -> gpui::Stateful<gpui::Div> {
    let identity = format!("comparison-table-status:{table_id}");
    div()
        .id(identity.clone())
        .debug_selector(move || identity.clone())
        .role(role)
        .aria_label(label)
}
fn comparison_feature_row_frame(
    table_id: &str,
    feature: &ComparisonFeature,
) -> gpui::Stateful<gpui::Div> {
    let identity = format!("comparison-feature:{table_id}:{}", feature.id);
    div()
        .id(identity.clone())
        .debug_selector(move || identity.clone())
        .role(Role::Row)
        .aria_label(feature.label.clone())
        .when_some(feature.description.clone(), |row, description| {
            row.aria_description(description)
        })
}

fn comparison_feature_header_frame(
    table_id: &str,
    feature: &ComparisonFeature,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(format!(
            "comparison-feature-label:{table_id}:{}",
            feature.id
        ))
        .role(Role::RowHeader)
        .aria_label(feature.label.clone())
        .when_some(feature.description.clone(), |header, description| {
            header.aria_description(description)
        })
}

fn comparison_cell_frame(
    table_id: &str,
    feature_id: &str,
    item: &ComparisonItem,
    display: SharedString,
) -> gpui::Stateful<gpui::Div> {
    let identity = format!("comparison-cell:{table_id}:{feature_id}:{}", item.id);
    div()
        .id(identity.clone())
        .debug_selector(move || identity.clone())
        .role(Role::Cell)
        .aria_label(format!("{}: {display}", item.label))
        .aria_value(display)
}

impl ComparisonTable {
    /// Builds one feature row on demand for the virtual list.
    ///
    /// Only rows inside the viewport and its overdraw reach this, which is
    /// what keeps a frame's cost proportional to what is on screen rather
    /// than to the whole bounded snapshot. Every cell still carries the same
    /// selectable Markdown and the same semantics an eagerly built row did.
    fn render_feature_row(
        &mut self,
        index: usize,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let snapshot = self.snapshot.content();
        let (Some(feature), items) = (
            snapshot.features.get(index).cloned(),
            snapshot.items.clone(),
        ) else {
            return div().hidden().into_any_element();
        };
        #[cfg(test)]
        self.construction.count_feature_row();
        let tokens = cx.theme().semantic_tokens();
        let (feature_width, item_width, _) = comparison_layout(cx);

        comparison_feature_row_frame(&self.id, &feature)
            .flex()
            // A virtualized row is measured on its own, so it has to be told
            // it spans the grid; without that its columns have no width to
            // divide and each falls back to its floor, leaving the body
            // narrower than the header above it.
            .w_full()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                comparison_feature_header_frame(&self.id, &feature)
                    .w(feature_width)
                    .flex_none()
                    .p(tokens.spacing.sm)
                    .child(
                        TextView::markdown(
                            format!("comparison-feature-copy:{}:{}", self.id, feature.id),
                            escape_markdown_text(&feature.label),
                        )
                        .selectable(true),
                    )
                    .when_some(feature.description.clone(), |header, description| {
                        header.child(
                            TextView::markdown(
                                format!(
                                    "comparison-feature-description:{}:{}",
                                    self.id, feature.id
                                ),
                                escape_markdown_text(&description),
                            )
                            .selectable(true),
                        )
                    }),
            )
            .children(items.iter().map(|item| {
                #[cfg(test)]
                self.construction.count_cell();
                let display: SharedString = feature
                    .value(&item.id)
                    .map(|value| value.display.clone())
                    .unwrap_or_else(|| "Not specified".into());
                comparison_cell_frame(&self.id, &feature.id, item, display.clone())
                    // Fluid, and for the reason the header gives: a cell holds
                    // the column its header names, so the two divide the width
                    // by the same rule or they stop lining up.
                    .flex_1()
                    .min_w(item_width)
                    .p(tokens.spacing.sm)
                    .border_l_1()
                    .border_color(cx.theme().border)
                    .when(item.state == ComparisonItemState::Highlighted, |cell| {
                        cell.bg(cx.theme().accent)
                    })
                    .child(
                        TextView::markdown(
                            format!(
                                "comparison-cell-copy:{}:{}:{}",
                                self.id, feature.id, item.id
                            ),
                            escape_markdown_text(&display),
                        )
                        .selectable(true),
                    )
            }))
            .into_any_element()
    }
}

impl gpui::Styled for ComparisonTable {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for ComparisonTable {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(test)]
        self.construction.start_draw();
        // The rem is a resolved-layout input, so a change to it invalidates
        // every measured row height. Reading it here costs nothing; the
        // reaction is deferred so that render itself neither mutates nor
        // notifies.
        let rem_size = window.rem_size();
        if !self.resolved_layout.matches(rem_size) {
            cx.defer_in(window, move |table, _, cx| {
                table.resolve_layout(rem_size, cx);
            });
        }
        let tokens = cx.theme().semantic_tokens();
        let owner = cx.weak_entity();
        let navigation_owner = owner.clone();
        let items = self.snapshot.content().items.clone();
        let features = self.snapshot.content().features.clone();
        let (feature_width, item_width, header_height) = comparison_layout(cx);
        let status = match self.snapshot.state() {
            ProgressState::Pending => Some((Role::ProgressIndicator, "Loading comparison".into())),
            ProgressState::Running => Some((Role::ProgressIndicator, "Updating comparison".into())),
            ProgressState::Failed(reason) => Some((Role::Alert, reason.clone())),
            ProgressState::Complete if items.is_empty() || features.is_empty() => {
                Some((Role::Status, "No comparison data".into()))
            }
            ProgressState::Complete => None,
        };

        comparison_table_frame(&self.id, self.label.clone())
            .size_full()
            .min_h_0()
            .relative()
            .overflow_hidden()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(tokens.radius.lg)
            .child(
                div()
                    .id(format!("comparison-table-scroll:{}", self.id))
                    .size_full()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .overflow_x_scroll()
                    .lock_scroll_axis()
                    .track_scroll(&self.horizontal_scroll)
                    .policy_horizontal_scrollbar(&self.horizontal_scroll, cx)
                    .when_some(status, |surface, (role, label): (Role, SharedString)| {
                        // Status states carry the same semantic color language as
                        // Task Rows: info for in-flight work, danger for failures,
                        // muted for empty. A spinner accompanies loading so the
                        // state is visible, not just readable.
                        fn status_visuals(role: Role, cx: &App) -> (Hsla, IconName) {
                            match role {
                                Role::ProgressIndicator => {
                                    (cx.theme().info, IconName::LoaderCircle)
                                }
                                Role::Alert => (cx.theme().danger, IconName::CircleX),
                                _ => (cx.theme().muted_foreground, IconName::Dash),
                            }
                        }
                        let spinner = role == Role::ProgressIndicator;
                        // A settled empty comparison takes the shared
                        // empty-state anatomy; in-flight and failed states
                        // keep the inline status row, and loading adds a
                        // skeleton in the coming table's shape below it.
                        if role == Role::Status {
                            return surface.child(
                                comparison_status_frame(&self.id, role, label.clone())
                                    .p(tokens.spacing.md)
                                    .child(crate::surface::empty_state(
                                        IconName::Inbox,
                                        label,
                                        None,
                                        cx,
                                    )),
                            );
                        }
                        let skeleton_columns = items.len().max(3);
                        let (color, text) = (status_visuals(role, cx).0, label.clone());
                        surface.child(
                            comparison_status_frame(&self.id, role, label.clone())
                                .p(tokens.spacing.md)
                                .flex()
                                .flex_col()
                                .gap(tokens.spacing.md)
                                .child(
                                    h_flex()
                                        .gap(tokens.spacing.sm)
                                        .items_center()
                                        .when(spinner, |row| {
                                            let spin =
                                                crate::motion::MotionTokens::effective_preference(
                                                    cx,
                                                ) != crate::motion::MotionPreference::Snap;
                                            // The animated element must be the direct
                                            // child; wrap the rotating icon in a
                                            // fixed-size slot so layout stays stable.
                                            let icon = status_visuals(role, cx).1;
                                            row.child(
                                                div().size_4().child(
                                                    Icon::new(icon)
                                                        .text_color(color)
                                                        .with_visible_animation(
                                                            "comparison-status-spinner",
                                                            // Frame demand: active only for a
                                                            // ProgressIndicator status. Settled
                                                            // statuses take the branch below and
                                                            // render a still icon. Reduced motion
                                                            // holds delta at 0 — an unrotated icon.
                                                            MotionTokens::read(cx)
                                                                .status_spinner()
                                                                .looping(),
                                                            move |this, delta| {
                                                                let delta =
                                                                    if spin { delta } else { 0.0 };
                                                                this.rotate(gpui::percentage(delta))
                                                            },
                                                        )
                                                        .into_any_element(),
                                                ),
                                            )
                                        })
                                        .when(!spinner, |row| {
                                            let icon = status_visuals(role, cx).1;
                                            row.child(Icon::new(icon).size_4().text_color(color))
                                        })
                                        .child(
                                            div()
                                                .text_token(tokens.typography.sm)
                                                .text_color(color)
                                                .child(text),
                                        ),
                                )
                                .when(spinner, |frame| {
                                    frame.child(div().w_full().child(
                                        crate::surface::skeleton_rows(
                                            ElementId::from((
                                                ElementId::from(self.id.clone()),
                                                "loading-skeleton",
                                            )),
                                            3,
                                            skeleton_columns,
                                            cx,
                                        ),
                                    ))
                                }),
                        )
                    })
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .flex_1()
                            .min_h_0()
                            // The grid's intrinsic width is feature column + N item
                            // columns. Only the outer viewport owns horizontal
                            // movement, keeping keyboard reveal, scrollbar drag,
                            // headers, and cells on one canvas.
                            .min_w(feature_width + item_width * items.len() as f32)
                            .child(
                                div()
                                    .id(format!("comparison-table-rows:{}", self.id))
                                    .size_full()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .role(Role::RowGroup)
                                    .child(
                                        div()
                                            .id(format!("comparison-table-header-row:{}", self.id))
                                            .flex()
                                            // Column headers sit outside the scrolled
                                            // list, so they stay put while features
                                            // scroll under them, and their controls
                                            // stay reachable from the keyboard.
                                            .flex_none()
                                            .role(Role::Row)
                                            .child(
                                                div()
                                                    .id(format!(
                                                        "comparison-feature-header:{}",
                                                        self.id
                                                    ))
                                                    .w(feature_width)
                                                    .flex_none()
                                                    .flex()
                                                    .flex_col()
                                                    .gap(tokens.spacing.xxs)
                                                    .p(tokens.spacing.sm)
                                                    .role(Role::ColumnHeader)
                                                    .aria_label("Feature"),
                                            )
                                            .children(items.iter().map(|item| {
                                                let item_id = item.id.clone();
                                                let handler_owner = owner.clone();
                                                let selected = self.selected_item_id.as_ref()
                                                    == Some(&item.id);
                                                // The selection a table mounts
                                                // with is placed settled; a
                                                // column the user selects fades
                                                // its marker in once at the
                                                // quick tempo.
                                                let acknowledged =
                                                    crate::motion::acknowledged_state(
                                                        ElementId::Name(
                                                            format!(
                                                                "comparison-selected:{}:{}",
                                                                self.id, item.id
                                                            )
                                                            .into(),
                                                        ),
                                                        selected as u64,
                                                        window,
                                                        cx,
                                                    );
                                                let focused =
                                                    self.focused_item_id.as_ref() == Some(&item.id);
                                                let state_label = match item.state {
                                                    ComparisonItemState::Default => None,
                                                    ComparisonItemState::Highlighted => {
                                                        Some("Recommended")
                                                    }
                                                    ComparisonItemState::Disabled => {
                                                        Some("Unavailable")
                                                    }
                                                };
                                                comparison_item_header_frame(
                                                    &self.id, item, selected,
                                                )
                                                // Fluid: the columns divide
                                                // whatever width the table is
                                                // given, rather than each
                                                // taking a fixed slice and
                                                // leaving the remainder as a
                                                // band of empty table beside
                                                // them. The fixed width stays
                                                // on as the floor, so a narrow
                                                // viewport scrolls sideways
                                                // instead of squeezing the
                                                // columns past reading.
                                                .flex_1()
                                                .min_w(item_width)
                                                .min_h(header_height)
                                                .p(tokens.spacing.sm)
                                                .border_l_1()
                                                .border_color(cx.theme().border)
                                                .when(
                                                    item.state == ComparisonItemState::Highlighted,
                                                    |header| header.bg(cx.theme().accent),
                                                )
                                                .when(selected, |header| {
                                                    // Selection is the shared
                                                    // fill grammar, not a ring:
                                                    // an inset rounded wash on
                                                    // an absolute overlay, so
                                                    // the marker cannot reflow
                                                    // the header it sits under.
                                                    // Painted first, it stays
                                                    // beneath the content.
                                                    header.child(
                                                        div()
                                                            .absolute()
                                                            .inset(tokens.spacing.xxs)
                                                            .rounded(crate::surface::nested_radius(
                                                                tokens.radius.lg,
                                                                tokens.spacing.xxs,
                                                                tokens.radius.sm,
                                                            ))
                                                            .bg(cx
                                                                .theme()
                                                                .list_active
                                                                .opacity(acknowledged)),
                                                    )
                                                })
                                                .child(
                                                    comparison_item_control(
                                                        &self.id, item, window, cx,
                                                    )
                                                    .w_full()
                                                    .tab_stop(focused)
                                                    .when(focused, |button| {
                                                        button.track_focus(&self.focus_handle)
                                                    })
                                                    .on_click(move |_, window, cx| {
                                                        let _ = handler_owner.update(
                                                            cx,
                                                            |table, cx| {
                                                                table.focus_item(
                                                                    &item_id, window, cx,
                                                                );
                                                                table.request_selection(
                                                                    item_id.clone(),
                                                                    cx,
                                                                );
                                                            },
                                                        );
                                                    }),
                                                )
                                                .when_some(
                                                    item.description.clone(),
                                                    |header, description| {
                                                        header.child(
                                            TextView::markdown(
                                                format!(
                                                    "comparison-item-description:{}:{}",
                                                    self.id, item.id
                                                ),
                                                escape_markdown_text(&description),
                                            )
                                            .selectable(true),
                                        )
                                                    },
                                                )
                                                .when_some(state_label, |header, state| {
                                                    header.child(
                                                        crate::surface::hint(state, cx)
                                                            .mt(tokens.spacing.xs),
                                                    )
                                                })
                                                .child(
                                                    div()
                                                        .mt(tokens.spacing.xs)
                                                        .text_token(tokens.typography.xs)
                                                        .opacity(if selected {
                                                            acknowledged
                                                        } else {
                                                            0.0
                                                        })
                                                        .child("Selected"),
                                                )
                                            }))
                                            .on_key_down(move |event, window, cx| {
                                                let delta = match event.keystroke.key.as_str() {
                                                    "left" => -1,
                                                    "right" => 1,
                                                    _ => return,
                                                };
                                                let _ = navigation_owner.update(cx, |table, cx| {
                                                    table.move_item_focus(delta, window, cx);
                                                });
                                                cx.stop_propagation();
                                            }),
                                    )
                                    .child(
                                        div()
                                            .id(format!("comparison-table-features:{}", self.id))
                                            .relative()
                                            .flex_1()
                                            .min_h_0()
                                            .overflow_hidden()
                                            .policy_vertical_scrollbar(&self.feature_list, cx)
                                            .child(
                                                list(
                                                    self.feature_list.clone(),
                                                    cx.processor(Self::render_feature_row),
                                                )
                                                .size_full(),
                                            ),
                                    ),
                            )
                            .child(list_scroll_mask(&self.feature_list)),
                    ),
            )
            .child(
                ScrollableMask::new(Axis::Horizontal, &self.horizontal_scroll)
                    .id((ElementId::from(self.id.clone()), "horizontal-scroll-mask")),
            )
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Element as _, Entity, RenderOnce as _, TestAppContext, VisualTestContext, accesskit,
        canvas, px, size,
    };
    use std::{
        sync::{Arc, Mutex},
        time::{Duration, Instant},
    };

    type CapturedNodes = Arc<Mutex<Option<[(Option<Role>, accesskit::Node); 9]>>>;

    struct SemanticsProbe {
        captured: CapturedNodes,
    }

    impl Render for SemanticsProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let captured = self.captured.clone();
            canvas(
                move |_, window, cx| {
                    let root = comparison_table_frame("plans", "Plan comparison").into_element();
                    let business = ComparisonItem::new("business", "Business")
                        .description("Best for growing teams")
                        .state(ComparisonItemState::Highlighted);
                    let header =
                        comparison_item_header_frame("plans", &business, true).into_element();
                    let enabled_control = comparison_item_control("plans", &business, window, cx)
                        .on_click(|_, _, _| {})
                        .render(window, cx)
                        .into_element();
                    let control = comparison_item_control(
                        "plans",
                        &ComparisonItem::new("legacy", "Legacy")
                            .state(ComparisonItemState::Disabled),
                        window,
                        cx,
                    )
                    .on_click(|_, _, _| {})
                    .render(window, cx)
                    .into_element();
                    let disabled_header = comparison_item_header_frame(
                        "plans",
                        &ComparisonItem::new("legacy", "Legacy")
                            .state(ComparisonItemState::Disabled),
                        false,
                    )
                    .into_element();
                    let feature = ComparisonFeature::new("support", "Priority support")
                        .description("Response within four hours");
                    let row = comparison_feature_row_frame("plans", &feature).into_element();
                    let row_header =
                        comparison_feature_header_frame("plans", &feature).into_element();
                    let cell =
                        comparison_cell_frame("plans", "support", &business, "Included".into())
                            .into_element();
                    let status = comparison_status_frame(
                        "plans",
                        Role::ProgressIndicator,
                        "Loading comparison".into(),
                    )
                    .into_element();
                    let mut root_node = accesskit::Node::new(Role::Unknown);
                    root.write_a11y_info(&mut root_node);
                    let mut header_node = accesskit::Node::new(Role::Unknown);
                    header.write_a11y_info(&mut header_node);
                    let mut control_node = accesskit::Node::new(Role::Unknown);
                    control.write_a11y_info(&mut control_node);
                    let mut disabled_header_node = accesskit::Node::new(Role::Unknown);
                    disabled_header.write_a11y_info(&mut disabled_header_node);
                    let mut enabled_control_node = accesskit::Node::new(Role::Unknown);
                    enabled_control.write_a11y_info(&mut enabled_control_node);
                    let mut row_node = accesskit::Node::new(Role::Unknown);
                    row.write_a11y_info(&mut row_node);
                    let mut row_header_node = accesskit::Node::new(Role::Unknown);
                    row_header.write_a11y_info(&mut row_header_node);
                    let mut cell_node = accesskit::Node::new(Role::Unknown);
                    cell.write_a11y_info(&mut cell_node);
                    let mut status_node = accesskit::Node::new(Role::Unknown);
                    status.write_a11y_info(&mut status_node);
                    *captured.lock().expect("capture mutex should be available") = Some([
                        (root.a11y_role(), root_node),
                        (header.a11y_role(), header_node),
                        (control.a11y_role(), control_node),
                        (disabled_header.a11y_role(), disabled_header_node),
                        (enabled_control.a11y_role(), enabled_control_node),
                        (row.a11y_role(), row_node),
                        (row_header.a11y_role(), row_header_node),
                        (cell.a11y_role(), cell_node),
                        (status.a11y_role(), status_node),
                    ]);
                },
                |_, _, _, _| {},
            )
        }
    }

    #[gpui::test]
    fn comparison_semantics_name_table_selected_highlight_and_disabled_action(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(move |_, _| SemanticsProbe { captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let [
            root,
            header,
            disabled,
            disabled_header,
            enabled,
            row,
            row_header,
            cell,
            status,
        ] = result
            .lock()
            .expect("capture mutex should be available")
            .take()
            .expect("semantic nodes should be captured");

        assert_eq!(root.0, Some(Role::Table));
        assert_eq!(root.1.label(), Some("Plan comparison"));
        assert_eq!(header.0, Some(Role::ColumnHeader));
        assert_eq!(header.1.label(), Some("Business; Recommended"));
        assert_eq!(header.1.description(), Some("Best for growing teams"));
        assert_eq!(header.1.is_selected(), Some(true));
        assert_eq!(disabled.0, Some(Role::Button));
        assert!(!disabled.1.supports_action(accesskit::Action::Click));
        assert_eq!(disabled_header.1.label(), Some("Legacy; Unavailable"));
        assert_eq!(enabled.0, Some(Role::Button));
        assert_eq!(enabled.1.label(), Some("Select Business"));
        assert_eq!(enabled.1.description(), Some("Best for growing teams"));
        assert!(enabled.1.supports_action(accesskit::Action::Click));
        assert_eq!(row.0, Some(Role::Row));
        assert_eq!(row.1.label(), Some("Priority support"));
        assert_eq!(row.1.description(), Some("Response within four hours"));
        assert_eq!(row_header.0, Some(Role::RowHeader));
        assert_eq!(row_header.1.label(), Some("Priority support"));
        assert_eq!(cell.0, Some(Role::Cell));
        assert_eq!(cell.1.label(), Some("Business: Included"));
        assert_eq!(cell.1.value(), Some("Included"));
        assert_eq!(status.0, Some(Role::ProgressIndicator));
        assert_eq!(status.1.label(), Some("Loading comparison"));
    }

    /// Viewport used by every construction measurement below. A fixed size
    /// keeps the numbers comparable between runs; it is smaller than the
    /// bounded grid on both axes, so the measurement covers the scrolled case
    /// the surface actually ships in.
    const MEASURED_VIEWPORT: (f32, f32) = (900., 600.);

    /// Largest feature-row window a draw at [`MEASURED_VIEWPORT`] may build.
    ///
    /// The viewport is 600 px, a described row measures around 173 px, and
    /// the list keeps [`FEATURE_OVERDRAW_REM`] measured past the fold, so the
    /// window runs to about five rows. Eight leaves room for measurement to
    /// move without coming anywhere near [`MAX_COMPARISON_FEATURES`], which
    /// is the number this guard exists to keep a frame away from.
    const MAX_WINDOWED_ROWS: usize = 8;

    /// Timed draws per shape: enough for a stable median and a p95 that is a
    /// real sample rather than the single worst draw.
    const TIMED_DRAWS: usize = 40;

    /// Draws discarded before timing. The first draws pay one-time font, glyph,
    /// and text-layout caching that no steady-state frame repeats.
    const WARMUP_DRAWS: usize = 5;

    /// Worst-case content for a bounded shape: every item and every feature
    /// carries a description, so each row builds the most selectable Markdown
    /// views the contract allows, and every cell has a value to render.
    /// Item columns divide the table's width, and a cell sits under its own
    /// header.
    ///
    /// The columns were fixed slices: three plans in a table with room for
    /// five left a band of empty table beside them, which reads as a table
    /// that failed to draw rather than one with room to spare. They divide
    /// what they are given now, and the same rule has to reach the rows -
    /// they are virtualized and measured apart from the header, so getting
    /// this wrong shows up as a body narrower than the header above it.
    #[gpui::test]
    fn item_columns_fill_the_table_and_cells_line_up_with_their_header(cx: &mut TestAppContext) {
        let (table, cx) = measured_table(cx);
        draw_shape(&table, cx, 3, 3);
        redraw(&table, cx);

        let frame = cx
            .debug_bounds("comparison-table-root:measured")
            .expect("the table should render");
        let last = cx
            .debug_bounds("comparison-item-header:measured:item-2")
            .expect("the last column header should render");
        let cell = cx
            .debug_bounds("comparison-cell:measured:feature-0:item-2")
            .expect("the last column's first cell should render");

        // The frame's own border is the only thing that may stand between the
        // last column and the edge of the table.
        assert_eq!(
            frame.right() - last.right(),
            px(1.),
            "the columns leave a band of empty table: header ends {:?} short of the frame",
            frame.right() - last.right()
        );
        assert!(
            last.size.width > comparison_layout_item_width(cx),
            "a column that fills is wider than the floor it may not go below: {:?}",
            last.size.width
        );
        assert_eq!(
            (cell.left(), cell.right()),
            (last.left(), last.right()),
            "a cell must sit under the header of its own column: cell {cell:?}, header {last:?}"
        );
    }

    #[gpui::test]
    fn selecting_a_column_keeps_the_header_height_and_acknowledges_once(cx: &mut TestAppContext) {
        let (table, cx) = measured_table(cx);
        draw_shape(&table, cx, 4, 3);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        redraw(&table, cx);
        let header = |cx: &mut VisualTestContext| {
            cx.debug_bounds("comparison-item-header:measured:item-1")
                .expect("the header should render")
        };
        let unselected = header(cx);

        crate::motion::take_reveal_frame_requests();
        cx.update(|window, cx| {
            table.update(cx, |table, cx| {
                table.set_selected_item("item-1", window, cx);
            });
        });
        redraw(&table, cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "selecting a column must acknowledge the new marker"
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        redraw(&table, cx);
        let selected = header(cx);
        assert_eq!(
            selected.size.height, unselected.size.height,
            "the Selected label lives in a reserved slot; selection must not reflow the header"
        );
    }

    fn bounded_snapshot(features: usize, items: usize) -> ComparisonSnapshot {
        let item_ids = (0..items)
            .map(|index| SharedString::from(format!("item-{index}")))
            .collect::<Vec<_>>();
        ComparisonSnapshot::try_new(
            item_ids.iter().enumerate().map(|(index, id)| {
                ComparisonItem::new(id.clone(), format!("Plan {index}"))
                    .description("Readable supporting copy for one comparison column")
            }),
            (0..features).map(|feature| {
                ComparisonFeature::new(format!("feature-{feature}"), format!("Feature {feature}"))
                    .description(
                        "Selectable supporting detail that makes every feature row variable height",
                    )
                    .values(
                        item_ids
                            .iter()
                            .map(|id| ComparisonValue::new(id.clone(), format!("Value {feature}"))),
                    )
            }),
        )
        .expect("the fixture should stay inside the bounded contract")
    }

    /// Applies one bounded shape, draws it, and returns what that draw built.
    fn draw_shape(
        table: &Entity<ComparisonTable>,
        cx: &mut VisualTestContext,
        features: usize,
        items: usize,
    ) -> (usize, usize) {
        cx.update(|window, cx| {
            table.update(cx, |table, cx| {
                table.set_snapshot(
                    Progressive::complete(bounded_snapshot(features, items)),
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        table.read_with(cx, |table, _| table.construction_counts())
    }

    /// Redraws without touching the snapshot — the frame a selection, scroll,
    /// or theme change produces — and returns what that draw built.
    fn redraw(table: &Entity<ComparisonTable>, cx: &mut VisualTestContext) -> (usize, usize) {
        cx.update(|window, cx| {
            table.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        table.read_with(cx, |table, _| table.construction_counts())
    }

    /// Redraws once and returns the wall-clock cost of the whole draw, layout
    /// and paint included, alongside what it built.
    fn timed_redraw(
        table: &Entity<ComparisonTable>,
        cx: &mut VisualTestContext,
    ) -> (Duration, (usize, usize)) {
        let elapsed = cx.update(|window, cx| {
            table.update(cx, |_, cx| cx.notify());
            let started = Instant::now();
            window.draw(cx).clear(cx);
            started.elapsed()
        });
        (
            elapsed,
            table.read_with(cx, |table, _| table.construction_counts()),
        )
    }

    /// Nearest-rank percentile over ascending samples.
    fn percentile(ascending: &[Duration], percent: usize) -> Duration {
        let rank = (ascending.len() * percent).div_ceil(100).max(1);
        ascending[rank - 1]
    }

    /// A table in a window sized to [`MEASURED_VIEWPORT`], ready to be drawn.
    /// The width one item column may not go below, for the fluid-column test.
    fn comparison_layout_item_width(cx: &mut VisualTestContext) -> gpui::Pixels {
        cx.update(|_, cx| comparison_layout(cx).1)
    }

    fn measured_table(
        cx: &mut TestAppContext,
    ) -> (Entity<ComparisonTable>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (table, cx) = cx.add_window_view(|window, cx| {
            ComparisonTable::new("measured", "Measured comparison", window, cx)
        });
        let cx: &mut VisualTestContext = cx;
        cx.simulate_resize(size(px(MEASURED_VIEWPORT.0), px(MEASURED_VIEWPORT.1)));
        (table, cx)
    }

    /// A bound the render draws in full is not a bound on frame cost. Every
    /// mounted row is laid out whether or not it is on screen, so before
    /// virtualization a maximum-shape draw laid out 128 rows to show three
    /// and a half of them.
    ///
    /// This is the guard on the window: a draw builds the rows the viewport
    /// can show plus overdraw, one cell per item in each, and neither the
    /// snapshot's length nor the number of frames drawn widens it.
    #[gpui::test]
    fn a_draw_builds_the_viewport_window_rather_than_the_whole_snapshot(cx: &mut TestAppContext) {
        let (table, cx) = measured_table(cx);

        let (maximum_rows, maximum_cells) =
            draw_shape(&table, cx, MAX_COMPARISON_FEATURES, MAX_COMPARISON_ITEMS);
        assert!(
            (1..=MAX_WINDOWED_ROWS).contains(&maximum_rows),
            "the maximum snapshot must build a window, not its whole self: \
             {maximum_rows} rows"
        );
        assert_eq!(
            maximum_cells,
            maximum_rows * MAX_COMPARISON_ITEMS,
            "every constructed row must build one cell per item"
        );

        // Frames after the first must not creep upward: the window is the
        // viewport's size, and redrawing does not widen it or accumulate.
        for _ in 0..3 {
            let (rows, cells) = redraw(&table, cx);
            assert!(
                (1..=maximum_rows).contains(&rows),
                "a redraw must not widen the window: {rows} rows"
            );
            assert_eq!(cells, rows * MAX_COMPARISON_ITEMS);
        }

        let (typical_rows, typical_cells) = draw_shape(&table, cx, 24, 6);
        assert_eq!(
            typical_cells,
            typical_rows * 6,
            "a typical snapshot must also build one cell per item"
        );
        // The point of the whole change: rows of the same height cost the
        // same per frame whether the snapshot holds 24 of them or 128.
        assert_eq!(
            typical_rows, maximum_rows,
            "the window must follow the viewport, not the snapshot's length"
        );

        assert_eq!(
            draw_shape(&table, cx, 0, 0),
            (0, 0),
            "an empty snapshot must build no rows at all"
        );
    }

    /// Draw cost at the bounded maximum, printed rather than asserted:
    /// wall-clock is machine- and load-dependent, so a threshold here would
    /// gate on noise. The release budget belongs to the release-profile
    /// harness. What this test does enforce is that every timed draw built
    /// only the visible window, so the numbers describe the work they claim.
    ///
    /// Ignored by default because it exists for its printout, not an
    /// assertion. Run it deliberately, in the profile you mean to measure:
    ///
    /// ```text
    /// cargo test --release -p gpui-ai --lib comparison -- --ignored --nocapture
    /// ```
    ///
    /// The record that decided this surface's shape, both on desktop Windows
    /// at a 900x600 viewport, 40 draws after warmup:
    ///
    /// ```text
    /// eager (removed), debug profile, whole bound built every draw:
    /// 128 x 12 -> 1536 cells | p50 1463..1523 ms | p95 1555..1817 ms
    /// construction was ~18 ms of a ~1140 ms draw; the rest was layout and
    /// paint of a 22,144 px grid with ~3.5 rows on screen (~29x overdraw).
    ///
    /// virtualized (current), release profile, viewport window only:
    /// 128 x 12 ->  3 rows / 36 cells | p50 2.420 ms | p95 2.525 ms
    ///  24 x  6 ->  3 rows / 18 cells | p50 1.400 ms | p95 1.478 ms
    /// ```
    ///
    /// The eager debug row is not comparable to the release row as a
    /// profile; it is kept because the ~29x overdraw it documents is
    /// profile-independent and is the reason the list is virtualized.
    #[gpui::test]
    #[ignore = "measurement, not a gate: exists for its printout"]
    fn the_bounded_maximum_draw_cost_is_measured_against_the_frame_budget(cx: &mut TestAppContext) {
        let (table, cx) = measured_table(cx);
        // The profile comes from the invocation, so the header must not name
        // one: this same test prints debug or release numbers.
        let mut report = format!(
            "\ncomparison draw cost — profile per invocation, \
             {}x{} viewport, {TIMED_DRAWS} draws after {WARMUP_DRAWS} warmup\n",
            MEASURED_VIEWPORT.0, MEASURED_VIEWPORT.1
        );

        for (features, items) in [(MAX_COMPARISON_FEATURES, MAX_COMPARISON_ITEMS), (24, 6)] {
            draw_shape(&table, cx, features, items);
            for _ in 0..WARMUP_DRAWS {
                redraw(&table, cx);
            }

            let mut samples = Vec::with_capacity(TIMED_DRAWS);
            let mut built = (0, 0);
            for _ in 0..TIMED_DRAWS {
                let (elapsed, counts) = timed_redraw(&table, cx);
                assert!(
                    counts.0 <= MAX_WINDOWED_ROWS && counts.1 == counts.0 * items,
                    "a timed draw must build a window of whole rows: {counts:?}"
                );
                built = counts;
                samples.push(elapsed);
            }
            samples.sort();

            let milliseconds = |sample: Duration| sample.as_secs_f64() * 1000.;
            report.push_str(&format!(
                "  {features:>3} features x {items:>2} items -> {:>4} rows, {:>5} cells built | \
                 p50 {:>8.3} ms | p95 {:>8.3} ms\n",
                built.0,
                built.1,
                milliseconds(percentile(&samples, 50)),
                milliseconds(percentile(&samples, 95)),
            ));
        }

        eprintln!("{report}");
    }
}
