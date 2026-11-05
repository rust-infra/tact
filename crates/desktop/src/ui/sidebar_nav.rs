//! Local, filtered session navigation for the desktop rail.

use gpui::{
    AnyElement, ClickEvent, Context, ElementId, EventEmitter, InteractiveElement as _, IntoElement,
    KeyDownEvent, ParentElement as _, Render, Role, SharedString, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    menu::{ContextMenuExt as _, PopupMenuItem},
    scroll::ScrollableElement as _,
    v_flex,
};

/// One application-owned navigation row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarNavItem {
    id: SharedString,
    label: SharedString,
    icon: Option<SharedString>,
    badge: Option<SharedString>,
    detail: Option<SharedString>,
    trailing: Option<SharedString>,
    context_action: Option<SharedString>,
    new_session_action: Option<SharedString>,
    disabled: bool,
    children: Vec<SidebarNavItem>,
}

impl SidebarNavItem {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            badge: None,
            detail: None,
            trailing: None,
            context_action: None,
            new_session_action: None,
            disabled: false,
            children: Vec::new(),
        }
    }

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn badge(mut self, badge: impl Into<SharedString>) -> Self {
        self.badge = Some(badge.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn trailing(mut self, trailing: impl Into<SharedString>) -> Self {
        self.trailing = Some(trailing.into());
        self
    }

    pub fn context_action(mut self, label: impl Into<SharedString>) -> Self {
        self.context_action = Some(label.into());
        self
    }

    pub fn new_session_action(mut self, label: impl Into<SharedString>) -> Self {
        self.new_session_action = Some(label.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn children(mut self, children: impl IntoIterator<Item = SidebarNavItem>) -> Self {
        self.children = children.into_iter().collect();
        self
    }
}

/// One labelled group of navigation rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SidebarSection {
    id: SharedString,
    label: SharedString,
    leading: Option<SharedString>,
    meta: Option<SharedString>,
    action: Option<SharedString>,
    new_session_action: Option<SharedString>,
    collapsed: bool,
    items: Vec<SidebarNavItem>,
}

impl SidebarSection {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            leading: None,
            meta: None,
            action: None,
            new_session_action: None,
            collapsed: false,
            items: Vec::new(),
        }
    }

    pub fn leading(mut self, leading: impl Into<SharedString>) -> Self {
        self.leading = Some(leading.into());
        self
    }

    pub fn meta(mut self, meta: impl Into<SharedString>) -> Self {
        self.meta = Some(meta.into());
        self
    }

    pub fn action(mut self, label: impl Into<SharedString>) -> Self {
        self.action = Some(label.into());
        self
    }

    pub fn new_session_action(mut self, label: impl Into<SharedString>) -> Self {
        self.new_session_action = Some(label.into());
        self
    }

    pub fn collapsed(mut self, collapsed: bool) -> Self {
        self.collapsed = collapsed;
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = SidebarNavItem>) -> Self {
        self.items = items.into_iter().collect();
        self
    }
}

/// User-visible copy for the navigation.
#[derive(Clone, Debug, PartialEq, Eq)]
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
            new_task: "New run".into(),
            collapse: "Collapse navigation".into(),
            expand: "Expand navigation".into(),
            no_items: "No items".into(),
            no_matches: "No matching items".into(),
            badge_prefix: "Badge".into(),
            contains_selected: "Contains selected item".into(),
            unavailable: "Unavailable".into(),
        }
    }
}

/// How the navigation is hosted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SidebarNavPresentation {
    Standalone,
    #[default]
    Embedded,
}

/// An interaction emitted by [`SidebarNav`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SidebarNavEvent {
    Selected {
        id: SharedString,
        item_id: SharedString,
    },
    NewTaskRequested {
        id: SharedString,
    },
    QueryChanged {
        id: SharedString,
        query: SharedString,
    },
    SectionToggled {
        id: SharedString,
        section_id: SharedString,
        collapsed: bool,
    },
    SectionActionRequested {
        id: SharedString,
        section_id: SharedString,
    },
    SectionNewSessionRequested {
        id: SharedString,
        section_id: SharedString,
    },
    ItemActionRequested {
        id: SharedString,
        item_id: SharedString,
    },
    ItemNewSessionRequested {
        id: SharedString,
        item_id: SharedString,
    },
    CollapsedChanged {
        id: SharedString,
        collapsed: bool,
    },
}

