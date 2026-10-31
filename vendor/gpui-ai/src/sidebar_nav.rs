//! Stable-ID, filterable navigation composed over gpui-component's Sidebar primitives.
//!
//! The entity's only rendering input is one flattened snapshot of visible
//! rows. Sections, items, filtering, and expansion resolve into a single
//! `[VisibleRow]` recomputed when the state it derives from changes and never
//! from `Render`, so virtualization spans rows rather than section roots: a
//! section holding ten thousand expanded descendants constructs the same
//! bounded window as a section holding ten.
//!
//! Flattening costs the tree relationships that element nesting used to
//! carry, so every row states them: level, parent, and position within its
//! visible sibling set. Those fields are what the ARIA tree keyboard model
//! walks and what the AccessKit nodes report, which is the sanctioned way to
//! describe a tree whose rows cannot contain one another.

mod render;
mod rows;
#[cfg(test)]
mod tests;

use gpui_base::StyledExt as _;
use std::{cell::Cell, collections::HashSet, rc::Rc, sync::Arc};

use gpui::{
    AnyElement, App, AppContext as _, Context, Div, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable as _, InteractiveElement as _, IntoElement, ListAlignment, ListState,
    ParentElement as _, Pixels, Render, Role, SharedString, Stateful,
    StatefulInteractiveElement as _, Styled as _, Subscription, Window, div, list,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, IconName, h_flex,
    input::{Input, InputEvent, InputState},
    v_flex,
};

use crate::scrolling::PolicyScrollbarExt as _;
use crate::{
    motion::disclosure_progress, resolved_layout::ResolvedLayoutKey, scrolling::list_scroll_mask,
};

use render::{nav_control, render_row, sidebar_tree_container};
use rows::{VisibleRow, collect_parent_ids, snapshot_ids_are_unique};

/// One application-owned recursive sidebar item.
///
/// IDs must be stable and globally unique inside a sidebar snapshot. Labels
/// may be duplicated because every interaction is routed by `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarNavItem {
    id: SharedString,
    label: SharedString,
    icon: Option<SharedString>,
    badge: Option<SharedString>,
    detail: Option<SharedString>,
    trailing: Option<SharedString>,
    disabled: bool,
    children: Arc<[SidebarNavItem]>,
}

impl SidebarNavItem {
    /// Create an enabled item with stable application identity.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            badge: None,
            detail: None,
            trailing: None,
            disabled: false,
            children: Arc::from([]),
        }
    }

    /// Set the optional leading icon.
    pub fn icon(mut self, icon: impl gpui_component::IconNamed) -> Self {
        self.icon = Some(icon.path());
        self
    }

    /// Set compact trailing badge text.
    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    /// Set a secondary line under the label, such as a timestamp and count.
    ///
    /// A row with a detail line is taller; the list measures the row, so the
    /// rhythm adapts instead of assuming one label height.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Set a short right-aligned label on the title line, such as a time.
    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    /// Set whether every activation path is unavailable.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Replace the recursive child snapshot.
    pub fn children<I>(mut self, children: I) -> Self
    where
        I: IntoIterator<Item = SidebarNavItem>,
    {
        self.children = children.into_iter().collect();
        self
    }

    /// Return the stable application identity.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Return the visible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Return the optional leading icon.
    pub fn icon_path(&self) -> Option<&SharedString> {
        self.icon.as_ref()
    }

    /// Return optional trailing badge text.
    pub fn badge_text(&self) -> Option<&SharedString> {
        self.badge.as_ref()
    }

    /// Return the optional secondary line.
    pub fn detail_text(&self) -> Option<&SharedString> {
        self.detail.as_ref()
    }

    /// Return whether every activation path is unavailable.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    /// Return the recursive child snapshot.
    pub fn child_items(&self) -> &[SidebarNavItem] {
        &self.children
    }
}

/// One application-owned labeled section of sidebar items.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarSection {
    id: SharedString,
    label: SharedString,
    items: Arc<[SidebarNavItem]>,
}

impl SidebarSection {
    /// Create an empty section with stable identity.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            items: Arc::from([]),
        }
    }

    /// Replace the section's recursive item snapshot.
    pub fn items<I>(mut self, items: I) -> Self
    where
        I: IntoIterator<Item = SidebarNavItem>,
    {
        self.items = items.into_iter().collect();
        self
    }

    /// Return the stable section identity.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Return the visible section label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Return the recursive item snapshot.
    pub fn nav_items(&self) -> &[SidebarNavItem] {
        &self.items
    }
}

