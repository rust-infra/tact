//! Conversation list: new, switch, rename, archive, delete, and search.
//!
//! The application owns the threads (grouped into sections such as Today /
//! Yesterday / Earlier) and the active thread ID; the list owns only its
//! native search input, the "show archived" toggle, which row holds keyboard
//! focus, and the flattened snapshot it virtualizes. Every intent — selecting,
//! creating, renaming, archiving, deleting — is a typed [`ThreadListEvent`]
//! keyed by stable ID.
//!
//! Two constraints shape the rendering. Construction is bounded by the visible
//! window: section headers and surviving threads are flattened into
//! `ThreadRow`s once, in the setters that can change them, and `gpui::list`
//! asks for rows by index — a ten-thousand-thread snapshot builds a screenful.
//! And rows are uniform: Rename / Archive / Delete live in an upstream popup
//! menu anchored to the row's ellipsis rather than in a toolbar revealed
//! inside the row, so opening one never changes a row's height or shifts the
//! conversations below it, and dismissal plus focus return come from upstream.

use crate::cues::{self, Cue};
use crate::scrolling::PolicyScrollbarExt as _;
use crate::{
    control::composed_button,
    resolved_layout::ResolvedLayoutKey,
    surface::{eyebrow, icon_button},
    theme::SemanticStyledExt as _,
};
use gpui::{
    Anchor, AnyElement, App, AppContext as _, ClickEvent, Context, DismissEvent, ElementId, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement as _, IntoElement, KeyDownEvent,
    ListAlignment, ListOffset, ListState, ParentElement as _, Pixels, Render, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, WeakEntity, Window,
    div, list, prelude::FluentBuilder as _,
};
use gpui_base::StyledExt as _;
use gpui_component::{
    ActiveTheme as _, IconName, h_flex,
    input::{Input, InputEvent, InputState},
    menu::{PopupMenu, PopupMenuItem},
    popover::Popover,
    v_flex,
};
use std::{rc::Rc, sync::Arc};

/// One conversation with stable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadItem {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    archived: bool,
}

impl ThreadItem {
    /// Creates a thread with a stable identifier and visible title.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            archived: false,
        }
    }

    /// Adds secondary text (a timestamp or the last message).
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Marks the thread archived; archived threads hide until requested.
    pub fn archived(mut self, archived: bool) -> Self {
        self.archived = archived;
        self
    }

    /// Returns the stable thread identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Returns the secondary text, when present.
    pub fn subtitle_text(&self) -> Option<&SharedString> {
        self.subtitle.as_ref()
    }

    /// Whether the thread is archived.
    pub fn is_archived(&self) -> bool {
        self.archived
    }

    fn matches(&self, query: &str) -> bool {
        if query.is_empty() {
            return true;
        }
        let needle = query.to_lowercase();
        self.title.to_lowercase().contains(&needle)
            || self
                .subtitle
                .as_ref()
                .is_some_and(|subtitle| subtitle.to_lowercase().contains(&needle))
    }
}

/// A labeled group of threads (for example "Today").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadSection {
    id: SharedString,
    label: SharedString,
    items: Vec<ThreadItem>,
}

impl ThreadSection {
    /// Creates a section with a stable identifier and visible label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            items: Vec::new(),
        }
    }

    /// Sets the section's threads, in display order.
    pub fn items(mut self, items: impl IntoIterator<Item = ThreadItem>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Returns the stable section identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }

    /// Returns the section's threads.
    pub fn thread_items(&self) -> &[ThreadItem] {
        &self.items
    }
}

/// An interaction emitted by [`ThreadList`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThreadListEvent {
    /// The user chose a thread.
    Selected {
        /// Stable thread identifier.
        id: SharedString,
    },
    /// The user asked for a new conversation.
    NewRequested,
    /// The user wants to rename a thread.
    RenameRequested {
        /// Stable thread identifier.
        id: SharedString,
    },
    /// The user wants to archive a thread.
    ArchiveRequested {
        /// Stable thread identifier.
        id: SharedString,
    },
    /// The user wants to restore an archived thread.
    UnarchiveRequested {
        /// Stable thread identifier.
        id: SharedString,
    },
    /// The user wants to delete a thread.
    DeleteRequested {
        /// Stable thread identifier.
        id: SharedString,
    },
    /// The search query changed.
    QueryChanged {
        /// The complete current query.
        query: SharedString,
    },
}

fn sections_are_well_formed(sections: &[ThreadSection]) -> bool {
    let mut section_ids = std::collections::HashSet::new();
    let mut item_ids = std::collections::HashSet::new();
    sections.iter().all(|section| {
        section_ids.insert(&section.id)
            && section.items.iter().all(|item| item_ids.insert(&item.id))
    })
}

/// One row of the flattened snapshot the virtual list indexes into.
///
/// Sections are flattened rather than nested because a list that virtualizes
/// sections still builds every thread inside a visible section. Headers travel
/// as rows of their own so one list owns the whole document and a header
/// scrolls away with the threads it labels.
#[derive(Clone, Debug, PartialEq, Eq)]
enum ThreadRow {
    /// A section eyebrow.
    Header {
        /// Stable section identifier, used to re-find the row after remeasure.
        section_id: SharedString,
        /// The section's visible label.
        label: SharedString,
    },
    /// A conversation and its 1-based position among the visible threads.
    Thread {
        /// The thread itself; cloning is three refcount bumps and a bool.
        item: ThreadItem,
        /// 1-based position across the whole listbox, for `aria-posinset`.
        position: usize,
    },
}

impl ThreadRow {
    /// The stable thread ID, for rows that are threads.
    fn thread_id(&self) -> Option<&SharedString> {
        match self {
            Self::Header { .. } => None,
            Self::Thread { item, .. } => Some(&item.id),
        }
    }

    /// Identity that survives a rebuild, so a scroll anchor can be re-found.
    ///
    /// Section and thread identifiers live in separate namespaces and may
    /// legitimately collide, so the kind travels with the ID.
    fn anchor(&self) -> (bool, SharedString) {
        match self {
            Self::Header { section_id, .. } => (true, section_id.clone()),
            Self::Thread { item, .. } => (false, item.id.clone()),
        }
    }
}

/// Row construction counts for one draw.
///
/// The whole point of virtualizing is that a draw builds a window, not a
/// snapshot, so the bound has to be observable. Counting is test-only:
/// production builds carry neither the field nor the branch.
#[cfg(test)]
#[derive(Default, Clone)]
struct ThreadConstructionCounts {
    rows: std::rc::Rc<std::cell::Cell<usize>>,
    threads: std::rc::Rc<std::cell::Cell<usize>>,
}

#[cfg(test)]
impl ThreadConstructionCounts {
    /// Opens a draw. Counts describe one draw, never a running total.
    fn start_draw(&self) {
        self.rows.set(0);
        self.threads.set(0);
    }

    fn count_row(&self) {
        self.rows.set(self.rows.get() + 1);
    }

    fn count_thread(&self) {
        self.threads.set(self.threads.get() + 1);
    }
}

