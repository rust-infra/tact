//! Reader surfaces that used to come from gpui-ai.
//!
//! These are presentation components over Tact's existing ANSI, diff and
//! Markdown helpers; the application still owns the data and the decisions.

use std::rc::Rc;

use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex,
    scroll::ScrollableElement as _, text::TextView, v_flex,
};

use crate::ui::diff_review;

type EventHandler<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;

/// A syntax-labelled code block.
#[derive(IntoElement)]
pub struct CodeBlock {
    id: SharedString,
    code: SharedString,
    language: Option<SharedString>,
}

impl CodeBlock {
    pub fn new(id: impl Into<SharedString>, code: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            code: code.into(),
            language: None,
        }
    }

    pub fn language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }
}

impl RenderOnce for CodeBlock {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let language = self.language.clone();
        let body = match &language {
            Some(language) => format!("```{language}\n{}\n```", self.code),
            None => format!("```\n{}\n```", self.code),
        };
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .debug_selector({
                let id = self.id.clone();
                move || format!("code-block-{id}")
            })
            .w_full()
            .min_w_0()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .overflow_hidden()
            .when_some(language, |block, language| {
                block.child(
                    h_flex()
                        .px_2()
                        .py_1()
                        .border_b_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(language),
                        ),
                )
            })
            .child(div().w_full().min_w_0().p_2().child(TextView::markdown(
                ElementId::Name(format!("{}-body", self.id).into()),
                body,
            )))
    }
}

/// One file parsed from a unified diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiffFile {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    patch: String,
}

impl DiffFile {
    pub fn from_unified(patch: &str) -> Vec<Self> {
        let files = diff_review::files(patch);
        if files.is_empty() {
            return Vec::new();
        }
        files
            .into_iter()
            .map(|file| Self {
                path: file.path,
                added: file.added,
                removed: file.removed,
                patch: patch.to_string(),
            })
            .collect()
    }
}

/// A single-file unified diff.
#[derive(IntoElement)]
pub struct CodeDiff {
    id: SharedString,
    file: DiffFile,
    open: bool,
}

impl CodeDiff {
    pub fn new(id: impl Into<SharedString>, file: &DiffFile) -> Self {
        Self {
            id: id.into(),
            file: file.clone(),
            open: true,
        }
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }
}

impl RenderOnce for CodeDiff {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let mut body = v_flex()
            .id(ElementId::Name(self.id.clone()))
            .debug_selector({
                let path = self.file.path.clone();
                move || format!("code-diff-{path}")
            })
            .w_full()
            .min_w_0()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .overflow_hidden()
            .child(
                h_flex()
                    .px_2()
                    .py_1()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .child(self.file.path.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().success)
                            .child(format!("+{}", self.file.added)),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().danger)
                            .child(format!("−{}", self.file.removed)),
                    ),
            );
        if self.open {
            for line in self.file.patch.lines() {
                let (text, color, background) = if line.starts_with("+++")
                    || line.starts_with("---")
                    || line.starts_with("diff --git")
                    || line.starts_with("index ")
                {
                    (line, cx.theme().muted_foreground, None)
                } else if line.starts_with('+') {
                    (
                        line,
                        cx.theme().success,
                        Some(cx.theme().success.opacity(0.08)),
                    )
                } else if line.starts_with('-') {
                    (
                        line,
                        cx.theme().danger,
                        Some(cx.theme().danger.opacity(0.08)),
                    )
                } else if line.starts_with("@@") {
                    (line, cx.theme().info, Some(cx.theme().info.opacity(0.08)))
                } else {
                    (line, cx.theme().foreground, None)
                };
                body = body.child(
                    div()
                        .w_full()
                        .min_w_0()
                        .px_2()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_size(px(11.))
                        .text_color(color)
                        .when_some(background, |row, background| row.bg(background))
                        .child(text.to_string()),
                );
            }
        }
        body
    }
}

