//! Command palette adapter over `gpui-component`'s native command surface.

use std::sync::Arc;

use gpui::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Role, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon,
    command::{Command, CommandGroup, CommandItem, CommandState},
    h_flex,
    label::Label,
    v_flex,
};

/// One command-palette row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandSearchItem {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    shortcut: Option<SharedString>,
    keywords: Vec<SharedString>,
    icon: Option<SharedString>,
    disabled: bool,
}

impl CommandSearchItem {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            shortcut: None,
            keywords: Vec::new(),
            icon: None,
            disabled: false,
        }
    }

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    pub fn shortcut(mut self, shortcut: impl Into<SharedString>) -> Self {
        self.shortcut = Some(shortcut.into());
        self
    }

    pub fn keywords(mut self, keywords: impl IntoIterator<Item = impl Into<SharedString>>) -> Self {
        self.keywords = keywords.into_iter().map(Into::into).collect();
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    fn accessibility_label(&self) -> SharedString {
        match &self.subtitle {
            Some(subtitle) => format!("{}, {}", self.title, subtitle).into(),
            None => self.title.clone(),
        }
    }
}

/// User-visible copy for the command palette.
#[derive(Clone, Debug, PartialEq, Eq)]
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

/// An interaction emitted by [`CommandSearch`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommandSearchEvent {
    Selected {
        id: SharedString,
        item_id: SharedString,
    },
    QueryChanged {
        id: SharedString,
        query: SharedString,
    },
    Dismissed {
        id: SharedString,
    },
}

/// A controlled command palette with stable item identity.
pub struct CommandSearch {
    id: SharedString,
    items: Arc<[CommandSearchItem]>,
    state: Entity<CommandState>,
    labels: CommandSearchLabels,
    caption: Option<SharedString>,
    last_query: SharedString,
}

impl CommandSearch {
    pub fn new(id: impl Into<SharedString>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            id: id.into(),
            items: Arc::from([]),
            state: cx.new(|cx| CommandState::new(window, cx)),
            labels: CommandSearchLabels::default(),
            caption: None,
            last_query: SharedString::default(),
        }
    }

    pub fn set_labels(&mut self, labels: CommandSearchLabels, cx: &mut Context<Self>) {
        if self.labels != labels {
            self.labels = labels;
            cx.notify();
        }
    }

    pub fn set_caption(&mut self, caption: Option<SharedString>, cx: &mut Context<Self>) {
        if self.caption != caption {
            self.caption = caption;
            cx.notify();
        }
    }

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
        self.items = items;
        cx.notify();
    }

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

    pub fn query(&self, cx: &App) -> SharedString {
        self.state.read(cx).query(cx)
    }

    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.state.update(cx, |state, cx| state.focus(window, cx));
    }

    fn emit_query_changed(&mut self, query: SharedString, cx: &mut Context<Self>) {
        if self.last_query == query {
            return;
        }
        self.last_query = query.clone();
        cx.emit(CommandSearchEvent::QueryChanged {
            id: self.id.clone(),
            query,
        });
    }
}

impl EventEmitter<CommandSearchEvent> for CommandSearch {}

fn row_content(item: CommandSearchItem, cx: &mut App) -> AnyElement {
    let debug_id = item.id.clone();
    let label = item.accessibility_label();
    h_flex()
        .id(ElementId::Name(
            format!("command-search-item-{debug_id}").into(),
        ))
        .debug_selector(move || format!("command-search-item-{debug_id}"))
        .role(Role::Label)
        .aria_label(label)
        .w_full()
        .min_w_0()
        .gap_2()
        .when_some(item.icon, |row, icon| {
            row.child(
                div()
                    .flex_none()
                    .size(px(28.))
                    .rounded(px(8.))
                    .bg(cx.theme().muted)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::default().path(icon).size_4()),
            )
        })
        .child(
            v_flex()
                .min_w_0()
                .flex_1()
                .gap(px(1.))
                .child(Label::new(item.title).text_sm())
                .when_some(item.subtitle, |column, subtitle| {
                    column.child(
                        Label::new(subtitle)
                            .text_xs()
                            .text_color(cx.theme().muted_foreground),
                    )
                }),
        )
        .when_some(item.shortcut, |row, shortcut| {
            row.child(
                div()
                    .flex_none()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(Label::new(shortcut)),
            )
        })
        .into_any_element()
}

fn upstream_item(item: &CommandSearchItem) -> CommandItem {
    let row = item.clone();
    let mut keywords =
        Vec::with_capacity(item.keywords.len() + usize::from(item.subtitle.is_some()));
    if let Some(subtitle) = &item.subtitle {
        keywords.push(subtitle.clone());
    }
    keywords.extend(item.keywords.iter().cloned());
    CommandItem::new()
        .label(item.title.clone())
        .keywords(keywords)
        .child(move |_, cx| row_content(row.clone(), cx))
        .disabled(item.disabled)
}

impl Render for CommandSearch {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let id = self.id.clone();
        let labels = self.labels.clone();
        let caption = self.caption.clone();
        let items = self.items.clone();
        let weak = cx.weak_entity();
        let query_weak = weak.clone();
        let confirm_weak = weak.clone();
        let cancel_weak = weak;
        let empty_id = id.clone();
        let mut command = Command::new(&self.state)
            .placeholder(labels.placeholder.clone())
            .empty(move |state, _, cx| {
                let message = if state.query(cx).is_empty() {
                    labels.empty.clone()
                } else {
                    labels.no_results.clone()
                };
                let empty_id = empty_id.clone();
                div()
                    .debug_selector(move || format!("command-search-empty-{empty_id}"))
                    .p_4()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(message)
                    .into_any_element()
            })
            .on_query(move |query, _, cx| {
                let _ = query_weak.update(cx, |search, cx| {
                    search.emit_query_changed(query.to_string().into(), cx);
                });
            })
            .on_confirm(move |index, _, cx| {
                let _ = confirm_weak.update(cx, |search, cx| {
                    let Some(item) = search.items.get(index.row) else {
                        return;
                    };
                    if item.disabled {
                        return;
                    }
                    cx.emit(CommandSearchEvent::Selected {
                        id: search.id.clone(),
                        item_id: item.id.clone(),
                    });
                });
            })
            .on_cancel(move |_, cx| {
                let _ = cancel_weak.update(cx, |search, cx| {
                    cx.emit(CommandSearchEvent::Dismissed {
                        id: search.id.clone(),
                    });
                });
            });
        command = match caption {
            Some(caption) => command.group(
                CommandGroup::new()
                    .label(caption)
                    .items(items.iter().map(upstream_item)),
            ),
            None => command.items(items.iter().map(upstream_item)),
        };
        div()
            .id(ElementId::Name(id.to_string().into()))
            .debug_selector({
                let id = id.clone();
                move || format!("command-search-{id}")
            })
            .role(Role::Search)
            .aria_label(self.labels.container.clone())
            .w_full()
            .min_w_0()
            .overflow_hidden()
            .child(command)
    }
}