/// Everything a row needs that does not come from the row itself.
///
/// The virtual list hands its item renderer an `&mut App`, not this entity's
/// context, so intents travel back through a weak handle rather than through
/// `cx.listener`. Values are snapshotted per draw: changing the active or
/// focused thread notifies, which rebuilds this and re-renders the window.
#[derive(Clone)]
struct RowRenderer {
    component: SharedString,
    owner: WeakEntity<ThreadList>,
    /// The listbox's single focus handle; the popup menu returns focus here.
    list_focus: FocusHandle,
    active: Option<SharedString>,
    focused: Option<SharedString>,
    visible_threads: usize,
    /// The list's glide-hover state when the gliding highlight is on;
    /// rows feed it their bounds and hover instead of painting hover.
    glide: Option<gpui::Entity<crate::glide::GlideHover>>,
    #[cfg(test)]
    construction: ThreadConstructionCounts,
}

/// The inset every rectangle in the list shares — the new-chat control,
/// the search field, and each row — measured from the panel's edge.
fn row_gutter(cx: &App) -> gpui::Pixels {
    cx.theme().semantic_tokens().spacing.sm
}

/// The inset from a row's own edge to its text. Section labels stand
/// inside the same gutter and add it too, so headings and row titles
/// share one left edge.
fn row_text_inset(cx: &App) -> gpui::Pixels {
    cx.theme().semantic_tokens().spacing.xs
}

impl RowRenderer {
    fn render(&self, row: &ThreadRow, window: &mut Window, cx: &mut App) -> AnyElement {
        #[cfg(test)]
        self.construction.count_row();
        match row {
            ThreadRow::Header { section_id, label } => self.render_header(section_id, label, cx),
            ThreadRow::Thread { item, position } => self.render_thread(item, *position, window, cx),
        }
    }

    fn render_header(
        &self,
        section_id: &SharedString,
        label: &SharedString,
        cx: &mut App,
    ) -> AnyElement {
        let tokens = cx.theme().semantic_tokens();
        // A section label sits on the same left edge as the row titles it
        // names: both stand inside the list's gutter and add the row's own
        // text inset.
        let header_key = section_id.clone();
        let header_label_key = section_id.clone();
        let section_id =
            ElementId::from((ElementId::from(self.component.clone()), section_id.clone()));
        div()
            .id((section_id, "header"))
            .debug_selector(move || format!("thread-section-{header_key}"))
            // Flattening costs the containment a nested group would give, so
            // the header keeps an accessible node of its own: the boundary is
            // still announced, it just no longer wraps its options.
            .role(Role::Label)
            .aria_label(label.clone())
            .w_full()
            .min_w_0()
            .border_1()
            .border_color(cx.theme().transparent)
            .px(row_text_inset(cx))
            .pt(tokens.spacing.sm)
            .pb(tokens.spacing.xxs)
            .child({
                div()
                    .debug_selector(move || format!("thread-section-label-{header_label_key}"))
                    .child(eyebrow(label.clone(), cx))
            })
            .into_any_element()
    }

    fn render_thread(
        &self,
        item: &ThreadItem,
        position: usize,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        #[cfg(test)]
        self.construction.count_thread();

        let tokens = cx.theme().semantic_tokens();
        let root_id = ElementId::from(self.component.clone());
        let item_id = ElementId::from((root_id.clone(), item.id.clone()));
        let selected = self.active.as_ref() == Some(&item.id);
        // The marker a mounted list starts with is placed settled; a row the
        // user activates fades its surface in once at the quick tempo. Only
        // constructed rows reach here, so offscreen rows spend nothing, and
        // a row scrolled out and back keeps its acknowledged state.
        let acknowledged = crate::motion::acknowledged_state(
            ElementId::from((item_id.clone(), "active-ack")),
            selected as u64,
            window,
            cx,
        );
        let focused = self.focused.as_ref() == Some(&item.id);
        let accessibility_label: SharedString = if item.archived {
            format!("{}, archived", item.title).into()
        } else {
            item.title.clone()
        };
        let debug_id = item.id.clone();
        let select_id = item.id.clone();
        let owner = self.owner.clone();

        // The whole row — trailing actions included — is one selection
        // surface: an inset fill at the nesting-rule radius against the
        // list's panel, hover covering everything inside it, and the
        // acknowledged fade playing on the fill. The select control keeps
        // the semantics and the keyboard focus ring; it paints nothing.
        // With the gliding highlight on, the row paints no hover of its
        // own and reports its geometry and hover to the shared state.
        let row_debug_id = item.id.clone();
        let base = div()
            .id((item_id.clone(), "row"))
            .debug_selector(move || format!("thread-row-{row_debug_id}"));
        crate::surface::selection_surface(
            base,
            selected,
            tokens.radius.lg,
            tokens.spacing.xs,
            self.glide.as_ref().map(|glide| (&item.id, glide)),
            cx,
        )
        // The acknowledged fade plays on the fill alone — the row's
        // content never dims — by re-stating the selected background
        // at the acknowledgment's alpha over the surface's own.
        .when(selected, |row| {
            row.bg(cx.theme().list_active.opacity(acknowledged))
        })
        // One rect is the row: selection fills it, the keyboard ring
        // outlines it, and the hover highlight covers it — trailing
        // actions included. A ring around the label alone stopped
        // before the ellipsis and made focus look unlike selection.
        .border_1()
        .border_color(if focused {
            cx.theme().ring
        } else {
            cx.theme().transparent
        })
        .w_full()
        .min_w_0()
        .flex()
        .items_center()
        .gap(tokens.spacing.xxs)
        .mb(tokens.spacing.xxs)
        .pr(tokens.spacing.xxs)
        .child(
            composed_button((item_id.clone(), "select"), accessibility_label)
                .debug_selector(move || format!("thread-{debug_id}"))
                .role(Role::ListBoxOption)
                .selected(selected)
                .aria_selected(selected)
                .aria_position_in_set(position)
                .aria_size_of_set(self.visible_threads)
                // Keyboard focus stays on the listbox and travels as an
                // active descendant, so a row scrolling out of the window
                // cannot take the focus with it.
                .when(focused, |button| button.aria_active_descendant())
                .tab_stop(false)
                .flex_1()
                .min_w_0()
                .justify_start()
                .px(row_text_inset(cx))
                .py(tokens.spacing.xs)
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .items_start()
                        .child({
                            let title_debug = item.id.clone();
                            div()
                                .debug_selector(move || format!("thread-title-{title_debug}"))
                                .w_full()
                                .truncate()
                                .text_token(tokens.typography.sm)
                                .text_color(cx.theme().foreground)
                                .child(item.title.clone())
                        })
                        .when_some(item.subtitle.clone(), |this, subtitle| {
                            this.child(crate::surface::hint(subtitle, cx).w_full().truncate())
                        }),
                )
                .on_click(move |_, window, cx| {
                    let _ = owner.update(cx, |this, cx| {
                        this.focus_thread(select_id.clone(), window, cx);
                        cx.emit(ThreadListEvent::Selected {
                            id: select_id.clone(),
                        });
                        cues::emit(cx, Cue::ThreadSelected);
                    });
                }),
        )
        .child(self.render_actions_menu(item, item_id, window, cx))
        .into_any_element()
    }

    /// The row's ellipsis and the popup menu it opens.
    ///
    /// Upstream owns the popup: `Popover` owns the open lifecycle, placement,
    /// and outside dismissal, and `PopupMenu` owns the items, their keyboard
    /// model, Escape, and the return of focus to the listbox. The row keeps no
    /// "which row is open" state, so no row can change height.
    fn render_actions_menu(
        &self,
        item: &ThreadItem,
        item_id: ElementId,
        window: &mut Window,
        cx: &mut App,
    ) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let more_debug_id = item.id.clone();
        let list_focus = self.list_focus.clone();
        let owner = self.owner.clone();
        let action_id = item.id.clone();
        let archived = item.archived;
        // The menu is rebuilt on each open, so the builder clones rather than
        // moves: it must be able to run again.
        ThreadActionsMenu {
            id: ElementId::from((item_id.clone(), "more-menu")),
            trigger: icon_button(
                (item_id, "more"),
                IconName::Ellipsis,
                format!("More actions for {}", item.title),
                window,
                cx,
            )
            .debug_selector(move || format!("thread-more-{more_debug_id}"))
            .rounded(tokens.radius.sm),
            build: Rc::new(move |menu, _, _| {
                let rename_id = action_id.clone();
                let archive_id = action_id.clone();
                let delete_id = action_id.clone();
                let rename_owner = owner.clone();
                let archive_owner = owner.clone();
                let delete_owner = owner.clone();
                menu.action_context(list_focus.clone())
                    .item(PopupMenuItem::new("Rename").on_click(move |_, _, cx| {
                        let _ = rename_owner.update(cx, |_, cx| {
                            cx.emit(ThreadListEvent::RenameRequested {
                                id: rename_id.clone(),
                            });
                        });
                    }))
                    .item(
                        PopupMenuItem::new(if archived { "Unarchive" } else { "Archive" })
                            .on_click(move |_, _, cx| {
                                let _ = archive_owner.update(cx, |_, cx| {
                                    cx.emit(if archived {
                                        ThreadListEvent::UnarchiveRequested {
                                            id: archive_id.clone(),
                                        }
                                    } else {
                                        ThreadListEvent::ArchiveRequested {
                                            id: archive_id.clone(),
                                        }
                                    });
                                });
                            }),
                    )
                    .item(PopupMenuItem::new("Delete").on_click(move |_, _, cx| {
                        let _ = delete_owner.update(cx, |_, cx| {
                            cx.emit(ThreadListEvent::DeleteRequested {
                                id: delete_id.clone(),
                            });
                        });
                    }))
            }),
        }
    }
}