/// One web-search hit.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchResult {
    id: SharedString,
    title: SharedString,
    domain: Option<SharedString>,
}

impl SearchResult {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            domain: None,
        }
    }

    pub fn domain(mut self, domain: impl Into<SharedString>) -> Self {
        self.domain = Some(domain.into());
        self
    }
}

/// An interaction emitted by [`SearchResults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchResultsEvent {
    Opened { id: SharedString },
}

/// The query and hits of a web-search tool call.
#[derive(IntoElement)]
pub struct SearchResults {
    id: SharedString,
    query: SharedString,
    results: Vec<SearchResult>,
    on_event: Option<EventHandler<SearchResultsEvent>>,
}

impl SearchResults {
    pub fn new(id: impl Into<SharedString>, query: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            query: query.into(),
            results: Vec::new(),
            on_event: None,
        }
    }

    pub fn results(mut self, results: impl IntoIterator<Item = SearchResult>) -> Self {
        self.results = results.into_iter().collect();
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&SearchResultsEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SearchResults {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        let mut list = v_flex()
            .id(ElementId::Name(self.id.clone()))
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .child(self.query.clone()),
                    ),
            );
        for result in self.results {
            let id = result.id.clone();
            let event = SearchResultsEvent::Opened { id: id.clone() };
            let handler = handler.clone();
            let mut row = h_flex()
                .id(ElementId::Name(format!("search-result-{id}").into()))
                .role(Role::Button)
                .aria_label(result.title.clone())
                .w_full()
                .items_center()
                .gap_2()
                .rounded(px(6.))
                .px_2()
                .py_1()
                .hover(|style| style.bg(cx.theme().muted))
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .child(result.title),
                )
                .when_some(result.domain, |row, domain| {
                    row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(domain),
                    )
                });
            if let Some(handler) = handler {
                row = row
                    .cursor_pointer()
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
            }
            list = list.child(row);
        }
        list
    }
}

/// What kind of artifact a panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArtifactKind {
    #[default]
    Code,
    Markdown,
    Html,
    Data,
    Other,
}

impl ArtifactKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Markdown => "Document",
            Self::Html => "HTML",
            Self::Data => "Data",
            Self::Other => "Artifact",
        }
    }
}

/// The face an artifact panel shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ArtifactView {
    #[default]
    Preview,
    Source,
}

/// A generated artifact.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Artifact {
    id: SharedString,
    title: SharedString,
    kind: ArtifactKind,
    language: Option<SharedString>,
    source: SharedString,
}

impl Artifact {
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        source: impl Into<SharedString>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind: ArtifactKind::Code,
            language: None,
            source: source.into(),
        }
    }

    pub fn kind(mut self, kind: ArtifactKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }
}

/// An interaction emitted by [`ArtifactPanel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactPanelEvent {
    Closed {
        id: SharedString,
    },
    ViewSelected {
        id: SharedString,
        view: ArtifactView,
    },
    VersionSelected {
        id: SharedString,
        version_id: SharedString,
    },
    ActionActivated {
        id: SharedString,
        action_id: SharedString,
    },
}

/// A preview/source panel for one artifact.
#[derive(IntoElement)]
pub struct ArtifactPanel {
    id: SharedString,
    artifact: Artifact,
    view: ArtifactView,
    on_event: Option<EventHandler<ArtifactPanelEvent>>,
}

impl ArtifactPanel {
    pub fn new(id: impl Into<SharedString>, artifact: &Artifact) -> Self {
        Self {
            id: id.into(),
            artifact: artifact.clone(),
            view: ArtifactView::Preview,
            on_event: None,
        }
    }