/// A typed application intent emitted by [`SidebarNav`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarNavEvent {
    /// An enabled item was activated by pointer, keyboard, or accessibility.
    ///
    /// Activating a parent intentionally toggles its transient expansion and
    /// emits `Selected` in the same interaction so applications may navigate
    /// to parent routes as well as expose their children.
    Selected {
        /// Sidebar component identity.
        id: SharedString,
        /// Stable application item identity.
        item_id: SharedString,
    },
    /// The sidebar's owned collapsed state changed.
    CollapsedChanged {
        /// Sidebar component identity.
        id: SharedString,
        /// Latest collapsed state.
        collapsed: bool,
    },
    /// The named new-task control was activated.
    NewTaskRequested {
        /// Sidebar component identity.
        id: SharedString,
    },
    /// The native quick-filter query changed.
    QueryChanged {
        /// Sidebar component identity.
        id: SharedString,
        /// Latest query text.
        query: SharedString,
    },
}

/// User-visible copy for a navigation rail.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct SidebarNavLabels {
    pub workspace: SharedString,
    pub filter: SharedString,
    pub tree: SharedString,
    pub new_task: SharedString,
    pub collapse: SharedString,
    pub expand: SharedString,
    pub no_items: SharedString,
    pub no_matches: SharedString,
    pub badge_prefix: SharedString,
    pub contains_selected: SharedString,
    pub unavailable: SharedString,
}

impl Default for SidebarNavLabels {
    fn default() -> Self {
        Self {
            workspace: "Workspace navigation".into(),
            filter: "Filter navigation".into(),
            tree: "Navigation items".into(),
            new_task: "New task".into(),
            collapse: "Collapse navigation".into(),
            expand: "Expand navigation".into(),
            no_items: "No navigation items".into(),
            no_matches: "No matching navigation items".into(),
            badge_prefix: "Badge".into(),
            contains_selected: "Contains selected item".into(),
            unavailable: "Unavailable".into(),
        }
    }
}

/// How the navigation draws its outer shell.
///
/// The shell is all this changes. Header, rows, filtering, the keyboard model,
/// and every accessibility identity are the same in both modes, so a docked
/// navigation differs from a standalone one by its box alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SidebarNavPresentation {
    /// A standalone rail that owns its own size: a fixed collapsed or expanded
    /// width, no flex growth, and a border along its trailing edge.
    #[default]
    Standalone,
    /// A shell that fills whatever box its host gives it, with no forced
    /// width, no rail sizing, and no edge border.
    ///
    /// For hosts that already own placement and size — a dock panel, a split,
    /// a resizable pane. Placement stays with the host and is deliberately not
    /// described here: a navigation docked along the bottom fills that
    /// region's width exactly as one docked left fills its column, because the
    /// shell contributes no width of its own either way.
    Embedded,
}

/// A hybrid-controlled, filterable application sidebar.
///
/// Applications own the immutable section/item snapshot and active item ID.
/// This entity retains one native [`InputState`] plus collapsed, query,
/// expansion, focus, and scroll interaction state. Selection is emitted as a
/// typed stable ID and never changes the consumer-controlled active item.
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::AppContext;
/// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
/// let nav = cx.new(|cx| SidebarNav::new("workspace-nav", window, cx));
/// nav.update(cx, |nav, cx| {
///     nav.set_sections([
///         SidebarSection::new("main", "Workspace").items([
///             SidebarNavItem::new("orders", "Orders")
///                 .icon(gpui_component::IconName::LayoutDashboard),
///         ]),
///     ], cx);
///     nav.set_active_item("orders", cx);
/// });
/// # }
/// ```
pub struct SidebarNav {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    sections: Arc<[SidebarSection]>,
    active_item: Option<SharedString>,
    collapsed: bool,
    /// One highlight glides between rows instead of per-row hover fills.
    /// Default on; [`Self::set_hover_glide`] restores a local hover fill.
    hover_glide: bool,
    query: SharedString,
    input: Entity<InputState>,
    filter_placeholder: SharedString,
    labels: SidebarNavLabels,
    /// Whether the component draws its own new-task button and filter input.
    /// Hosts with their own rail chrome turn this off and dock the row tree
    /// below their header.
    chrome_visible: bool,
    expanded: HashSet<SharedString>,
    /// Flattened visible rows, recomputed only where state changes.
    rows: Arc<[VisibleRow]>,
    /// Stable ID of the tree's roving row, retained across every projection.
    focused_row: Option<SharedString>,
    /// The tree's single focus handle; rows are active descendants of it.
    tree_focus: FocusHandle,
    /// Virtualized row list and the nav body's sole scroll owner.
    row_list: ListState,
    /// Rem size the retained row heights were measured against.
    resolved_layout: ResolvedLayoutKey,
    /// Rows the virtual list has constructed, which is what bounded
    /// construction is asserted against.
    constructed_rows: Rc<Cell<usize>>,
    /// How the shell is drawn. Read only by `render_shell`.
    presentation: SidebarNavPresentation,
    _input_subscription: Subscription,
}