/// A filtered, application-controlled navigation tree.
pub struct SidebarNav {
    id: SharedString,
    sections: Vec<SidebarSection>,
    labels: SidebarNavLabels,
    active_item: Option<SharedString>,
    query: SharedString,
    chrome_visible: bool,
    compact_rows: bool,
    presentation: SidebarNavPresentation,
    collapsed: bool,
}

impl SidebarNav {
    pub fn new(id: impl Into<SharedString>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            sections: Vec::new(),
            labels: SidebarNavLabels::default(),
            active_item: None,
            query: SharedString::default(),
            chrome_visible: true,
            compact_rows: false,
            presentation: SidebarNavPresentation::Embedded,
            collapsed: false,
        }
    }

    pub fn with_presentation(mut self, presentation: SidebarNavPresentation) -> Self {
        self.presentation = presentation;
        self
    }

    pub fn with_compact_rows(mut self, compact: bool) -> Self {
        self.compact_rows = compact;
        self
    }

    pub fn set_sections(
        &mut self,
        sections: impl IntoIterator<Item = SidebarSection>,
        cx: &mut Context<Self>,
    ) {
        let sections: Vec<SidebarSection> = sections.into_iter().collect();
        if self.sections == sections {
            return;
        }
        self.sections = sections;
        cx.notify();
    }

    pub fn set_active_item(&mut self, item_id: impl Into<SharedString>, cx: &mut Context<Self>) {
        let item_id = item_id.into();
        if self.active_item.as_ref() != Some(&item_id) {
            self.active_item = Some(item_id);
            cx.notify();
        }
    }

    pub fn clear_active_item(&mut self, cx: &mut Context<Self>) {
        if self.active_item.take().is_some() {
            cx.notify();
        }
    }

    pub fn set_labels(
        &mut self,
        labels: SidebarNavLabels,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.labels == labels {
            return;
        }
        self.labels = labels;
        cx.notify();
    }

    pub fn set_chrome_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.chrome_visible != visible {
            self.chrome_visible = visible;
            cx.notify();
        }
    }

    pub fn set_query(
        &mut self,
        query: impl Into<SharedString>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = query.into();
        if self.query != query {
            self.query = query;
            cx.notify();
        }
    }

    pub fn set_collapsed(&mut self, collapsed: bool, cx: &mut Context<Self>) {
        if self.collapsed != collapsed {
            self.collapsed = collapsed;
            cx.emit(SidebarNavEvent::CollapsedChanged {
                id: self.id.clone(),
                collapsed,
            });
            cx.notify();
        }
    }

    fn matches(&self, item: &SidebarNavItem) -> bool {
        let query = self.query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        item.label.to_lowercase().contains(&query)
            || item.id.to_lowercase().contains(&query)
            || item
                .detail
                .as_ref()
                .is_some_and(|detail| detail.to_lowercase().contains(&query))
            || item
                .trailing
                .as_ref()
                .is_some_and(|trailing| trailing.to_lowercase().contains(&query))
    }

    fn render_item(&self, item: &SidebarNavItem, cx: &mut Context<Self>) -> AnyElement {
        if !self.matches(item) {
            return div().into_any_element();
        }
        let active = self.active_item.as_ref() == Some(&item.id);
        let id = item.id.clone();
        let click_id = item.id.clone();
        let debug_id = item.id.clone();
        let event = SidebarNavEvent::Selected {
            id: self.id.clone(),
            item_id: item.id.clone(),
        };
        let label_size = if self.compact_rows { px(12.) } else { px(14.) };
        let meta_size = if self.compact_rows { px(10.) } else { px(12.) };
        let mut row = v_flex()
            .id(SharedString::from(format!("sidebar-nav-item-{debug_id}")))
            .debug_selector(move || format!("sidebar-nav-item-{debug_id}"))
            .role(Role::Button)
            .aria_label(item.label.clone())
            .aria_selected(active)
            .w_full()
            .min_w_0()
            .gap(px(1.))
            .rounded(px(6.))
            .px_2()
            .py(px(if self.compact_rows { 5. } else { 7. }))
            .when(active, |row| {
                row.bg(cx.theme().accent)
                    .debug_selector(move || format!("sidebar-nav-active-{id}"))
            })
            .when(!item.disabled, |row| {
                row.cursor_pointer()
                    .hover(|style| style.bg(cx.theme().muted))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        cx.emit(event.clone());
                        this.active_item = Some(click_id.clone());
                        cx.notify();
                    }))
            })
            .when(item.disabled, |row| row.opacity(0.5))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .items_center()
                    .gap_1()
                    .when_some(item.icon.clone(), |line, icon| {
                        line.child(
                            Icon::default()
                                .path(icon)
                                .size(px(14.))
                                .text_color(cx.theme().muted_foreground),
                        )
                    })
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_size(label_size)
                            .child(item.label.clone()),
                    )
                    .when_some(item.badge.clone(), |line, badge| {
                        line.child(
                            div()
                                .flex_none()
                                .rounded_full()
                                .bg(cx.theme().muted)
                                .px_1()
                                .text_size(px(9.))
                                .text_color(cx.theme().muted_foreground)
                                .child(badge),
                        )
                    })
                    .when_some(item.trailing.clone(), |line, trailing| {
                        line.child(
                            div()
                                .flex_none()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(meta_size)
                                .text_color(cx.theme().muted_foreground)
                                .child(trailing),
                        )
                    }),
            )
            .when_some(item.detail.clone(), |row, detail| {
                row.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(meta_size)
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                )
            });
        for child in &item.children {
            row = row.child(self.render_item(child, cx));
        }
        let new_session_action = item.new_session_action.clone();
        let context_action = item.context_action.clone();
        if new_session_action.is_some() || context_action.is_some() {
            let weak = cx.weak_entity();
            let nav_id = self.id.clone();
            let item_id = item.id.clone();
            return row
                .context_menu(move |menu, _, _| {
                    let mut menu = menu;
                    if let Some(action) = new_session_action.clone() {
                        let weak = weak.clone();
                        let nav_id = nav_id.clone();
                        let item_id = item_id.clone();
                        menu = menu.item(PopupMenuItem::new(action).on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |_, cx| {
                                cx.emit(SidebarNavEvent::ItemNewSessionRequested {
                                    id: nav_id.clone(),
                                    item_id: item_id.clone(),
                                });
                                cx.notify();
                            });
                        }));
                    }
                    if let Some(action) = context_action.clone() {
                        let weak = weak.clone();
                        let nav_id = nav_id.clone();
                        let item_id = item_id.clone();
                        menu = menu.item(PopupMenuItem::new(action).on_click(move |_, _, cx| {
                            let _ = weak.update(cx, |_, cx| {
                                cx.emit(SidebarNavEvent::ItemActionRequested {
                                    id: nav_id.clone(),
                                    item_id: item_id.clone(),
                                });
                                cx.notify();
                            });
                        }));
                    }
                    menu
                })
                .into_any_element();
        }
        row.into_any_element()
    }
}

