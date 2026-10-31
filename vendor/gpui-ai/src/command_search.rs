//! Stable-ID command search composed over gpui-component's native command palette.

use gpui_base::StyledExt as _;
use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, ElementExt as _, IconName, IndexPath,
    command::{Command, CommandItem, CommandState},
    h_flex,
    label::Label,
    v_flex,
};

use crate::theme::SemanticStyledExt as _;

/// One application-owned command-search item.
///
/// IDs must be stable and unique inside a snapshot. Labels may be duplicated:
/// selection and emitted events always use `id`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSearchItem {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    keywords: Arc<[SharedString]>,
    shortcut: Option<SharedString>,
    disabled: bool,
}

/// User-visible copy for the command search.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct CommandSearchLabels {
    pub container: SharedString,
    pub placeholder: SharedString,
    pub empty: SharedString,
    pub no_results: SharedString,
}

impl Default for CommandSearchLabels {
    fn default() -> Self {
        Self {
            container: "Command search".into(),
            placeholder: "Search commands".into(),
            empty: "No commands available".into(),
            no_results: "No matching commands".into(),
        }
    }
}

impl CommandSearchItem {
    /// Create an enabled item with stable identity and a visible title.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            keywords: Arc::from([]),
            shortcut: None,
            disabled: false,
        }
    }

    /// Set secondary descriptive text, which also participates in filtering.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Replace the extra case-insensitive search terms.
    pub fn keywords<I, S>(mut self, keywords: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<SharedString>,
    {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    /// Set the visible shortcut hint.
    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    /// Set whether the item is visible but unavailable to every activation path.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Return the stable application identity.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Return the visible title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Return the optional secondary description.
    pub fn subtitle_text(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }

    /// Return the extra search terms.
    pub fn keyword_terms(&self) -> &[SharedString] {
        &self.keywords
    }

    /// Return the optional shortcut hint.
    pub fn shortcut_text(&self) -> Option<&SharedString> {
        self.shortcut.as_ref()
    }

    /// Return whether activation is disabled.
    pub fn is_disabled(&self) -> bool {
        self.disabled
    }

    fn accessibility_label(&self) -> SharedString {
        match (&self.subtitle, &self.shortcut) {
            (Some(subtitle), Some(shortcut)) => {
                format!("{}, {}, shortcut {}", self.title, subtitle, shortcut).into()
            }
            (Some(subtitle), None) => format!("{}, {}", self.title, subtitle).into(),
            (None, Some(shortcut)) => format!("{}, shortcut {}", self.title, shortcut).into(),
            (None, None) => self.title.clone(),
        }
    }
}

/// A typed application intent emitted by [`CommandSearch`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommandSearchEvent {
    /// The native command query changed.
    QueryChanged {
        /// Command-search component identity.
        id: SharedString,
        /// Latest query text.
        query: SharedString,
    },
    /// An enabled item was confirmed by pointer, keyboard, or accessibility.
    Selected {
        /// Command-search component identity.
        id: SharedString,
        /// Stable application item identity.
        item_id: SharedString,
    },
    /// Escape requested that the surrounding surface dismiss the search.
    Dismissed {
        /// Command-search component identity.
        id: SharedString,
    },
}

fn retained_selection(
    previous: Option<&SharedString>,
    items: &[CommandSearchItem],
) -> Option<SharedString> {
    previous
        .and_then(|id| items.iter().find(|item| item.id == *id && !item.disabled))
        .or_else(|| items.iter().find(|item| !item.disabled))
        .map(|item| item.id.clone())
}

fn command_search_frame(
    id: &SharedString,
    label: impl Into<SharedString>,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id.clone())
        .accessibility_id(format!("command-search.{id}"))
        .debug_selector({
            let id = id.clone();
            move || format!("command-search-{id}")
        })
        .role(Role::Search)
        .aria_label(label)
        .w_full()
        .min_w_0()
        .overflow_hidden()
}