impl SidebarNav {
    /// Create an empty navigation entity with one retained native filter input.
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Filter navigation"));
        let subscription =
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let query: SharedString = input.read(cx).value().to_string().into();
                    this.update_query(query, cx);
                }
            });
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            sections: Arc::from([]),
            active_item: None,
            collapsed: false,
            hover_glide: true,
            query: "".into(),
            input,
            filter_placeholder: "Filter navigation".into(),
            labels: SidebarNavLabels::default(),
            chrome_visible: true,
            expanded: HashSet::new(),
            rows: Arc::from([]),
            focused_row: None,
            tree_focus: cx.focus_handle(),
            row_list: ListState::new(0, ListAlignment::Top, Pixels::ZERO),
            resolved_layout: ResolvedLayoutKey::default(),
            constructed_rows: Rc::new(Cell::new(0)),
            presentation: SidebarNavPresentation::default(),
            _input_subscription: subscription,
        }
    }

    /// Set how the shell is drawn (default:
    /// [`SidebarNavPresentation::Standalone`]).
    ///
    /// ```no_run
    /// # use gpui_ai::prelude::*;
    /// # use gpui::AppContext;
    /// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
    /// let nav = cx.new(|cx| {
    ///     SidebarNav::new("docked-nav", window, cx)
    ///         .with_presentation(SidebarNavPresentation::Embedded)
    /// });
    /// # }
    /// ```
    pub fn with_presentation(mut self, presentation: SidebarNavPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    /// Replace the controlled section snapshot.
    ///
    /// Malformed snapshots containing duplicate section or item IDs are
    /// ignored so focus, events, and accessibility identities cannot alias.
    pub fn set_sections(
        &mut self,
        sections: impl IntoIterator<Item = SidebarSection>,
        cx: &mut Context<Self>,
    ) {
        let sections: Arc<[SidebarSection]> = sections.into_iter().collect();
        if self.sections.as_ref() == sections.as_ref() || !snapshot_ids_are_unique(&sections) {
            return;
        }

        let mut old_parents = HashSet::new();
        let mut new_parents = HashSet::new();
        for section in self.sections.iter() {
            collect_parent_ids(&section.items, &mut old_parents);
        }
        for section in sections.iter() {
            collect_parent_ids(&section.items, &mut new_parents);
        }
        self.expanded.retain(|id| new_parents.contains(id));
        self.expanded
            .extend(new_parents.difference(&old_parents).cloned());

        self.sections = sections;
        self.rebuild_rows(cx);
    }

    /// Replace the consumer-controlled active item ID.
    pub fn set_active_item(&mut self, item_id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let item_id = item_id.into();
        if self.active_item.as_ref() != Some(&item_id) {
            self.active_item = Some(item_id);
            // Selection recolors rows, so the list keeps its measurements and
            // its scroll position instead of resetting under the pointer.
            self.rebuild_rows(cx);
        }
    }

    /// Replaces the native quick-filter placeholder.
    pub fn set_filter_placeholder(
        &mut self,
        placeholder: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let placeholder = placeholder.into();
        if self.filter_placeholder == placeholder {
            return;
        }
        self.input.update(cx, |input, cx| {
            input.set_placeholder(placeholder.to_string(), window, cx);
        });
        self.filter_placeholder = placeholder;
        cx.notify();
    }

    /// Shows or hides the component's own header controls.
    ///
    /// The row tree, keyboard model and events stay active; only the new-task
    /// button and filter input are removed, for hosts that render their own.
    pub fn set_chrome_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.chrome_visible == visible {
            return;
        }
        self.chrome_visible = visible;
        cx.notify();
    }

    /// Replaces navigation copy and the filter placeholder together.
    pub fn set_labels(
        &mut self,
        labels: SidebarNavLabels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.set_filter_placeholder(labels.filter.clone(), window, cx);
        if self.labels == labels {
            return;
        }
        self.labels = labels;
        cx.notify();
    }

    /// Replace the owned collapsed state and emit its stable component identity.
    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.collapsed == collapsed {
            return;
        }
        self.collapsed = collapsed;
        self.rebuild_rows(cx);
        cx.emit(SidebarNavEvent::CollapsedChanged {
            id: self.id.clone(),
            collapsed,
        });
    }

    /// Replace the native query and emit a change only when its text differs.
    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = query.into();
        if self.query == query {
            return;
        }
        self.query = query.clone();
        self.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx)
        });
        // InputState intentionally suppresses Change for programmatic values,
        // and it may be unmounted while collapsed, so the owner must rebuild
        // its own projection here.
        self.rebuild_rows(cx);
        self.emit_query_changed(query, cx);
    }

    /// Return whether the sidebar is currently collapsed.
    pub fn is_collapsed(&self) -> bool {
        self.collapsed
    }

    /// Choose between the gliding hover highlight (the default) and a
    /// local hover fill per row.
    pub fn set_hover_glide(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.hover_glide != enabled {
            self.hover_glide = enabled;
            cx.notify();
        }
    }

    /// Return the latest quick-filter query.
    pub fn query(&self) -> &SharedString {
        &self.query
    }

    /// Return how the shell is drawn.
    pub fn presentation(&self) -> SidebarNavPresentation {
        self.presentation
    }

    /// Move focus into the retained native filter input when it is mounted.
    ///
    /// Collapsed navigation keeps focus on its visible compact control instead
    /// of moving it into an unmounted input.
    pub fn focus_filter(&self, window: &mut Window, cx: &mut App) {
        if self.collapsed {
            return;
        }
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }

    fn update_query(&mut self, query: SharedString, cx: &mut Context<Self>) {
        if self.query == query {
            return;
        }
        self.query = query.clone();
        self.rebuild_rows(cx);
        self.emit_query_changed(query, cx);
    }

    fn emit_query_changed(&self, query: SharedString, cx: &mut Context<Self>) {
        cx.emit(SidebarNavEvent::QueryChanged {
            id: self.id.clone(),
            query,
        });
    }
}