/// The menu entity a row keeps between the frame that opens it and the frame
/// that dismisses it.
#[derive(Default)]
struct ThreadActionsMenuState {
    open: bool,
    menu: Option<Entity<PopupMenu>>,
}

/// Fills a freshly built menu with a row's actions.
///
/// Called on every open rather than once, so an item's label and handler are
/// always the ones the current snapshot implies — Archive and Unarchive are
/// the same item under two names.
type ThreadMenuBuilder = dyn Fn(PopupMenu, &mut Window, &mut Context<PopupMenu>) -> PopupMenu;

/// An ellipsis trigger and the popup menu it opens.
///
/// This composes upstream `Popover` and `PopupMenu` by hand rather than using
/// the `DropdownMenu` trait, which is implemented only for upstream's own
/// `Button` — and that button takes its accessible name from a visible label,
/// which an icon-only control does not have. Everything else, including
/// rebuilding the menu on each open so item state is never stale, follows
/// `DropdownMenuPopover`.
#[derive(IntoElement)]
struct ThreadActionsMenu {
    id: ElementId,
    trigger: gpui_base::Button,
    build: Rc<ThreadMenuBuilder>,
}

impl RenderOnce for ThreadActionsMenu {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let build = self.build.clone();
        let state = window.use_keyed_state((self.id.clone(), "menu"), cx, |_, _| {
            ThreadActionsMenuState::default()
        });
        let open = state.read(cx).open;
        let trigger_state = state.clone();
        let trigger = self.trigger.on_click(move |event, _, cx| {
            if matches!(event, ClickEvent::Keyboard(_)) {
                trigger_state.update(cx, |state, cx| {
                    state.open = !state.open;
                    if !state.open {
                        state.menu = None;
                    }
                    cx.notify();
                });
            }
        });
        let open_state = state.clone();

        Popover::new(self.id)
            // The menu paints its own surface, and dismisses its own outside
            // presses, so the popover must add neither.
            .appearance(false)
            .overlay_closable(false)
            // The menu's top-right corner meets the trigger's, so it drops
            // downward flush with the row's trailing edge.
            .anchor(Anchor::TopRight)
            .open(open)
            .on_open_change(move |open, _, cx| {
                open_state.update(cx, |state, cx| {
                    state.open = *open;
                    if !open {
                        state.menu = None;
                    }
                    cx.notify();
                });
            })
            .trigger(trigger)
            .content(move |_, window, cx| {
                if let Some(menu) = state.read(cx).menu.clone() {
                    return div()
                        .debug_selector(|| "thread-actions-menu".into())
                        .child(menu);
                }
                let build = build.clone();
                let menu =
                    PopupMenu::build(window, cx, move |menu, window, cx| build(menu, window, cx));
                state.update(cx, |this, _| this.menu = Some(menu.clone()));
                menu.focus_handle(cx).focus(window, cx);

                window
                    .subscribe(&menu, cx, {
                        let state = state.clone();
                        move |_, _: &DismissEvent, _, cx| {
                            state.update(cx, |state, cx| {
                                state.open = false;
                                state.menu = None;
                                cx.notify();
                            });
                        }
                    })
                    .detach();
                div()
                    .debug_selector(|| "thread-actions-menu".into())
                    .child(menu)
            })
    }
}

/// A controlled conversation list with native search.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::AppContext;
/// # fn example(window: &mut gpui::Window, cx: &mut gpui::App) {
/// let threads = cx.new(|cx| ThreadList::new("threads", window, cx));
/// threads.update(cx, |threads, cx| {
///     threads.set_sections(
///         [ThreadSection::new("today", "Today").items([
///             ThreadItem::new("t-1", "Supplier pricing").subtitle("2 min ago"),
///         ])],
///         cx,
///     );
///     threads.set_active(Some("t-1"), cx);
/// });
/// # }
/// ```
pub struct ThreadList {
    /// Styles the caller put on this component, applied to its own frame.
    ///
    /// Last, so a caller outranks the component's defaults - the same rule the
    /// builder components follow. A wrapper `div` cannot stand in for this:
    /// a background, a border, or an ink set on a wrapper paints around the
    /// component rather than on it.
    style: gpui::StyleRefinement,
    id: SharedString,
    sections: Arc<[ThreadSection]>,
    active: Option<SharedString>,
    query: SharedString,
    input: Entity<InputState>,
    show_archived: bool,
    /// The flattened visible snapshot; rebuilt in setters, never in `Render`.
    rows: Arc<[ThreadRow]>,
    /// Threads in `rows`, for `aria-setsize`.
    visible_threads: usize,
    /// Threads that survive the archive filter, whatever the query — the
    /// difference between "no conversations yet" and "nothing matches".
    candidates: usize,
    /// The row holding keyboard focus, by stable ID. Distinct from `active`,
    /// which the application owns: moving focus does not select.
    focused_thread: Option<SharedString>,
    /// The listbox's single tab stop; options are not focusable themselves.
    list_focus: FocusHandle,
    list_state: ListState,
    /// One highlight glides between rows instead of per-row hover fills.
    /// Default on; [`Self::set_hover_glide`] restores per-row hover.
    hover_glide: bool,
    /// Rem size the retained row heights were measured against.
    resolved_layout: ResolvedLayoutKey,
    #[cfg(test)]
    construction: ThreadConstructionCounts,
    _input_subscription: Subscription,
}