    pub fn view(mut self, view: ArtifactView) -> Self {
        self.view = view;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ArtifactPanelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ArtifactPanel {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        let id = self.artifact.id.clone();
        let mut tabs = h_flex().gap_1();
        for view in [ArtifactView::Preview, ArtifactView::Source] {
            let selected = self.view == view;
            let event = ArtifactPanelEvent::ViewSelected {
                id: id.clone(),
                view,
            };
            let handler = handler.clone();
            let label = match view {
                ArtifactView::Preview => "Preview",
                ArtifactView::Source => "Source",
            };
            let mut tab = div()
                .id(ElementId::Name(format!("artifact-tab-{id}-{label}").into()))
                .role(Role::Button)
                .aria_label(label)
                .rounded(px(5.))
                .px_2()
                .py_1()
                .text_xs()
                .when(selected, |tab| {
                    tab.bg(cx.theme().accent).text_color(cx.theme().foreground)
                })
                .when(!selected, |tab| tab.text_color(cx.theme().muted_foreground))
                .child(label);
            if let Some(handler) = handler {
                tab = tab
                    .cursor_pointer()
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
            }
            tabs = tabs.child(tab);
        }
        let body = match self.view {
            ArtifactView::Preview => match self.artifact.kind {
                ArtifactKind::Markdown => TextView::markdown(
                    ElementId::Name(format!("{}-preview", self.id).into()),
                    self.artifact.source.clone(),
                ),
                ArtifactKind::Code => {
                    let language = self
                        .artifact
                        .language
                        .as_deref()
                        .unwrap_or_default()
                        .to_string();
                    TextView::markdown(
                        ElementId::Name(format!("{}-preview", self.id).into()),
                        format!("```{language}\n{}\n```", self.artifact.source),
                    )
                }
                _ => TextView::markdown(
                    ElementId::Name(format!("{}-preview", self.id).into()),
                    self.artifact.source.clone(),
                ),
            },
            ArtifactView::Source => TextView::markdown(
                ElementId::Name(format!("{}-source", self.id).into()),
                format!("```\n{}\n```", self.artifact.source),
            ),
        };
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .debug_selector({
                let id = self.artifact.id.clone();
                move || format!("artifact-{id}")
            })
            .w_full()
            .h_full()
            .min_h_0()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .overflow_hidden()
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(IconName::File)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .truncate()
                            .text_sm()
                            .child(self.artifact.title.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.artifact.kind.label()),
                    )
                    .child(tabs),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p_3()
                    .child(body),
            )
    }
}

/// A compact analytical card with a title and body.
#[derive(IntoElement)]
pub struct InsightCard {
    id: SharedString,
    title: SharedString,
    body: Option<SharedString>,
}

impl InsightCard {
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            body: None,
        }
    }

    pub fn body(mut self, body: impl Into<SharedString>) -> Self {
        self.body = Some(body.into());
        self
    }
}

impl RenderOnce for InsightCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .w_full()
            .gap_2()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .p_3()
            .child(div().text_sm().font_semibold().child(self.title))
            .when_some(self.body, |card, body| {
                card.child(TextView::markdown(
                    ElementId::Name(format!("{}-body", self.id).into()),
                    body,
                ))
            })
    }
}

/// One prompt waiting in the queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueuedMessage {
    id: SharedString,
    text: SharedString,
    note: Option<SharedString>,
}

impl QueuedMessage {
    pub fn new(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            note: None,
        }
    }

    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }
}

/// An interaction emitted by [`MessageQueue`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum QueueEvent {
    Removed { id: SharedString },
    SentNow { id: SharedString },
    MovedUp { id: SharedString },
    MovedDown { id: SharedString },
    EditRequested { id: SharedString },
    Cleared,
}

/// User-visible copy for queued prompts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MessageQueueLabels {
    pub clear: SharedString,
    pub queued: SharedString,
    pub waiting: SharedString,
    pub queued_item: SharedString,
    pub of: SharedString,
    pub move_earlier: SharedString,
    pub move_later: SharedString,
    pub edit: SharedString,
    pub send_now: SharedString,
    pub remove: SharedString,
}