impl EventEmitter<SidebarNavEvent> for SidebarNav {}

impl gpui::Styled for SidebarNav {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for SidebarNav {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Measured row heights are only valid for the rem they were laid out
        // at. Reading it here mutates nothing; the reaction is deferred so
        // that render never notifies.
        let rem_size = window.rem_size();
        if !self.resolved_layout.matches(rem_size) {
            cx.defer_in(window, move |nav, _, cx| {
                nav.resolve_layout(rem_size, cx);
            });
        }

        let content = self.render_content(window, cx);

        // Collapse restructures the tree — descendants unmount, levels
        // flatten — so the rail cannot cross-fade rows one by one. Instead
        // the standalone shell glides its width between the poles while the
        // incoming structure fades in as the width settles, which keeps
        // icons and focus at their settled positions rather than dragging
        // them through the travel. Reduced motion snaps both.
        let expansion = disclosure_progress(
            (ElementId::from(self.id.clone()), "expanse"),
            !self.collapsed,
            window,
            cx,
        );
        let expansion_fade = crate::motion::disclosure_fade(
            (ElementId::from(self.id.clone()), "expanse"),
            !self.collapsed,
            window,
            cx,
        );
        let settle = if self.collapsed {
            1.0 - expansion_fade
        } else {
            expansion_fade
        };
        self.render_shell(expansion, cx)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .h_full()
                    .min_h_0()
                    .w_full()
                    .opacity(settle)
                    .children(content),
            )
            .refine_style(&self.style)
    }
}