fn row_content(
    search_id: SharedString,
    item: CommandSearchItem,
    cx: &mut App,
) -> gpui::Stateful<gpui::Div> {
    let tokens = cx.theme().semantic_tokens();
    let accessibility_id = format!("command-search.{search_id}.item.{}", item.id);
    let debug_item_id = item.id.clone();
    let content_id = ElementId::from((
        ElementId::from((ElementId::from(search_id), item.id.clone())),
        "content",
    ));
    let label = item.accessibility_label();

    h_flex()
        .id(content_id)
        .accessibility_id(accessibility_id)
        .debug_selector(move || format!("command-search-item-{debug_item_id}"))
        // Pinned Command supplies the sole option node and activation path.
        .role(Role::Label)
        .aria_label(label)
        .w_full()
        .min_w_0()
        .gap(tokens.spacing.sm)
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(tokens.spacing.xxs)
                .child(Label::new(item.title.clone()).text_token(tokens.typography.sm))
                .when_some(item.subtitle.clone(), |this, subtitle| {
                    this.child(
                        Label::new(subtitle)
                            .text_token(tokens.typography.xs)
                            .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        .when_some(item.shortcut.clone(), |this, shortcut| {
            this.child(
                div()
                    .flex_none()
                    .px(tokens.spacing.xs)
                    .py(tokens.spacing.xxs)
                    .rounded(tokens.radius.sm)
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted.opacity(0.4))
                    .text_token(tokens.typography.xs)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().muted_foreground)
                    .child(Label::new(shortcut)),
            )
        })
}

fn upstream_item(search_id: &SharedString, item: &CommandSearchItem) -> CommandItem {
    let row_item = item.clone();
    let row_search_id = search_id.clone();
    let mut search_terms =
        Vec::with_capacity(item.keywords.len() + usize::from(item.subtitle.is_some()));
    if let Some(subtitle) = &item.subtitle {
        search_terms.push(subtitle.clone());
    }
    search_terms.extend(item.keywords.iter().cloned());

    CommandItem::new()
        .label(item.title.clone())
        .keywords(search_terms)
        .child(move |_, cx| row_content(row_search_id.clone(), row_item.clone(), cx))
        .disabled(item.disabled)
}

/// A controlled stable-ID adapter around gpui-component's native command palette.
///
/// The application replaces immutable item snapshots and handles typed events.
/// This entity owns one upstream [`CommandState`], which remains the sole owner
/// of native text editing, focus, filtering, navigation, and virtual-list state.
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::AppContext;
/// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
/// let search = cx.new(|cx| CommandSearch::new("workspace-search", window, cx));
/// search.update(cx, |search, cx| {
///     search.set_items(
///         [CommandSearchItem::new("calendar", "Open calendar")
///             .shortcut("Ctrl+K")],
///         window,
///         cx,
///     );
/// });
/// # }
/// ```
pub struct CommandSearch {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    items: Arc<[CommandSearchItem]>,
    upstream_items: Arc<[CommandItem]>,
    state: Entity<CommandState>,
    selected_id: Option<SharedString>,
    last_emitted_query: SharedString,
    items_revision: u64,
    applied_selection_revision: u64,
    labels: CommandSearchLabels,
}

impl CommandSearch {
    /// Create an empty command search with native input and focus state.
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            items: Arc::from([]),
            upstream_items: Arc::from([]),
            state: cx.new(|cx| CommandState::new(window, cx)),
            selected_id: None,
            last_emitted_query: "".into(),
            items_revision: 0,
            applied_selection_revision: 0,
            labels: CommandSearchLabels::default(),
        }
    }

    /// Replaces user-visible copy for this command search.
    pub fn set_labels(&mut self, labels: CommandSearchLabels, cx: &mut Context<Self>) {
        if self.labels == labels {
            return;
        }
        self.labels = labels;
        cx.notify();
    }

    /// Replace the controlled item snapshot and retain selection by stable ID.
    pub fn set_items(
        &mut self,
        items: impl IntoIterator<Item = CommandSearchItem>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let items: Arc<[CommandSearchItem]> = items.into_iter().collect();
        if self.items == items {
            return;
        }

        self.selected_id = retained_selection(self.selected_id.as_ref(), &items);
        self.upstream_items = items
            .iter()
            .map(|item| upstream_item(&self.id, item))
            .collect();
        self.items = items;
        self.items_revision = self.items_revision.wrapping_add(1);
        cx.notify();
    }

    /// Replace the native query and emit [`CommandSearchEvent::QueryChanged`]
    /// when it actually changes.
    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = query.into();
        let changed = self.query(cx) != query;
        self.state
            .update(cx, |state, cx| state.set_query(query.clone(), window, cx));
        if changed {
            self.emit_query_changed(query, cx);
        }
    }

    /// Return the native input's current query.
    pub fn query(&self, cx: &App) -> SharedString {
        self.state.read(cx).query(cx)
    }

    /// Move focus to the native command input.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    fn item_id_at(items: &[CommandSearchItem], index: IndexPath) -> Option<SharedString> {
        (index.section == 0)
            .then(|| items.get(index.row))
            .flatten()
            .map(|item| item.id.clone())
    }

    fn index_for_id(&self, id: &SharedString) -> Option<IndexPath> {
        self.items
            .iter()
            .position(|item| item.id == *id && !item.disabled)
            .map(IndexPath::new)
    }

    fn current_item_id_from_installed(
        &self,
        installed_revision: u64,
        installed_items: &[CommandSearchItem],
        index: IndexPath,
    ) -> Option<SharedString> {
        if installed_revision > self.items_revision {
            return None;
        }
        let item_id = Self::item_id_at(installed_items, index)?;
        self.items
            .iter()
            .find(|item| item.id == item_id && !item.disabled)
            .map(|item| item.id.clone())
    }

    fn on_select(
        &mut self,
        installed_revision: u64,
        installed_items: &[CommandSearchItem],
        index: IndexPath,
    ) {
        if let Some(item_id) =
            self.current_item_id_from_installed(installed_revision, installed_items, index)
        {
            self.selected_id = Some(item_id);
        }
    }

    fn on_confirm(
        &mut self,
        installed_revision: u64,
        installed_items: &[CommandSearchItem],
        index: IndexPath,
        cx: &mut Context<Self>,
    ) {
        let Some(item_id) =
            self.current_item_id_from_installed(installed_revision, installed_items, index)
        else {
            return;
        };
        self.confirm_id(item_id, cx);
    }

    fn confirm_id(&mut self, item_id: SharedString, cx: &mut Context<Self>) {
        let Some(item) = self
            .items
            .iter()
            .find(|item| item.id == item_id && !item.disabled)
        else {
            return;
        };

        self.selected_id = Some(item.id.clone());
        cx.emit(CommandSearchEvent::Selected {
            id: self.id.clone(),
            item_id: item.id.clone(),
        });
    }

    fn on_query(&mut self, query: &str, cx: &mut Context<Self>) {
        if self.query(cx).as_ref() != query {
            return;
        }
        self.emit_query_changed(query.to_owned().into(), cx);
    }

    fn emit_query_changed(&mut self, query: SharedString, cx: &mut Context<Self>) {
        if self.last_emitted_query == query {
            return;
        }
        self.last_emitted_query = query.clone();
        cx.emit(CommandSearchEvent::QueryChanged {
            id: self.id.clone(),
            query,
        });
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        cx.emit(CommandSearchEvent::Dismissed {
            id: self.id.clone(),
        });
    }
}