impl Default for MessageQueueLabels {
    fn default() -> Self {
        Self {
            clear: "Clear queue".into(),
            queued: "Queued".into(),
            waiting: "waiting".into(),
            queued_item: "Queued".into(),
            of: "of".into(),
            move_earlier: "Move earlier".into(),
            move_later: "Move later".into(),
            edit: "Edit".into(),
            send_now: "Send now".into(),
            remove: "Remove".into(),
        }
    }
}

/// The prompts waiting behind the active turn.
#[derive(IntoElement)]
pub struct MessageQueue {
    id: SharedString,
    items: Vec<QueuedMessage>,
    editable: bool,
    labels: MessageQueueLabels,
    on_event: Option<EventHandler<QueueEvent>>,
}

impl MessageQueue {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            items: Vec::new(),
            editable: false,
            labels: MessageQueueLabels::default(),
            on_event: None,
        }
    }

    pub fn items(mut self, items: impl IntoIterator<Item = QueuedMessage>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    pub fn labels(mut self, labels: MessageQueueLabels) -> Self {
        self.labels = labels;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&QueueEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

fn queue_action(
    id: impl Into<ElementId>,
    label: &'static str,
    event: QueueEvent,
    handler: &Option<EventHandler<QueueEvent>>,
    cx: &mut App,
) -> AnyElement {
    let mut control = div()
        .id(id)
        .flex_none()
        .rounded(px(4.))
        .px_1()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .hover(|style| style.bg(cx.theme().muted))
        .child(label);
    if let Some(handler) = handler.clone() {
        control = control
            .cursor_pointer()
            .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
    }
    control.into_any_element()
}

impl RenderOnce for MessageQueue {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        let count = self.items.len();
        let clear = queue_action(
            ElementId::Name(format!("queue-clear-{}", self.id).into()),
            "×",
            QueueEvent::Cleared,
            &handler,
            cx,
        );
        let mut list = v_flex()
            .id(ElementId::Name(self.id.clone()))
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .child(
                        div()
                            .flex_1()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "{} · {count} {}",
                                self.labels.queued, self.labels.waiting
                            )),
                    )
                    .child(clear),
            );
        for item in self.items {
            let id = item.id.clone();
            let mut row = h_flex()
                .id(ElementId::Name(
                    format!("queue-item-{}-{id}", self.id).into(),
                ))
                .debug_selector({
                    let queue = self.id.clone();
                    let row = id.clone();
                    move || format!("queue-item-{queue}-{row}")
                })
                .role(Role::ListItem)
                .aria_label(item.text.clone())
                .w_full()
                .items_center()
                .gap_1()
                .rounded(px(6.))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .px_2()
                .py_1()
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .child(item.text),
                )
                .when_some(item.note, |row, note| {
                    row.child(
                        div()
                            .flex_none()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(note),
                    )
                });
            if self.editable {
                row = row
                    .child(queue_action(
                        ElementId::Name(format!("queue-up-{}-{id}", self.id).into()),
                        "↑",
                        QueueEvent::MovedUp { id: id.clone() },
                        &handler,
                        cx,
                    ))
                    .child(queue_action(
                        ElementId::Name(format!("queue-down-{}-{id}", self.id).into()),
                        "↓",
                        QueueEvent::MovedDown { id: id.clone() },
                        &handler,
                        cx,
                    ))
                    .child(queue_action(
                        ElementId::Name(format!("queue-edit-{}-{id}", self.id).into()),
                        "✎",
                        QueueEvent::EditRequested { id: id.clone() },
                        &handler,
                        cx,
                    ))
                    .child(queue_action(
                        ElementId::Name(format!("queue-send-{}-{id}", self.id).into()),
                        "↵",
                        QueueEvent::SentNow { id: id.clone() },
                        &handler,
                        cx,
                    ))
                    .child(queue_action(
                        ElementId::Name(format!("queue-remove-{}-{id}", self.id).into()),
                        "×",
                        QueueEvent::Removed { id: id.clone() },
                        &handler,
                        cx,
                    ));
            }
            list = list.child(row);
        }
        list
    }
}