impl SidebarNav {
    /// The outer shell: stable identity, navigation semantics, and the box the
    /// host sees.
    ///
    /// This is the only rendering [`SidebarNavPresentation`] reaches, which is
    /// what keeps the setting additive: the content below draws the same rows
    /// either way.
    fn render_shell(&self, expansion: f32, cx: &mut App) -> Stateful<Div> {
        let tokens = cx.theme().semantic_tokens();
        div()
            .id((ElementId::from(self.id.clone()), "frame"))
            .debug_selector({
                let id = self.id.clone();
                move || format!("sidebar-nav-{id}")
            })
            .accessibility_id(format!("sidebar-nav.{}", self.id))
            .role(Role::Navigation)
            .aria_label(self.labels.workspace.clone())
            .h_full()
            .min_h_0()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(cx.theme().sidebar)
            .text_color(cx.theme().sidebar_foreground)
            .map(|shell| match self.presentation {
                // A standalone rail sizes itself and draws the edge between
                // itself and whatever sits beside it.
                SidebarNavPresentation::Standalone => {
                    let narrow = tokens.spacing.xxl * 1.5;
                    let wide = tokens.spacing.xxl * 8.;
                    shell
                        .w(narrow + (wide - narrow) * expansion)
                        .flex_none()
                        .border_r_1()
                        .border_color(cx.theme().sidebar_border)
                }
                // The host owns placement and size, so the shell contributes
                // neither a width nor an edge: docked along the bottom it
                // fills that region's width instead of keeping a rail's, and
                // the host's own divider is the only border drawn.
                SidebarNavPresentation::Embedded => shell.w_full().min_w_0(),
            })
    }