impl EventEmitter<CommandSearchEvent> for CommandSearch {}

impl Focusable for CommandSearch {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.state.focus_handle(cx)
    }
}

impl gpui::Styled for CommandSearch {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for CommandSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id.clone();
        let labels = self.labels.clone();
        let tokens = cx.theme().semantic_tokens();
        let query_owner = cx.weak_entity();
        let select_owner = cx.weak_entity();
        let confirm_owner = cx.weak_entity();
        let cancel_owner = cx.weak_entity();
        let selection_owner = cx.weak_entity();
        let revision = self.items_revision;
        let installed_items = self.items.clone();
        let select_items = installed_items.clone();
        let confirm_items = installed_items;
        let should_restore_selection = self.applied_selection_revision != revision;
        let command = Command::new(&self.state)
            .items(self.upstream_items.iter().cloned())
            .placeholder(labels.placeholder.clone())
            .max_h(tokens.spacing.xxl * 10.)
            .empty({
                let id = id.clone();
                move |state, _, cx| {
                    let (message, selector): (SharedString, &'static str) =
                        if state.query(cx).is_empty() {
                            (
                                labels.empty.clone(),
                                "command-search-empty-catalog",
                            )
                        } else {
                            (labels.no_results.clone(), "command-search-no-results")
                        };
                    div()
                        .id(ElementId::from((ElementId::from(id.clone()), "empty")))
                        .debug_selector(move || selector.to_owned())
                        .role(Role::Status)
                        .aria_label(message.clone())
                        .child(crate::surface::empty_state(
                            IconName::Search,
                            message,
                            None,
                            cx,
                        ))
                }
            })
            .on_query(move |query, _, cx| {
                _ = query_owner.update(cx, |search, cx| search.on_query(query, cx));
            })
            .on_select(move |index, _, cx| {
                _ = select_owner.update(cx, |search, _| {
                    search.on_select(revision, &select_items, index);
                });
            })
            .on_confirm(move |index, _, cx| {
                _ = confirm_owner.update(cx, |search, cx| {
                    search.on_confirm(revision, &confirm_items, index, cx);
                });
            })
            .on_cancel(move |_, cx| {
                _ = cancel_owner.update(cx, |search, cx| search.dismiss(cx));
            });

        command_search_frame(&self.id, labels.container.clone())
            .on_prepaint(move |_, window, cx| {
                if !should_restore_selection {
                    return;
                }
                _ = selection_owner.update(cx, |search, cx| {
                    if search.items_revision != revision
                        || search.applied_selection_revision == revision
                    {
                        return;
                    }
                    search.applied_selection_revision = revision;
                    let selected_index = search
                        .selected_id
                        .as_ref()
                        .and_then(|id| search.index_for_id(id));
                    search.state.update(cx, |state, cx| {
                        let current_index = state.selected_index();
                        state.set_selected_index(selected_index, window, cx);
                        if selected_index.is_some()
                            && state.selected_index().is_none()
                            && current_index.is_some()
                        {
                            state.set_selected_index(current_index, window, cx);
                        }
                    });
                });
            })
            .child(command)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::{Arc, Mutex},
    };