impl ThreadList {
    /// Chooses between the gliding hover highlight (the default) and
    /// classic per-row hover fills.
    pub fn set_hover_glide(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.hover_glide != enabled {
            self.hover_glide = enabled;
            cx.notify();
        }
    }

    /// Creates an empty list around a native search input.
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search conversations"));
        let subscription =
            cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
                if let InputEvent::Change = event {
                    let query: SharedString = input.read(cx).value().to_string().into();
                    this.apply_query(query, cx);
                }
            });
        Self {
            style: gpui::StyleRefinement::default(),
            id: id.into(),
            sections: Arc::from([]),
            active: None,
            query: "".into(),
            input,
            show_archived: false,
            rows: Arc::from([]),
            visible_threads: 0,
            candidates: 0,
            focused_thread: None,
            list_focus: cx.focus_handle(),
            list_state: ListState::new(0, ListAlignment::Top, Pixels::ZERO),
            hover_glide: true,
            resolved_layout: ResolvedLayoutKey::default(),
            #[cfg(test)]
            construction: ThreadConstructionCounts::default(),
            _input_subscription: subscription,
        }
    }

    /// Replaces the controlled sections. Snapshots with duplicate section or
    /// thread IDs are ignored so identities can never alias.
    pub fn set_sections(
        &mut self,
        sections: impl IntoIterator<Item = ThreadSection>,
        cx: &mut Context<Self>,
    ) {
        let sections: Arc<[ThreadSection]> = sections.into_iter().collect();
        if !sections_are_well_formed(&sections) || self.sections == sections {
            return;
        }
        self.sections = sections;
        self.rebuild_rows();
        cx.notify();
    }

    /// Sets the active thread by stable ID (or none).
    pub fn set_active(&mut self, id: Option<impl Into<SharedString>>, cx: &mut Context<Self>) {
        let id = id.map(Into::into);
        if self.active != id {
            self.active = id;
            // Selection repaints a row; it never changes its height, so the
            // measured snapshot and the reader's scroll position both stand.
            cx.notify();
        }
    }

    /// Returns the active thread ID.
    pub fn active_id(&self) -> Option<&SharedString> {
        self.active.as_ref()
    }

    /// Shows or hides archived threads.
    pub fn set_show_archived(&mut self, show: bool, cx: &mut Context<Self>) {
        if self.show_archived != show {
            self.show_archived = show;
            self.rebuild_rows();
            cx.notify();
        }
    }

    /// Whether archived threads are shown.
    pub fn shows_archived(&self) -> bool {
        self.show_archived
    }

    /// Sets the search query programmatically.
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
        self.input.update(cx, |input, cx| {
            input.set_value(query.to_string(), window, cx)
        });
        // InputState suppresses Change for programmatic values.
        self.apply_query(query, cx);
    }

    /// Returns the current search query.
    pub fn query(&self) -> &SharedString {
        &self.query
    }

    /// Moves keyboard focus into the search input.
    pub fn focus_search(&self, window: &mut Window, cx: &mut App) {
        self.input.read(cx).focus_handle(cx).focus(window, cx);
    }

    /// Scrolls the list to its end (the oldest visible thread).
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.list_state.scroll_to_end();
        cx.notify();
    }

    fn apply_query(&mut self, query: SharedString, cx: &mut Context<Self>) {
        if self.query == query {
            return;
        }
        self.query = query.clone();
        self.rebuild_rows();
        cx.emit(ThreadListEvent::QueryChanged { query });
        cx.notify();
    }

    /// Rebuilds the flattened row snapshot the virtual list indexes into.
    ///
    /// Called from every setter that can change what is visible, and never
    /// from `Render`: the list asks for rows by index during layout, so a
    /// snapshot rebuilt per frame would cost exactly what virtualizing saves.
    fn rebuild_rows(&mut self) {
        let sections = self.sections.clone();
        let mut rows: Vec<ThreadRow> = Vec::new();
        let mut visible_threads = 0usize;
        let mut candidates = 0usize;
        for section in sections.iter() {
            let mut header_written = false;
            for item in section.items.iter() {
                if !self.show_archived && item.archived {
                    continue;
                }
                candidates += 1;
                if !item.matches(&self.query) {
                    continue;
                }
                if !header_written {
                    rows.push(ThreadRow::Header {
                        section_id: section.id.clone(),
                        label: section.label.clone(),
                    });
                    header_written = true;
                }
                visible_threads += 1;
                rows.push(ThreadRow::Thread {
                    item: item.clone(),
                    position: visible_threads,
                });
            }
        }
        self.visible_threads = visible_threads;
        self.candidates = candidates;
        // Keyboard focus is an ID, not an index: it survives a reorder or a
        // narrowing query as long as the thread it names is still visible.
        if !self
            .focused_thread
            .as_ref()
            .is_some_and(|id| rows.iter().any(|row| row.thread_id() == Some(id)))
        {
            self.focused_thread = None;
        }
        self.rows = rows.into();
        self.list_state.reset(self.rows.len());
    }

    /// Re-measures the rows after the window's rem size changed.
    ///
    /// Row heights cache text laid out at the previous rem, and neither a
    /// snapshot nor a filter reports a zoom change. The row that was first on
    /// screen stays first.
    fn resolve_layout(&mut self, rem_size: Pixels, cx: &mut Context<Self>) {
        if !self.resolved_layout.observe(rem_size) {
            return;
        }
        let offset = self.list_state.logical_scroll_top();
        let anchor = self
            .rows
            .get(offset.item_ix)
            .map(|row| (row.anchor(), offset.offset_in_item));

        self.list_state.remeasure();
        if let Some((anchor, offset_in_item)) = anchor
            && let Some(item_ix) = self.rows.iter().position(|row| row.anchor() == anchor)
        {
            self.list_state.scroll_to(ListOffset {
                item_ix,
                offset_in_item,
            });
        }
        cx.notify();
    }

    /// Flattened index of the row for `id`, when it is visible.
    fn row_index_of(&self, id: &SharedString) -> Option<usize> {
        self.rows.iter().position(|row| row.thread_id() == Some(id))
    }

    /// Moves the roving focus onto `id` and keeps that row on screen.
    fn focus_thread(&mut self, id: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(index) = self.row_index_of(&id) {
            self.list_state.scroll_to_reveal_item(index);
        }
        self.focused_thread = Some(id);
        self.list_focus.focus(window, cx);
        cx.notify();
    }

    /// The stable IDs of the visible threads, in display order.
    fn focusable_ids(&self) -> Vec<SharedString> {
        self.rows
            .iter()
            .filter_map(|row| row.thread_id().cloned())
            .collect()
    }

    /// Moves the roving focus by `delta` options, clamped at both ends.
    ///
    /// Section headers and threads hidden by the archive filter or the query
    /// are not options, so they are never landed on: only rows that are in the
    /// snapshot can hold focus. Reaching an end stays there rather than
    /// wrapping, so Home and End remain the only way to jump the list.
    fn move_focus(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.focusable_ids();
        if ids.is_empty() {
            return;
        }
        let last = ids.len() - 1;
        let next = match self
            .focused_thread
            .as_ref()
            .and_then(|id| ids.iter().position(|candidate| candidate == id))
        {
            Some(current) => current.saturating_add_signed(delta).min(last),
            None if delta < 0 => last,
            None => 0,
        };
        let id = ids[next].clone();
        self.focus_thread(id, window, cx);
    }

    /// Moves the roving focus to the first or last option.
    fn focus_bound(&mut self, last: bool, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.focusable_ids();
        let Some(id) = (if last { ids.last() } else { ids.first() }) else {
            return;
        };
        let id = id.clone();
        self.focus_thread(id, window, cx);
    }

    /// Selects the focused option, the way clicking it would.
    fn activate_focused(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.focused_thread.clone() else {
            return;
        };
        cx.emit(ThreadListEvent::Selected { id });
        cues::emit(cx, Cue::ThreadSelected);
    }

    /// The listbox's keyboard model, on stable IDs.
    ///
    /// Only reaches here when focus is inside the listbox, so the search
    /// input's own caret keys are untouched. Escape is deliberately absent:
    /// while a row's menu is open the menu holds focus and closes itself
    /// first, and with no menu open Escape belongs to the application.
    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event.keystroke.key.as_str() {
            "up" => self.move_focus(-1, window, cx),
            "down" => self.move_focus(1, window, cx),
            "home" => self.focus_bound(false, window, cx),
            "end" => self.focus_bound(true, window, cx),
            "enter" | "space" => self.activate_focused(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl EventEmitter<ThreadListEvent> for ThreadList {}

impl Focusable for ThreadList {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.read(cx).focus_handle(cx)
    }
}

impl gpui::Styled for ThreadList {
    fn style(&mut self) -> &mut gpui::StyleRefinement {
        &mut self.style
    }
}

impl Render for ThreadList {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Measured row heights are only valid for the rem they were laid out
        // at. Reading it here mutates nothing; the reaction is deferred so
        // that render never notifies.
        let rem_size = window.rem_size();
        if !self.resolved_layout.matches(rem_size) {
            cx.defer_in(window, move |this, _, cx| {
                this.resolve_layout(rem_size, cx);
            });
        }
        #[cfg(test)]
        self.construction.start_draw();

        let tokens = cx.theme().semantic_tokens();
        let root_id = ElementId::from(self.id.clone());
        let show_archived = self.show_archived;
        let empty_message: Option<SharedString> = if self.candidates == 0 {
            Some("No conversations yet".into())
        } else if self.rows.is_empty() {
            Some(format!("No conversations match “{}”", self.query).into())
        } else {
            None
        };
        let rows = self.rows.clone();
        let glide = self
            .hover_glide
            .then(|| crate::glide::glide_hover_state(root_id.clone(), window, cx));
        let renderer = RowRenderer {
            component: self.id.clone(),
            owner: cx.weak_entity(),
            list_focus: self.list_focus.clone(),
            active: self.active.clone(),
            focused: self.focused_thread.clone(),
            visible_threads: self.visible_threads,
            glide: glide.clone(),
            #[cfg(test)]
            construction: self.construction.clone(),
        };
        let list_state = self.list_state.clone();

        v_flex()
            .id(root_id.clone())
            .role(Role::Group)
            .aria_label("Conversations")
            .size_full()
            .min_h_0()
            .min_w_0()
            .gap(tokens.spacing.sm)
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .px(row_gutter(cx))
                    .gap(tokens.spacing.xs)
                    .child(
                        div().debug_selector(|| "thread-list-new".into()).child(
                            crate::control::outlined_control_with_icon(
                                (root_id.clone(), "new"),
                                "New chat",
                                IconName::Plus,
                                "New chat",
                                window,
                                cx,
                            )
                            .accessibility_id(format!("{}-new", self.id))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(ThreadListEvent::NewRequested);
                            })),
                        ),
                    )
                    .child(div().flex_1())
                    .child(
                        icon_button(
                            (root_id.clone(), "archived"),
                            IconName::Inbox,
                            if show_archived {
                                "Hide archived conversations"
                            } else {
                                "Show archived conversations"
                            },
                            window,
                            cx,
                        )
                        .debug_selector(|| "thread-list-archived-toggle".into())
                        .selected(show_archived)
                        .aria_expanded(show_archived)
                        .on_click(cx.listener(|this, _, _, cx| {
                            let show = !this.show_archived;
                            this.set_show_archived(show, cx);
                        })),
                    ),
            )
            .child(
                h_flex()
                    .debug_selector(|| "thread-list-search".into())
                    .w_full()
                    .items_center()
                    .px(row_gutter(cx))
                    .child(
                        div()
                            .debug_selector(|| "thread-list-field".into())
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&self.input).prefix(IconName::Search)),
                    ),
            )
            .child(
                // The outer frame owns the flex constraint; the virtual list
                // fills it, so a long snapshot scrolls a window of rows
                // instead of growing the pane or building the whole document.
                {
                    let frame = div()
                        .id((root_id.clone(), "list-frame"))
                        .debug_selector(|| "thread-list-scroll".into())
                        .relative()
                        .flex_1()
                        .min_h_0()
                        .w_full()
                        .overflow_hidden()
                        .policy_vertical_scrollbar(&list_state, cx);
                    // The gliding highlight paints beneath the rows; the
                    // frame's bounds anchor its coordinates.
                    match &glide {
                        Some(glide) => crate::glide::glide_frame(frame, glide).child(
                            crate::glide::glide_highlight(
                                root_id.clone(),
                                glide,
                                crate::surface::nested_radius(
                                    tokens.radius.lg,
                                    tokens.spacing.xs,
                                    tokens.radius.sm,
                                ),
                                "thread-list-glide",
                                window,
                                cx,
                            ),
                        ),
                        None => frame,
                    }
                }
                .child(
                    div()
                        .id((root_id.clone(), "list"))
                        .role(Role::ListBox)
                        .aria_label("Conversation list")
                        // The gutter every row stands on. A virtual list
                        // positions its items itself, so the inset is the
                        // scroll region's, not each row's.
                        .pl(row_gutter(cx))
                        // And the trailing side keeps the overlaid bar's lane
                        // clear, because these rows carry a menu at that edge
                        // and a bar drawn over it is a target nobody can hit.
                        .pr(row_gutter(cx).max(crate::scrolling::control_lane(cx)))
                        .track_focus(&self.list_focus)
                        .tab_stop(true)
                        .on_key_down(cx.listener(Self::on_key_down))
                        .size_full()
                        .min_h_0()
                        .when(!rows.is_empty(), |this| {
                            this.child(
                                list(list_state, move |index, window, cx| {
                                    rows.get(index)
                                        .map(|row| renderer.render(row, window, cx))
                                        .unwrap_or_else(|| div().hidden().into_any_element())
                                })
                                .size_full(),
                            )
                        })
                        .when_some(empty_message, |this, message| {
                            this.child(
                                div()
                                    .id((root_id, "empty"))
                                    .debug_selector(|| "thread-list-empty".into())
                                    .role(Role::Status)
                                    .aria_label(message.clone())
                                    .child(crate::surface::empty_state(
                                        IconName::Inbox,
                                        message.clone(),
                                        Some("Start a chat to see it here".into()),
                                        cx,
                                    )),
                            )
                        }),
                ),
            )
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Bounds, KeyUpEvent, Keystroke, Modifiers, Pixels, TestAppContext, VisualTestContext, px,
        size,
    };
    use std::cell::Cell;

    struct ThreadActionsMenuProbe {
        activations: Rc<Cell<usize>>,
        trigger_focus: FocusHandle,
    }

    impl Render for ThreadActionsMenuProbe {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let activations = self.activations.clone();
            div().tab_group().child(ThreadActionsMenu {
                id: "keyboard-actions".into(),
                trigger: icon_button(
                    "keyboard-actions-trigger",
                    IconName::Ellipsis,
                    "More actions",
                    window,
                    cx,
                )
                .track_focus(&self.trigger_focus)
                .debug_selector(|| "keyboard-actions-trigger".into()),
                build: Rc::new(move |menu, _, _| {
                    let activations = activations.clone();
                    menu.item(PopupMenuItem::new("Rename").on_click(move |_, _, _| {
                        activations.set(activations.get() + 1);
                    }))
                }),
            })
        }
    }

    #[test]
    fn matching_is_case_insensitive_over_title_and_subtitle() {
        let item = ThreadItem::new("t", "Supplier pricing").subtitle("Compared Alpenrose");
        assert!(item.matches(""));
        assert!(item.matches("PRICING"));
        assert!(item.matches("alpenrose"));
        assert!(!item.matches("delivery"));
    }

    #[test]
    fn malformed_snapshots_are_rejected() {
        let duplicate_items = [ThreadSection::new("a", "A")
            .items([ThreadItem::new("t", "One"), ThreadItem::new("t", "Two")])];
        assert!(!sections_are_well_formed(&duplicate_items));
        let duplicate_sections = [
            ThreadSection::new("a", "A"),
            ThreadSection::new("a", "Again"),
        ];
        assert!(!sections_are_well_formed(&duplicate_sections));
        let fine = [
            ThreadSection::new("a", "A").items([ThreadItem::new("t-1", "One")]),
            ThreadSection::new("b", "B").items([ThreadItem::new("t-2", "Two")]),
        ];
        assert!(sections_are_well_formed(&fine));
    }

    #[gpui::test]
    fn row_action_menu_opens_and_activates_from_the_keyboard(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let activations = Rc::new(Cell::new(0));
        let (view, cx) = cx.add_window_view({
            let activations = activations.clone();
            move |_, cx| ThreadActionsMenuProbe {
                activations,
                trigger_focus: cx.focus_handle(),
            }
        });
        let cx: &mut VisualTestContext = cx;
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx.update(|window, cx| {
            let trigger_focus = view.read(cx).trigger_focus.clone();
            trigger_focus.focus(window, cx);
            window.draw(cx).clear(cx);
        });

        let enter = Keystroke::parse("enter").expect("Enter is a valid keystroke");
        cx.simulate_event(KeyDownEvent {
            keystroke: enter.clone(),
            is_held: false,
            prefer_character_input: false,
        });
        cx.simulate_event(KeyUpEvent { keystroke: enter });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds("thread-actions-menu").is_some(),
            "Enter on the named ellipsis button must open the popup"
        );

        cx.simulate_keystrokes("down enter");
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(activations.get(), 1);
        assert!(cx.debug_bounds("thread-actions-menu").is_none());
    }

    /// Rows tall enough that a 480pt viewport holds a couple of dozen, so the
    /// window is unambiguously smaller than the snapshot.
    const MEASURED_VIEWPORT: (f32, f32) = (320., 480.);

    /// Threads per section in the large snapshot; ten sections of a thousand.
    const LARGE_SECTION_SIZE: usize = 1_000;
    const LARGE_SECTIONS: usize = 10;

    #[gpui::test]
    fn activating_a_thread_acknowledges_once_and_a_mounted_marker_is_settled(
        cx: &mut TestAppContext,
    ) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| {
                threads.set_sections(
                    [ThreadSection::new("today", "Today")
                        .items([ThreadItem::new("t-1", "One"), ThreadItem::new("t-2", "Two")])],
                    cx,
                );
                threads.set_active(Some("t-1"), cx);
            });
        });
        redraw(&threads, cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        redraw(&threads, cx);
        crate::motion::take_reveal_frame_requests();
        redraw(&threads, cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "the marker the list mounts with is placed settled"
        );

        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_active(Some("t-2"), cx));
        });
        redraw(&threads, cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "activating a thread must acknowledge the new marker"
        );
    }

    fn large_sections() -> Vec<ThreadSection> {
        (0..LARGE_SECTIONS)
            .map(|section| {
                ThreadSection::new(format!("s-{section}"), format!("Section {section}")).items(
                    (0..LARGE_SECTION_SIZE).map(|row| {
                        let index = section * LARGE_SECTION_SIZE + row;
                        ThreadItem::new(format!("t-{index:05}"), format!("Conversation {index}"))
                            .subtitle(format!("{index} min ago"))
                    }),
                )
            })
            .collect()
    }

    /// Every rectangle in the list stands on one left edge, and every
    /// label — row titles and the section names above them — stands on
    /// one more. The feel review found four edges here instead of two.
    #[gpui::test]
    fn the_list_stands_on_one_left_edge(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(glide_sections(), cx));
        });
        let _ = redraw(&threads, cx);
        let _ = redraw(&threads, cx);

        let new_chat = bounds_of(cx, "thread-list-new".to_owned()).expect("new chat renders");
        let field = bounds_of(cx, "thread-list-field".to_owned()).expect("search renders");
        let row = bounds_of(cx, "thread-row-one".to_owned()).expect("a row renders");
        assert_eq!(
            (new_chat.left(), row.left()),
            (field.left(), field.left()),
            "the control, the field, and the rows share one gutter"
        );

        let header = bounds_of(cx, "thread-section-label-recent".to_owned())
            .expect("the section label renders");
        let title = bounds_of(cx, "thread-title-one".to_owned()).expect("a row title renders");
        assert_eq!(
            header.left(),
            title.left(),
            "a section label and the titles it names share one text edge"
        );
    }

    /// Selection, keyboard focus, and hover all describe the same
    /// rectangle. A ring drawn around the label alone stopped short of the
    /// row's trailing actions and made focus look unlike selection.
    #[gpui::test]
    fn selection_focus_and_hover_describe_one_row_rect(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| {
                threads.set_sections(glide_sections(), cx);
                threads.set_active(Some(SharedString::from("one")), cx);
            });
        });
        let _ = redraw(&threads, cx);
        let _ = redraw(&threads, cx);

        let row = bounds_of(cx, "thread-row-one".to_owned()).expect("the row renders");
        let select = bounds_of(cx, "thread-one".to_owned()).expect("the select control renders");
        assert!(
            row.right() > select.right(),
            "the row rect reaches past its label into the trailing actions:              row {row:?}, label {select:?}"
        );

        cx.simulate_mouse_move(row.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let highlight =
            bounds_of(cx, "thread-list-glide".to_owned()).expect("hovering highlights the row");
        assert_eq!(
            highlight.size.width,
            row.size.width - px(2.),
            "the hover highlight covers the row inside its own border"
        );
    }

    fn glide_sections() -> Vec<ThreadSection> {
        vec![ThreadSection::new("recent", "Recent").items([
            ThreadItem::new("one", "One"),
            ThreadItem::new("two", "Two"),
            ThreadItem::new("three", "Three"),
        ])]
    }

    /// A row's trailing control never sits under the scrollbar.
    ///
    /// The bar is sixteen pixels and overlaid by default, taking no layout
    /// space - which is right for prose, and wrong for the menu at a row's
    /// trailing edge. The row's gutter was eight, so the bar reached half its
    /// width into the row and covered a target nobody could then hit. On a
    /// list whose bar is always shown, that is the resting state.
    ///
    /// Measured against the scroll frame rather than the row, because the bar
    /// is positioned from the frame's trailing edge and that is the only edge
    /// the arithmetic can be anchored to.
    #[gpui::test]
    fn a_row_keeps_its_controls_clear_of_the_scrollbar(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(glide_sections(), cx));
        });
        let _ = redraw(&threads, cx);
        let _ = redraw(&threads, cx);

        let frame = bounds_of(cx, "thread-list-scroll".to_owned()).expect("the list scrolls");
        let row = bounds_of(cx, "thread-row-one".to_owned()).expect("a row renders");
        let bar = gpui_base::Scrollbar::width();
        let gutter = cx.update(|_, cx| super::row_gutter(cx));

        assert!(
            frame.right() - row.right() >= bar,
            "the bar covers this row's trailing edge: row ends {:?} from the frame, bar is {bar:?}",
            frame.right() - row.right()
        );
        // And the reading column keeps what it had: the lane comes off the
        // trailing side only.
        assert_eq!(
            row.left() - frame.left(),
            gutter,
            "the leading gutter is unchanged"
        );
    }

    /// The lag probe: a fresh hover seats the highlight on its row with no
    /// glide from stale geometry, the next hover departs from the previous
    /// row and travels between the two, and the highlight arrives exactly
    /// on the hovered row once the glide tempo has elapsed.
    #[gpui::test]
    fn glide_highlight_appears_on_hover_and_chases_the_next_row(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(glide_sections(), cx));
        });
        let _ = redraw(&threads, cx);
        let _ = redraw(&threads, cx);

        assert!(
            bounds_of(cx, "thread-list-glide".to_owned()).is_none(),
            "no highlight before anything is hovered"
        );

        let row_one = bounds_of(cx, "thread-row-one".to_owned()).expect("row one renders");
        cx.simulate_mouse_move(row_one.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let highlight = bounds_of(cx, "thread-list-glide".to_owned())
            .expect("hovering a row mounts the highlight");
        assert_eq!(
            highlight.size.height,
            row_one.size.height - px(2.),
            "a fresh hover seats the highlight on its row, inside its border"
        );
        assert_eq!(highlight.origin.y, row_one.origin.y + px(1.));

        let row_three = bounds_of(cx, "thread-row-three".to_owned()).expect("row three renders");
        cx.simulate_mouse_move(row_three.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let start = bounds_of(cx, "thread-list-glide".to_owned()).expect("highlight persists");
        assert_eq!(
            start.origin.y,
            row_one.origin.y + px(1.),
            "the glide departs from the previously hovered row"
        );

        cx.executor()
            .advance_clock(std::time::Duration::from_millis(10));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let moving = bounds_of(cx, "thread-list-glide".to_owned()).expect("highlight persists");
        assert!(
            moving.top() > row_one.top() && moving.top() < row_three.top(),
            "mid-glide the highlight sits between the rows: {moving:?}"
        );

        cx.executor()
            .advance_clock(std::time::Duration::from_secs(1));
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let settled = bounds_of(cx, "thread-list-glide".to_owned()).expect("highlight persists");
        assert_eq!(
            settled.origin.y,
            row_three.origin.y + px(1.),
            "the highlight arrives on the hovered row"
        );

        // Settled means settled: nothing moves on further draws.
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert_eq!(
            bounds_of(cx, "thread-list-glide".to_owned()),
            Some(settled),
            "a settled highlight holds its row"
        );
    }

    /// A hovered row that leaves the list takes its highlight with it.
    ///
    /// Hover is cleared by the row's own listener, and an unmounted row has
    /// no listener to fire: filtering the list under a stationary pointer
    /// used to leave the highlight painting the departed row's last
    /// geometry over whatever moved into that space.
    #[gpui::test]
    fn a_filtered_away_row_takes_its_highlight_with_it(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(glide_sections(), cx));
        });
        let _ = redraw(&threads, cx);
        let _ = redraw(&threads, cx);

        let row = bounds_of(cx, "thread-row-three".to_owned()).expect("row three renders");
        cx.simulate_mouse_move(row.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            bounds_of(cx, "thread-list-glide".to_owned()).is_some(),
            "the hovered row is highlighted"
        );

        // Filter it away without moving the pointer.
        cx.update(|window, cx| {
            threads.update(cx, |threads, cx| threads.set_query("One", window, cx));
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            bounds_of(cx, "thread-row-three".to_owned()).is_none(),
            "the filter really removed the row"
        );
        assert!(
            bounds_of(cx, "thread-list-glide".to_owned()).is_none(),
            "and its highlight left with it, rather than painting over what took its place"
        );
    }

    /// Any reduced preference snaps the highlight between rows: the move
    /// lands on the very next draw with no clock advanced.
    #[gpui::test]
    fn glide_snaps_between_rows_under_a_reduced_preference(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::motion::MotionTokens::default()
                .with_preference(crate::motion::MotionPreference::Crossfade)
                .set(cx);
            crate::init(cx);
        });
        let (threads, cx) =
            cx.add_window_view(|window, cx| ThreadList::new("measured", window, cx));
        let cx: &mut VisualTestContext = cx;
        cx.simulate_resize(size(px(MEASURED_VIEWPORT.0), px(MEASURED_VIEWPORT.1)));
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(glide_sections(), cx));
        });
        let _ = redraw(&threads, cx);
        let _ = redraw(&threads, cx);

        let row_one = bounds_of(cx, "thread-row-one".to_owned()).expect("row one renders");
        cx.simulate_mouse_move(row_one.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let row_three = bounds_of(cx, "thread-row-three".to_owned()).expect("row three renders");
        cx.simulate_mouse_move(row_three.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let snapped = bounds_of(cx, "thread-list-glide".to_owned())
            .expect("a reduced preference still highlights the hovered row");
        assert_eq!(
            snapped.origin.y,
            row_three.origin.y + px(1.),
            "a reduced preference snaps the highlight to the hovered row"
        );
    }

    /// Per-row hover is the builder escape: with the glide disabled the
    /// highlight never mounts.
    #[gpui::test]
    fn disabling_the_glide_restores_per_row_hover(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| {
                threads.set_sections(glide_sections(), cx);
                threads.set_hover_glide(false, cx);
            });
        });
        let _ = redraw(&threads, cx);
        let row_one = bounds_of(cx, "thread-row-one".to_owned()).expect("row one renders");
        cx.simulate_mouse_move(row_one.center(), None, Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            bounds_of(cx, "thread-list-glide".to_owned()).is_none(),
            "no gliding highlight when the builder disables it"
        );
    }

    fn measured_list(cx: &mut TestAppContext) -> (Entity<ThreadList>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (threads, cx) =
            cx.add_window_view(|window, cx| ThreadList::new("measured", window, cx));
        let cx: &mut VisualTestContext = cx;
        cx.simulate_resize(size(px(MEASURED_VIEWPORT.0), px(MEASURED_VIEWPORT.1)));
        (threads, cx)
    }

    /// Redraws and reports `(rows, threads)` built by that one draw.
    fn redraw(threads: &Entity<ThreadList>, cx: &mut VisualTestContext) -> (usize, usize) {
        cx.update(|window, cx| {
            threads.update(cx, |_, cx| cx.notify());
            window.draw(cx).clear(cx);
        });
        threads.read_with(cx, |threads, _| {
            (
                threads.construction.rows.get(),
                threads.construction.threads.get(),
            )
        })
    }

    fn bounds_of(cx: &mut VisualTestContext, selector: String) -> Option<Bounds<Pixels>> {
        cx.debug_bounds(Box::leak(selector.into_boxed_str()))
    }

    /// The bound is only a bound if construction tracks the window rather than
    /// the snapshot. Ten thousand conversations must build a screenful on the
    /// first draw and a screenful again after scrolling a page, and the rows
    /// that were not built must be absent from the frame rather than merely
    /// invisible.
    #[gpui::test]
    fn ten_thousand_threads_build_only_the_visible_window(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(large_sections(), cx));
        });

        let total = LARGE_SECTIONS * LARGE_SECTION_SIZE;
        let (rows, built) = redraw(&threads, cx);
        assert!(built > 0, "the first screen must build some threads");
        assert!(
            rows <= 32,
            "a {total}-thread snapshot built {rows} rows on the first draw"
        );
        assert!(
            bounds_of(cx, "thread-t-00000".to_owned()).is_some(),
            "the first thread should be on screen"
        );
        for far in ["thread-t-00500", "thread-t-05000", "thread-t-09999"] {
            assert!(
                bounds_of(cx, far.to_owned()).is_none(),
                "{far} is far outside the window and must not be built"
            );
        }

        // One page down: still a window, and a different one.
        let page = threads.read_with(cx, |threads, _| {
            threads.list_state.logical_scroll_top().item_ix + rows
        });
        cx.update(|_, cx| {
            threads.update(cx, |threads, _| {
                threads.list_state.scroll_to(ListOffset {
                    item_ix: page,
                    offset_in_item: Pixels::ZERO,
                });
            });
        });
        let (scrolled_rows, scrolled_threads) = redraw(&threads, cx);
        assert!(scrolled_threads > 0, "the second page must build threads");
        assert!(
            scrolled_rows <= 32,
            "one page down built {scrolled_rows} rows of {total}"
        );
        assert!(
            bounds_of(cx, "thread-t-00000".to_owned()).is_none(),
            "the first thread scrolled out and must no longer be built"
        );

        // Counts describe one draw, never a running total.
        let (again, _) = redraw(&threads, cx);
        assert_eq!(again, scrolled_rows, "construction must not accumulate");
    }

    /// The snapshot is derived once per change, so the flattening itself is
    /// the contract: headers only for sections that kept a thread, positions
    /// numbered across the whole listbox, and no row for a hidden thread.
    #[gpui::test]
    fn the_flattened_snapshot_carries_headers_and_listbox_positions(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| {
                threads.set_sections(
                    [
                        ThreadSection::new("today", "Today").items([
                            ThreadItem::new("a", "Alpha"),
                            ThreadItem::new("b", "Beta").archived(true),
                        ]),
                        ThreadSection::new("earlier", "Earlier")
                            .items([ThreadItem::new("c", "Gamma")]),
                        ThreadSection::new("empty", "Empty")
                            .items([ThreadItem::new("d", "Delta").archived(true)]),
                    ],
                    cx,
                );
            });
        });
        threads.read_with(cx, |threads, _| {
            assert_eq!(
                threads.rows.as_ref(),
                &[
                    ThreadRow::Header {
                        section_id: "today".into(),
                        label: "Today".into()
                    },
                    ThreadRow::Thread {
                        item: ThreadItem::new("a", "Alpha"),
                        position: 1
                    },
                    ThreadRow::Header {
                        section_id: "earlier".into(),
                        label: "Earlier".into()
                    },
                    ThreadRow::Thread {
                        item: ThreadItem::new("c", "Gamma"),
                        position: 2
                    },
                ],
                "a section whose every thread is archived contributes no header"
            );
            assert_eq!(threads.visible_threads, 2);
            assert_eq!(threads.candidates, 2);
        });
    }

    /// Zoom invalidates measured heights, and the row that was first on screen
    /// must still be first afterwards.
    #[gpui::test]
    fn a_changed_rem_remeasures_and_keeps_the_first_visible_row(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| threads.set_sections(large_sections(), cx));
        });
        redraw(&threads, cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, _| {
                threads.list_state.scroll_to(ListOffset {
                    item_ix: 24,
                    offset_in_item: Pixels::ZERO,
                });
            });
        });
        redraw(&threads, cx);
        let before = threads.read_with(cx, |threads, _| {
            threads.rows[threads.list_state.logical_scroll_top().item_ix].anchor()
        });

        cx.update(|window, cx| {
            window.set_rem_size(px(24.));
            window.draw(cx).clear(cx);
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let after = threads.read_with(cx, |threads, _| {
            threads.rows[threads.list_state.logical_scroll_top().item_ix].anchor()
        });
        assert_eq!(before, after, "zooming must not move the reader");
        assert!(
            threads.read_with(cx, |threads, _| threads.resolved_layout.matches(px(24.))),
            "the new rem must be recorded so the next draw does not remeasure again"
        );
    }

    /// Clicking a row takes the roving focus with it, so the keyboard picks up
    /// where the pointer left off.
    #[gpui::test]
    fn clicking_a_row_moves_the_roving_focus(cx: &mut TestAppContext) {
        let (threads, cx) = measured_list(cx);
        cx.update(|_, cx| {
            threads.update(cx, |threads, cx| {
                threads.set_sections(
                    [ThreadSection::new("today", "Today")
                        .items([ThreadItem::new("a", "Alpha"), ThreadItem::new("b", "Beta")])],
                    cx,
                );
            });
        });
        redraw(&threads, cx);
        let row = bounds_of(cx, "thread-b".to_owned()).expect("the row should render");
        cx.simulate_click(row.center(), Modifiers::default());
        cx.update(|window, cx| window.draw(cx).clear(cx));
        threads.read_with(cx, |threads, _| {
            assert_eq!(threads.focused_thread.as_deref(), Some("b"));
        });
    }
}