    /// The shell's children, in order: header, virtualized row tree, and the
    /// empty status.
    ///
    /// Nothing here reads [`SidebarNavPresentation`].
    fn render_content(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let tokens = cx.theme().semantic_tokens();
        // The rows' own accessibility overlay owns the pointer, so hover is
        // the crate's to draw: one highlight gliding between rows, exactly
        // as the thread list and the model picker draw it.
        let glide = self.hover_glide.then(|| {
            crate::glide::glide_hover_state(
                (ElementId::from(self.id.clone()), "row-glide").into(),
                window,
                cx,
            )
        });
        let new_task_owner = cx.weak_entity();
        let collapse_owner = cx.weak_entity();
        let row_owner = cx.weak_entity();
        let key_owner = cx.weak_entity();
        let collapsed = self.collapsed;
        let input = self.input.clone();
        let id = self.id.clone();
        let rows = self.rows.clone();
        let row_component_id = self.id.clone();
        let constructed_rows = self.constructed_rows.clone();
        let roving_row = self.roving_row_id();
        let tree_focused = self.tree_focus.is_focused(window);
        let empty_message: SharedString = if self.sections.is_empty() {
            self.labels.no_items.clone()
        } else {
            self.labels.no_matches.clone()
        };
        let empty_selector = self.sections.is_empty();

        let header_actions = if collapsed {
            h_flex().w_full().child(
                nav_control(
                    (ElementId::from(id.clone()), "collapse"),
                    self.labels.expand.clone(),
                    IconName::PanelLeftOpen,
                    false,
                    cx,
                )
                .w_full()
                .debug_selector(|| "sidebar-nav-collapse".to_owned())
                .on_click(move |_, _, cx| {
                    _ = collapse_owner.update(cx, |nav, cx| nav.set_collapsed(false, cx));
                }),
            )
        } else {
            h_flex()
                .w_full()
                .gap(tokens.spacing.xs)
                .child(
                    nav_control(
                        (ElementId::from(id.clone()), "new-task"),
                        self.labels.new_task.clone(),
                        IconName::Plus,
                        true,
                        cx,
                    )
                    .debug_selector(|| "sidebar-nav-new-task".to_owned())
                    .on_click(move |_, _, cx| {
                        _ = new_task_owner.update(cx, |nav, cx| {
                            cx.emit(SidebarNavEvent::NewTaskRequested { id: nav.id.clone() });
                        });
                    }),
                )
                .child(
                    nav_control(
                        (ElementId::from(id.clone()), "collapse"),
                        self.labels.collapse.clone(),
                        IconName::PanelLeftClose,
                        false,
                        cx,
                    )
                    .debug_selector(|| "sidebar-nav-collapse".to_owned())
                    .on_click(move |_, _, cx| {
                        _ = collapse_owner.update(cx, |nav, cx| nav.set_collapsed(true, cx));
                    }),
                )
        };

        let header = v_flex()
            .w_full()
            .gap(tokens.spacing.sm)
            .when(!self.chrome_visible, |this| this.flex_none())
            .when(self.chrome_visible, |this| this.child(header_actions))
            .when(self.chrome_visible && !collapsed, |this| {
                this.child(
                    div()
                        .id((ElementId::from(id.clone()), "filter"))
                        .debug_selector(|| "sidebar-nav-filter".to_owned())
                        .w_full()
                        .child(
                            Input::new(&input)
                                .accessibility_id(format!("sidebar-nav.{id}.filter"))
                                .aria_label(self.labels.filter.clone())
                                .prefix(IconName::Search)
                                .cleanable(true)
                                .w_full(),
                        ),
                )
            });
        let row_list = self.row_list.clone();
        let row_glide = glide.clone();
        let row_labels = self.labels.clone();
        let rows_are_empty = self.rows.is_empty();

        let mut content = vec![
            div()
                .w_full()
                .flex_none()
                .p(tokens.spacing.sm)
                .child(header)
                .into_any_element(),
            {
                let frame = div().relative().w_full().flex_1().min_h_0();
                match &glide {
                    Some(state) => crate::glide::glide_frame(frame, state).child(
                        crate::glide::glide_highlight(
                            (ElementId::from(self.id.clone()), "row-glide").into(),
                            state,
                            cx.theme().radius,
                            "sidebar-nav-glide",
                            window,
                            cx,
                        ),
                    ),
                    None => frame,
                }
            }
            .child(
                sidebar_tree_container(&self.id, self.labels.tree.clone())
                    // The tree is the single tab stop and holds focus
                    // for every row, so a row that unmounts while
                    // scrolled away cannot take the reader's focus
                    // out of the navigation with it.
                    .track_focus(&self.tree_focus.clone().tab_index(0).tab_stop(true))
                    .size_full()
                    .flex()
                    .flex_col()
                    .px(tokens.spacing.sm)
                    .gap(tokens.spacing.xxs)
                    .child(
                        list(row_list.clone(), move |index, window, cx| {
                            constructed_rows.set(constructed_rows.get() + 1);
                            rows.get(index)
                                .map(|row| {
                                    render_row(
                                        row,
                                        &render::RowContext {
                                            component_id: &row_component_id,
                                            collapsed,
                                            focused: tree_focused
                                                && roving_row.as_ref() == Some(&row.id),
                                            owner: &row_owner,
                                            glide: row_glide.as_ref(),
                                            labels: &row_labels,
                                        },
                                        window,
                                        cx,
                                    )
                                })
                                .unwrap_or_else(|| div().hidden().into_any_element())
                        })
                        .size_full(),
                    )
                    .policy_vertical_scrollbar(&row_list, cx)
                    .on_key_down(move |event, window, cx| {
                        if event.keystroke.modifiers.modified() {
                            return;
                        }
                        let key = event.keystroke.key.clone();
                        let handled = key_owner
                            .update(cx, |nav, cx| nav.navigate(&key, window, cx))
                            .unwrap_or(false);
                        if handled {
                            cx.stop_propagation();
                        }
                    }),
            )
            // Capture-phase containment wins over an ancestor catalog
            // list and releases the wheel at either nav edge.
            .child(list_scroll_mask(&self.row_list))
            .into_any_element(),
        ];
        if rows_are_empty && !collapsed {
            content.push(
                div()
                    .id((ElementId::from(self.id.clone()), "empty-status"))
                    .debug_selector(move || {
                        if empty_selector {
                            "sidebar-nav-empty".to_owned()
                        } else {
                            "sidebar-nav-no-results".to_owned()
                        }
                    })
                    .flex_none()
                    .role(Role::Status)
                    .aria_label(empty_message.clone())
                    .child(crate::surface::empty_state(
                        IconName::Search,
                        empty_message,
                        None,
                        cx,
                    ))
                    .into_any_element(),
            );
        }
        content
    }
}