    use gpui::{
        AnyElement, App, AppContext as _, Bounds, Context, Element, Entity, GlobalElementId,
        InspectorElementId, IntoElement, KeyDownEvent, KeyUpEvent, Keystroke, LayoutId, Modifiers,
        Pixels, Render, Role, Subscription, TestAppContext, VisualTestContext, Window, accesskit,
        canvas, point, px, size,
    };

    use super::{
        CommandSearch, CommandSearchEvent, CommandSearchItem, command_search_frame,
        retained_selection, row_content,
    };

    #[test]
    fn selection_is_retained_by_id_after_replacement_and_reorder() {
        let replacement = vec![
            CommandSearchItem::new("third", "Third"),
            CommandSearchItem::new("selected", "Selected"),
            CommandSearchItem::new("first", "First"),
        ];

        assert_eq!(
            retained_selection(Some(&"selected".into()), &replacement),
            Some("selected".into())
        );
        assert_eq!(
            retained_selection(Some(&"removed".into()), &replacement),
            Some("third".into())
        );
    }

    #[test]
    fn disabled_items_are_skipped_when_choosing_a_replacement_selection() {
        let replacement = vec![
            CommandSearchItem::new("disabled", "Disabled").disabled(true),
            CommandSearchItem::new("enabled", "Enabled"),
        ];

        assert_eq!(
            retained_selection(None, &replacement),
            Some("enabled".into())
        );
    }

    struct CapturedRow {
        role: Option<Role>,
        node: accesskit::Node,
    }

    struct RowA11yProbe {
        disabled: bool,
        captured: Arc<Mutex<Option<CapturedRow>>>,
    }