impl EventEmitter<SidebarNavEvent> for SidebarNav {}

impl Render for SidebarNav {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = v_flex()
            .id(ElementId::Name(self.id.clone()))
            .role(Role::Navigation)
            .aria_label(self.labels.workspace.clone())
            .w_full()
            .h_full()
            .min_h_0()
            .gap_1()
            .overflow_y_scrollbar();
        let mut any = false;
        for section in &self.sections {
            let items: Vec<_> = section
                .items
                .iter()
                .filter(|item| self.matches(item))
                .collect();
            if items.is_empty() {
                continue;
            }
            any = true;
            if !section.label.is_empty() {
                let expanded = !section.collapsed || !self.query.trim().is_empty();
                let toggle = SidebarNavEvent::SectionToggled {
                    id: self.id.clone(),
                    section_id: section.id.clone(),
                    collapsed: !section.collapsed,
                };
                let toggle_click = toggle.clone();
                let mut header = h_flex()
                    .id(SharedString::from(format!(
                        "sidebar-nav-section-{}",
                        section.id
                    )))
                    .debug_selector({
                        let id = section.id.clone();
                        move || format!("sidebar-nav-section-{id}")
                    })
                    .role(Role::Button)
                    .aria_label(section.label.clone())
                    .aria_expanded(expanded)
                    .focusable()
                    .tab_stop(true)
                    .w_full()
                    .items_center()
                    .gap(px(7.))
                    .rounded(px(6.))
                    .px_2()
                    .pt_2()
                    .pb(px(4.))
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().muted))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        cx.emit(toggle_click.clone());
                    }))
                    .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "enter" || event.keystroke.key == "space" {
                            cx.emit(toggle.clone());
                            cx.stop_propagation();
                        }
                    }))
                    .child(
                        div()
                            .debug_selector({
                                let id = section.id.clone();
                                move || format!("sidebar-nav-section-chevron-{id}")
                            })
                            .flex_none()
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(px(12.))
                                .text_color(cx.theme().muted_foreground),
                            ),
                    );
                if let Some(leading) = &section.leading {
                    header = header.child(
                        div()
                            .debug_selector({
                                let id = section.id.clone();
                                move || format!("sidebar-nav-section-leading-{id}")
                            })
                            .flex_none()
                            .size(px(20.))
                            .rounded(px(5.))
                            .bg(cx.theme().secondary)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(8.))
                            .font_semibold()
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(leading.clone()),
                    );
                }
                header = header.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_size(px(11.))
                        .font_semibold()
                        .child(section.label.clone()),
                );
                if let Some(meta) = &section.meta {
                    header = header.child(
                        div()
                            .debug_selector({
                                let id = section.id.clone();
                                move || format!("sidebar-nav-section-count-{id}")
                            })
                            .flex_none()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(px(9.))
                            .text_color(cx.theme().muted_foreground)
                            .child(meta.clone()),
                    );
                }
                if let Some(action) = &section.action {
                    let action_event = SidebarNavEvent::SectionActionRequested {
                        id: self.id.clone(),
                        section_id: section.id.clone(),
                    };
                    header = header.child(
                        div()
                            .id(SharedString::from(format!(
                                "sidebar-nav-section-action-{}",
                                section.id
                            )))
                            .debug_selector({
                                let id = section.id.clone();
                                move || format!("sidebar-nav-section-action-{id}")
                            })
                            .role(Role::Button)
                            .focusable()
                            .tab_stop(true)
                            .aria_label(action.clone())
                            .cursor_pointer()
                            .rounded(px(4.))
                            .p(px(2.))
                            .hover(|style| style.bg(cx.theme().border))
                            .child(
                                Icon::new(IconName::Close)
                                    .xsmall()
                                    .text_color(cx.theme().muted_foreground),
                            )
                            .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                                cx.emit(action_event.clone());
                                cx.stop_propagation();
                            })),
                    );
                }
                let new_session_action = section.new_session_action.clone();
                let remove_action = section.action.clone();
                let header = if new_session_action.is_some() || remove_action.is_some() {
                    let weak = cx.weak_entity();
                    let nav_id = self.id.clone();
                    let section_id = section.id.clone();
                    header
                        .context_menu(move |menu, _, _| {
                            let mut menu = menu;
                            if let Some(action) = new_session_action.clone() {
                                let weak = weak.clone();
                                let nav_id = nav_id.clone();
                                let section_id = section_id.clone();
                                menu = menu.item(PopupMenuItem::new(action).on_click(
                                    move |_, _, cx| {
                                        let _ = weak.update(cx, |_, cx| {
                                            cx.emit(SidebarNavEvent::SectionNewSessionRequested {
                                                id: nav_id.clone(),
                                                section_id: section_id.clone(),
                                            });
                                            cx.notify();
                                        });
                                    },
                                ));
                            }
                            if let Some(action) = remove_action.clone() {
                                let weak = weak.clone();
                                let nav_id = nav_id.clone();
                                let section_id = section_id.clone();
                                menu = menu.item(PopupMenuItem::new(action).on_click(
                                    move |_, _, cx| {
                                        let _ = weak.update(cx, |_, cx| {
                                            cx.emit(SidebarNavEvent::SectionActionRequested {
                                                id: nav_id.clone(),
                                                section_id: section_id.clone(),
                                            });
                                            cx.notify();
                                        });
                                    },
                                ));
                            }
                            menu
                        })
                        .into_any_element()
                } else {
                    header.into_any_element()
                };
                body = body.child(header);
                if expanded {
                    let children = v_flex().w_full().pl(px(12.));
                    let mut tree = v_flex()
                        .debug_selector({
                            let id = section.id.clone();
                            move || format!("sidebar-nav-section-children-{id}")
                        })
                        .w_full()
                        .pl(px(6.))
                        .border_l(px(1.))
                        .border_color(cx.theme().border);
                    for item in items {
                        tree = tree.child(self.render_item(item, cx));
                    }
                    body = body.child(children.child(tree));
                }
            } else {
                for item in items {
                    body = body.child(self.render_item(item, cx));
                }
            }
        }
        if !any {
            body = body.child(
                div()
                    .px_2()
                    .py_3()
                    .text_size(px(12.))
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.sections.is_empty() {
                        self.labels.no_items.clone()
                    } else {
                        self.labels.no_matches.clone()
                    }),
            );
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{
        AppContext as _, MouseButton, MouseDownEvent, Render, TestAppContext, div, px, size,
    };

    use super::*;

    struct NavProbe {
        nav: gpui::Entity<SidebarNav>,
    }

    struct ContextMenuProbe {
        nav: gpui::Entity<SidebarNav>,
        hits: Rc<Cell<usize>>,
        _subscription: gpui::Subscription,
    }

    impl Render for NavProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(320.)).h(px(400.)).child(self.nav.clone())
        }
    }

    impl Render for ContextMenuProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(320.)).h(px(400.)).child(self.nav.clone())
        }
    }

    #[gpui::test]
    fn clearing_active_item_removes_the_highlight(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (probe, window) = cx.add_window_view(|window, cx| {
            let nav = cx.new(|cx| SidebarNav::new("sidebar-test", window, cx));
            nav.update(cx, |nav, cx| {
                nav.set_sections(
                    [SidebarSection::new("sessions", "Sessions")
                        .items([SidebarNavItem::new("session-one", "Session one")])],
                    cx,
                );
                nav.set_active_item("session-one", cx);
            });
            NavProbe { nav }
        });
        window.simulate_resize(size(px(400.), px(500.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window
                .debug_bounds("sidebar-nav-active-session-one")
                .is_some()
        );
        window.update(|_, cx| {
            let nav = probe.read(cx).nav.clone();
            nav.update(cx, |nav, cx| nav.clear_active_item(cx));
        });
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window
                .debug_bounds("sidebar-nav-active-session-one")
                .is_none(),
            "clearing the active item must remove the stale highlight"
        );
    }

    #[gpui::test]
    fn session_context_menu_emits_archive_action(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (probe, window) =
            cx.add_window_view(|window, cx| {
                let nav = cx.new(|cx| SidebarNav::new("sidebar-context", window, cx));
                nav.update(cx, |nav, cx| {
                    nav.set_sections(
                        [
                            SidebarSection::new("sessions", "Sessions")
                                .items([SidebarNavItem::new("session-one", "Session one")
                                    .context_action("Archive")]),
                        ],
                        cx,
                    );
                });
                let hits = Rc::new(Cell::new(0));
                let hits_for_event = hits.clone();
                let subscription = cx.subscribe(&nav, move |_, _, event: &SidebarNavEvent, _| {
                    if matches!(event, SidebarNavEvent::ItemActionRequested { .. }) {
                        hits_for_event.set(hits_for_event.get() + 1);
                    }
                });
                ContextMenuProbe {
                    nav,
                    hits,
                    _subscription: subscription,
                }
            });
        window.simulate_resize(size(px(400.), px(500.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let row = window
            .debug_bounds("sidebar-nav-item-session-one")
            .expect("session row");
        window.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: row.center(),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.simulate_keystrokes("down enter");
        window.run_until_parked();
        let hits = window.update(|_, cx| probe.read(cx).hits.get());
        assert_eq!(
            hits, 1,
            "right-clicking a session must expose an archive action"
        );
    }

    #[gpui::test]
    fn project_context_menu_emits_new_session_action(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (probe, window) = cx.add_window_view(|window, cx| {
            let nav = cx.new(|cx| SidebarNav::new("sidebar-new-session", window, cx));
            nav.update(cx, |nav, cx| {
                nav.set_sections(
                    [
                        SidebarSection::new("sessions-project-alpha", "Alpha project")
                            .new_session_action("New session")
                            .items([SidebarNavItem::new("session-one", "Session one")]),
                    ],
                    cx,
                );
            });
            let hits = Rc::new(Cell::new(0));
            let hits_for_event = hits.clone();
            let subscription = cx.subscribe(&nav, move |_, _, event: &SidebarNavEvent, _| {
                if matches!(event, SidebarNavEvent::SectionNewSessionRequested { .. }) {
                    hits_for_event.set(hits_for_event.get() + 1);
                }
            });
            ContextMenuProbe {
                nav,
                hits,
                _subscription: subscription,
            }
        });
        window.simulate_resize(size(px(400.), px(500.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let header = window
            .debug_bounds("sidebar-nav-section-sessions-project-alpha")
            .expect("project header");
        window.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: header.center(),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.simulate_keystrokes("down enter");
        window.run_until_parked();
        let hits = window.update(|_, cx| probe.read(cx).hits.get());
        assert_eq!(
            hits, 1,
            "right-clicking a project must expose a new-session action"
        );
    }

    #[gpui::test]
    fn session_context_menu_emits_new_session_action(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (probe, window) = cx.add_window_view(|window, cx| {
            let nav = cx.new(|cx| SidebarNav::new("sidebar-session-new", window, cx));
            nav.update(cx, |nav, cx| {
                nav.set_sections(
                    [
                        SidebarSection::new("sessions", "Sessions").items([SidebarNavItem::new(
                            "session-one",
                            "Session one",
                        )
                        .new_session_action("New session")
                        .context_action("Archive")]),
                    ],
                    cx,
                );
            });
            let hits = Rc::new(Cell::new(0));
            let hits_for_event = hits.clone();
            let subscription = cx.subscribe(&nav, move |_, _, event: &SidebarNavEvent, _| {
                if matches!(event, SidebarNavEvent::ItemNewSessionRequested { .. }) {
                    hits_for_event.set(hits_for_event.get() + 1);
                }
            });
            ContextMenuProbe {
                nav,
                hits,
                _subscription: subscription,
            }
        });
        window.simulate_resize(size(px(400.), px(500.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let row = window
            .debug_bounds("sidebar-nav-item-session-one")
            .expect("session row");
        window.simulate_event(MouseDownEvent {
            button: MouseButton::Right,
            position: row.center(),
            modifiers: Default::default(),
            click_count: 1,
            first_mouse: false,
        });
        window.run_until_parked();
        window.update(|window, cx| window.draw(cx).clear(cx));
        window.simulate_keystrokes("down enter");
        window.run_until_parked();
        let hits = window.update(|_, cx| probe.read(cx).hits.get());
        assert_eq!(
            hits, 1,
            "a session's new-session action must come before archive"
        );
    }
}