    impl Render for RowA11yProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            let disabled = self.disabled;
            let captured = self.captured.clone();
            canvas(
                move |_, _, cx| {
                    let row = row_content(
                        "probe".into(),
                        CommandSearchItem::new("report", "Open report")
                            .subtitle("Supplier pricing")
                            .shortcut("Ctrl+R")
                            .disabled(disabled),
                        cx,
                    );
                    let role = row.a11y_role();
                    let mut node = accesskit::Node::new(Role::Unknown);
                    row.write_a11y_info(&mut node);
                    *captured.lock().expect("row capture should be available") =
                        Some(CapturedRow { role, node });
                },
                |_, _, _, _| {},
            )
        }
    }

    fn capture_row(disabled: bool, cx: &mut TestAppContext) -> CapturedRow {
        cx.update(crate::init);
        let captured = Arc::new(Mutex::new(None));
        let result = captured.clone();
        let (_, cx) = cx.add_window_view(move |_, _| RowA11yProbe { disabled, captured });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        result
            .lock()
            .expect("row capture should be available")
            .take()
            .expect("row should be captured")
    }

    #[gpui::test]
    fn custom_rows_are_named_noninteractive_labels(cx: &mut TestAppContext) {
        let enabled = capture_row(false, cx);
        assert_eq!(enabled.role, Some(Role::Label));
        assert_eq!(
            enabled.node.label(),
            Some("Open report, Supplier pricing, shortcut Ctrl+R")
        );
        assert!(!enabled.node.supports_action(accesskit::Action::Click));

        let disabled = capture_row(true, cx);
        assert_eq!(disabled.role, Some(Role::Label));
        assert_eq!(
            disabled.node.label(),
            Some("Open report, Supplier pricing, shortcut Ctrl+R")
        );
        assert!(!disabled.node.supports_action(accesskit::Action::Click));
    }

    #[test]
    fn production_frame_exposes_a_named_search_landmark() {
        let frame =
            command_search_frame(&"palette".into(), "Command search").into_element();
        let mut node = accesskit::Node::new(Role::Unknown);
        frame.write_a11y_info(&mut node);

        assert_eq!(frame.a11y_role(), Some(Role::Search));
        assert_eq!(node.label(), Some("Command search"));
    }

    struct Harness {
        search: Entity<CommandSearch>,
        events: Rc<RefCell<Vec<CommandSearchEvent>>>,
        _subscription: Subscription,
    }

    struct PreRenderQueryHarness {
        search: Entity<CommandSearch>,
        events: Rc<RefCell<Vec<CommandSearchEvent>>>,
        _subscription: Subscription,
    }

    impl PreRenderQueryHarness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let search = cx.new(|cx| CommandSearch::new("pre-render-query", window, cx));
            let events = Rc::new(RefCell::new(Vec::new()));
            let captured = events.clone();
            let subscription = cx.subscribe(&search, move |_, _, event, _| {
                captured.borrow_mut().push(event.clone());
            });
            search.update(cx, |search, cx| {
                search.set_query("before render", window, cx);
            });
            Self {
                search,
                events,
                _subscription: subscription,
            }
        }
    }

    impl Render for PreRenderQueryHarness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            self.search.clone()
        }
    }

    impl Harness {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let search = cx.new(|cx| CommandSearch::new("test-search", window, cx));
            let events = Rc::new(RefCell::new(Vec::new()));
            let captured = events.clone();
            let subscription = cx.subscribe(&search, move |_, _, event, _| {
                captured.borrow_mut().push(event.clone());
            });
            Self {
                search,
                events,
                _subscription: subscription,
            }
        }
    }

    impl Render for Harness {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
            self.search.clone()
        }
    }

    struct ReplaceBeforeChildPrepaint {
        search: Entity<CommandSearch>,
        replacement: Option<Vec<CommandSearchItem>>,
        observed_applied_revision: Rc<Cell<u64>>,
        child: AnyElement,
    }

    impl IntoElement for ReplaceBeforeChildPrepaint {
        type Element = Self;

        fn into_element(self) -> Self::Element {
            self
        }
    }

    impl Element for ReplaceBeforeChildPrepaint {
        type RequestLayoutState = ();
        type PrepaintState = ();

        fn id(&self) -> Option<gpui::ElementId> {
            None
        }

        fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
            None
        }

        fn request_layout(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            window: &mut Window,
            cx: &mut App,
        ) -> (LayoutId, Self::RequestLayoutState) {
            (self.child.request_layout(window, cx), ())
        }

        fn prepaint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut Self::RequestLayoutState,
            window: &mut Window,
            cx: &mut App,
        ) -> Self::PrepaintState {
            let replacement = self
                .replacement
                .take()
                .expect("the catalog should be replaced exactly once before child prepaint");
            self.search.update(cx, |search, cx| {
                search.set_items(replacement, window, cx);
            });
            self.child.prepaint(window, cx);
            self.observed_applied_revision
                .set(self.search.read(cx).applied_selection_revision);
        }

        fn paint(
            &mut self,
            _: Option<&GlobalElementId>,
            _: Option<&InspectorElementId>,
            _: Bounds<Pixels>,
            _: &mut Self::RequestLayoutState,
            _: &mut Self::PrepaintState,
            window: &mut Window,
            cx: &mut App,
        ) {
            self.child.paint(window, cx);
        }
    }

    fn interactive_catalog() -> Vec<CommandSearchItem> {
        vec![
            CommandSearchItem::new("first", "Open report").shortcut("Ctrl+1"),
            CommandSearchItem::new("disabled", "Unavailable report").disabled(true),
            CommandSearchItem::new("second", "Open report")
                .subtitle("Supplier pricing")
                .keywords(["margin"])
                .shortcut("Ctrl+2"),
        ]
    }

    fn harness(cx: &mut TestAppContext) -> (Entity<Harness>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (harness, cx) = cx.add_window_view(Harness::new);
        let cx: &mut VisualTestContext = cx;
        cx.simulate_resize(size(px(520.), px(360.)));
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.set_items(interactive_catalog(), window, cx);
                #[cfg(not(target_os = "macos"))]
                search.focus(window, cx);
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (harness, cx)
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
    fn set_query_emits_once_before_the_first_render(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (harness, cx) = cx.add_window_view(PreRenderQueryHarness::new);

        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [CommandSearchEvent::QueryChanged {
                id: "pre-render-query".into(),
                query: "before render".into(),
            }]
        );

        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [CommandSearchEvent::QueryChanged {
                id: "pre-render-query".into(),
                query: "before render".into(),
            }]
        );
    }

    #[gpui::test]
    fn upstream_filter_matches_title_subtitle_and_keywords_case_insensitively(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.set_items(
                    [
                        CommandSearchItem::new("calendar", "Open Calendar")
                            .subtitle("Review supplier delivery dates")
                            .keywords(["schedule", "dates"]),
                        CommandSearchItem::new("pricing", "Open report")
                            .subtitle("Supplier pricing")
                            .keywords(["margin", "cost"]),
                        CommandSearchItem::new("archive", "Archive workspace")
                            .keywords(["close", "store"]),
                    ],
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();

        for (query, expected_id) in [
            ("CALENDAR", "calendar"),
            ("DELIVERY", "calendar"),
            ("MARGIN", "pricing"),
        ] {
            cx.update(|window, cx| {
                search.update(cx, |search, cx| search.set_query(query, window, cx));
            });
            cx.run_until_parked();
            search.read_with(cx, |search, cx| {
                assert_eq!(search.state.read(cx).matched_count(), 1, "query {query}");
                assert_eq!(
                    search.selected_id.as_deref(),
                    Some(expected_id),
                    "query {query}"
                );
            });
        }
    }

    #[gpui::test]
    fn programmatic_query_emits_the_latest_stable_component_id(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());

        cx.update(|window, cx| {
            search.update(cx, |search, cx| search.set_query("MARGIN", window, cx));
        });
        cx.run_until_parked();

        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [CommandSearchEvent::QueryChanged {
                id: "test-search".into(),
                query: "MARGIN".into(),
            }]
        );
    }

    #[gpui::test]
    fn empty_catalog_and_no_results_render_distinct_named_statuses(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());

        cx.update(|window, cx| {
            search.update(cx, |search, cx| search.set_items([], window, cx));
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("command-search-empty-catalog").is_some());
        assert!(cx.debug_bounds("command-search-no-results").is_none());

        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.set_items(
                    [CommandSearchItem::new("present", "Present command")],
                    window,
                    cx,
                );
                search.set_query("absent", window, cx);
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("command-search-empty-catalog").is_none());
        assert!(cx.debug_bounds("command-search-no-results").is_some());
    }

    #[cfg_attr(
        target_os = "macos",
        ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
    )]
    #[gpui::test]
    fn keyboard_navigation_skips_disabled_and_confirms_duplicate_label_by_id(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = harness(cx);

        activate_key(cx, "down");
        activate_key(cx, "enter");

        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [CommandSearchEvent::Selected {
                id: "test-search".into(),
                item_id: "second".into(),
            }]
        );
    }

    #[gpui::test]
    fn pointer_and_keyboard_activation_emit_the_same_stable_event(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let first = cx
            .debug_bounds("command-search-item-first")
            .expect("the first visible row should expose a stable test selector");

        cx.simulate_click(first.center(), Modifiers::default());
        cx.run_until_parked();
        activate_key(cx, "enter");

        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [
                CommandSearchEvent::Selected {
                    id: "test-search".into(),
                    item_id: "first".into(),
                },
                CommandSearchEvent::Selected {
                    id: "test-search".into(),
                    item_id: "first".into(),
                },
            ]
        );
    }

    #[cfg_attr(
        target_os = "macos",
        ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
    )]
    #[gpui::test]
    fn deferred_select_and_confirm_keep_the_installed_snapshot_identity(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        cx.update(|window, cx| {
            window.dispatch_keystroke(Keystroke::parse("down").expect("test key should parse"), cx);
            window.dispatch_keystroke(
                Keystroke::parse("enter").expect("test key should parse"),
                cx,
            );
            search.update(cx, |search, cx| {
                search.set_items(
                    [
                        CommandSearchItem::new("second", "Open report"),
                        CommandSearchItem::new("first", "Open report"),
                        CommandSearchItem::new("replacement", "Replacement command"),
                    ],
                    window,
                    cx,
                );
            });
        });
        cx.run_until_parked();

        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [CommandSearchEvent::Selected {
                id: "test-search".into(),
                item_id: "second".into(),
            }]
        );
        assert_eq!(
            search.read_with(cx, |search, _| search.selected_id.clone()),
            Some("second".into())
        );
    }

    #[cfg_attr(
        target_os = "macos",
        ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
    )]
    #[gpui::test]
    fn escape_clears_a_query_before_emitting_dismissal(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        cx.update(|window, cx| {
            search.update(cx, |search, cx| search.set_query("report", window, cx));
        });
        cx.run_until_parked();

        activate_key(cx, "escape");
        activate_key(cx, "escape");

        assert_eq!(
            harness.read_with(cx, |harness, _| harness.events.borrow().clone()),
            [
                CommandSearchEvent::QueryChanged {
                    id: "test-search".into(),
                    query: "report".into(),
                },
                CommandSearchEvent::QueryChanged {
                    id: "test-search".into(),
                    query: "".into(),
                },
                CommandSearchEvent::Dismissed {
                    id: "test-search".into(),
                },
            ]
        );
    }

    #[cfg_attr(
        target_os = "macos",
        ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
    )]
    #[gpui::test]
    fn selected_id_survives_snapshot_reorder_and_moves_the_upstream_highlight(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = harness(cx);
        activate_key(cx, "down");
        let search = harness.read_with(cx, |harness, _| harness.search.clone());

        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.set_items(
                    [
                        CommandSearchItem::new("second", "Open report"),
                        CommandSearchItem::new("first", "Open report"),
                    ],
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        search.read_with(cx, |search, cx| {
            assert_eq!(search.selected_id, Some("second".into()));
            assert_eq!(
                search.state.read(cx).selected_index(),
                Some(gpui_component::IndexPath::new(0))
            );
        });
    }

    #[cfg_attr(
        target_os = "macos",
        ignore = "pinned GPUI TestWindow has no native macOS handle for focused InputState"
    )]
    #[gpui::test]
    fn stale_prepaint_does_not_apply_new_indices_to_an_old_command_model(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        activate_key(cx, "down");
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        let applied_before = search.read_with(cx, |search, _| search.applied_selection_revision);
        let observed_applied_revision = Rc::new(Cell::new(u64::MAX));

        cx.draw(
            point(px(0.), px(0.)),
            size(px(520.), px(360.)),
            |window, cx| {
                search.update(cx, |search, cx| {
                    search.set_items(
                        [
                            CommandSearchItem::new("first", "First command"),
                            CommandSearchItem::new("second", "Second command"),
                        ],
                        window,
                        cx,
                    );
                });
                ReplaceBeforeChildPrepaint {
                    search: search.clone(),
                    replacement: Some(vec![
                        CommandSearchItem::new("second", "Second command"),
                        CommandSearchItem::new("first", "First command"),
                    ]),
                    observed_applied_revision: observed_applied_revision.clone(),
                    child: search.clone().into_any_element(),
                }
            },
        );
        assert_eq!(
            observed_applied_revision.get(),
            applied_before,
            "a stale prepaint must not mark its model revision as applied"
        );
        cx.run_until_parked();

        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        search.read_with(cx, |search, cx| {
            assert_eq!(search.selected_id, Some("second".into()));
            assert_eq!(
                search.state.read(cx).selected_index(),
                Some(gpui_component::IndexPath::new(0))
            );
        });
    }

    #[gpui::test]
    fn pre_first_render_query_keeps_the_upstream_first_match_selected(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (harness, cx) = cx.add_window_view(Harness::new);
        let cx: &mut VisualTestContext = cx;
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.set_items(
                    [
                        CommandSearchItem::new("hidden", "Hidden command"),
                        CommandSearchItem::new("match", "Matching command"),
                    ],
                    window,
                    cx,
                );
                search.set_query("matching", window, cx);
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();

        search.read_with(cx, |search, cx| {
            assert_eq!(search.selected_id, Some("match".into()));
            assert_eq!(
                search.state.read(cx).selected_index(),
                Some(gpui_component::IndexPath::new(1))
            );
        });
    }

    #[gpui::test]
    fn replacement_under_query_preserves_a_valid_upstream_match(cx: &mut TestAppContext) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        cx.update(|window, cx| {
            search.update(cx, |search, cx| search.set_query("margin", window, cx));
        });
        cx.run_until_parked();
        assert_eq!(
            search.read_with(cx, |search, _| search.selected_id.clone()),
            Some("second".into())
        );

        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.set_items(
                    [
                        CommandSearchItem::new("second", "No longer matches"),
                        CommandSearchItem::new("next", "Margin command"),
                    ],
                    window,
                    cx,
                );
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();

        search.read_with(cx, |search, cx| {
            assert_eq!(search.selected_id, Some("next".into()));
            assert_eq!(
                search.state.read(cx).selected_index(),
                Some(gpui_component::IndexPath::new(1))
            );
        });
    }

    #[gpui::test]
    fn thousand_item_palette_paints_only_a_bounded_visible_range_and_reaches_the_end(
        cx: &mut TestAppContext,
    ) {
        let (harness, cx) = harness(cx);
        let search = harness.read_with(cx, |harness, _| harness.search.clone());
        let items = (0..1_000)
            .map(|index| {
                CommandSearchItem::new(format!("large-{index}"), format!("Large command {index}"))
            })
            .collect::<Vec<_>>();
        cx.update(|window, cx| {
            search.update(cx, |search, cx| search.set_items(items, window, cx));
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let visible = (0..1_000)
            .filter(|index| {
                let selector: &'static str =
                    Box::leak(format!("command-search-item-large-{index}").into_boxed_str());
                cx.debug_bounds(selector).is_some()
            })
            .count();
        assert!(
            visible < 50,
            "only a small visible range should be painted, got {visible}"
        );
        assert!(cx.debug_bounds("command-search-item-large-999").is_none());

        cx.update(|window, cx| {
            search.update(cx, |search, cx| {
                search.state.update(cx, |state, cx| {
                    state.set_selected_index(Some(gpui_component::IndexPath::new(999)), window, cx);
                });
            });
            window.draw(cx).clear(cx);
        });
        cx.run_until_parked();
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds("command-search-item-large-999").is_some());
        assert!(cx.debug_bounds("command-search-item-large-0").is_none());
    }
}
