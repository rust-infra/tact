//! Artifact side panel: a generated document, code file, or data view shown
//! beside the conversation.
//!
//! An [`Artifact`] carries the generated source as [`StreamedContent`] (so a
//! panel can render while the agent is still writing), the kind that picks
//! its preview, and the versions the agent has produced. [`ArtifactPanel`]
//! renders the header (kind, title, version switcher, close), a
//! Preview / Source switch, the scrolling body, and an optional action row,
//! reporting every intent as an [`ArtifactPanelEvent`] keyed by stable IDs.
//! Width, docking, and resizing belong to the application — compose the
//! panel inside the upstream resizable group.

use crate::control::ControlMetricsExt as _;
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::scrolling::PolicyScrollbarExt as _;
use crate::surface::CardFrameExt as _;
use crate::{
    ButtonLabelExt as _,
    code_block::CodeBlock,
    handlers::SharedHandler,
    status::{StatusBadge, StatusTone},
    stream::{ProgressState, StreamedContent},
    surface::{icon_button, meta},
    theme::SemanticStyledExt as _,
};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, StyledExt as _,
    button::Button, h_flex, tab::TabBar, text::TextView, v_flex,
};
use std::rc::Rc;

/// What the artifact is, which decides its preview and glyph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ArtifactKind {
    /// Source code; previewed as a highlighted block.
    #[default]
    Code,
    /// Markdown prose; previewed as rendered text.
    Markdown,
    /// HTML; no native preview, so the source is shown.
    Html,
    /// Tabular or structured data; shown as source.
    Data,
    /// Anything else.
    Other,
}

impl ArtifactKind {
    /// The short label for the kind.
    pub fn label(self) -> &'static str {
        match self {
            Self::Code => "Code",
            Self::Markdown => "Document",
            Self::Html => "HTML",
            Self::Data => "Data",
            Self::Other => "Artifact",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Self::Code => IconName::SquareTerminal,
            Self::Markdown => IconName::BookOpen,
            Self::Html => IconName::Globe,
            Self::Data => IconName::ChartPie,
            Self::Other => IconName::File,
        }
    }

    /// Whether the preview view renders something other than the source.
    pub fn has_preview(self) -> bool {
        matches!(self, Self::Markdown | Self::Code)
    }
}

/// Which face of the artifact is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ArtifactView {
    /// The rendered form.
    #[default]
    Preview,
    /// The raw source.
    Source,
}

impl ArtifactView {
    fn index(self) -> usize {
        match self {
            Self::Preview => 0,
            Self::Source => 1,
        }
    }

    fn from_index(index: usize) -> Self {
        if index == 0 {
            Self::Preview
        } else {
            Self::Source
        }
    }
}

/// One produced version of an artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactVersion {
    id: SharedString,
    label: SharedString,
}

impl ArtifactVersion {
    /// Creates a version with a stable identifier and a short label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    /// Returns the stable version identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// An application-defined action offered under the artifact.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArtifactAction {
    id: SharedString,
    label: SharedString,
}

impl ArtifactAction {
    /// Creates an action with a stable identifier and its label.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
        }
    }

    /// Returns the stable action identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible label.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// A generated artifact: title, kind, source, and versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    id: SharedString,
    title: SharedString,
    kind: ArtifactKind,
    language: Option<SharedString>,
    source: StreamedContent,
    versions: Vec<ArtifactVersion>,
    active_version: Option<SharedString>,
}

impl Artifact {
    /// Creates an artifact with its title and source.
    pub fn new(
        id: impl Into<SharedString>,
        title: impl Into<SharedString>,
        source: StreamedContent,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            kind: ArtifactKind::Code,
            language: None,
            source,
            versions: Vec::new(),
            active_version: None,
        }
    }

    /// Sets the kind (default [`ArtifactKind::Code`]).
    pub fn kind(mut self, kind: ArtifactKind) -> Self {
        self.kind = kind;
        self
    }

    /// Sets the language used to highlight the source.
    pub fn language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Sets the produced versions, oldest first.
    pub fn versions(mut self, versions: impl IntoIterator<Item = ArtifactVersion>) -> Self {
        self.versions = versions.into_iter().collect();
        self
    }

    /// Selects the shown version by ID.
    pub fn active_version(mut self, version_id: impl Into<SharedString>) -> Self {
        self.active_version = Some(version_id.into());
        self
    }

    /// Returns the stable artifact identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the title.
    pub fn title(&self) -> &SharedString {
        &self.title
    }

    /// Returns the kind.
    pub fn artifact_kind(&self) -> ArtifactKind {
        self.kind
    }

    /// Returns the source with its lifecycle.
    pub fn source(&self) -> &StreamedContent {
        &self.source
    }

    /// Returns the versions, oldest first.
    pub fn version_refs(&self) -> &[ArtifactVersion] {
        &self.versions
    }

    /// Returns the index of the active version (the last one by default).
    pub fn active_version_index(&self) -> Option<usize> {
        if self.versions.is_empty() {
            return None;
        }
        self.active_version
            .as_ref()
            .and_then(|id| self.versions.iter().position(|version| &version.id == id))
            .or(Some(self.versions.len() - 1))
    }

    /// The accessible name: title, kind, and lifecycle.
    pub fn accessibility_label(&self) -> String {
        let mut label = format!("Artifact: {}, {}", self.title, self.kind.label());
        match self.source.state() {
            ProgressState::Pending | ProgressState::Running => label.push_str(", generating"),
            ProgressState::Failed(reason) => {
                label.push_str(", failed: ");
                label.push_str(reason);
            }
            ProgressState::Complete => {}
        }
        label
    }
}

/// An interaction emitted by [`ArtifactPanel`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArtifactPanelEvent {
    /// The panel's close control was activated.
    Closed {
        /// Stable artifact identifier.
        id: SharedString,
    },
    /// Preview or Source was chosen.
    ViewSelected {
        /// Stable artifact identifier.
        id: SharedString,
        /// The chosen face.
        view: ArtifactView,
    },
    /// Another version was chosen.
    VersionSelected {
        /// Stable artifact identifier.
        id: SharedString,
        /// Stable version identifier.
        version_id: SharedString,
    },
    /// An application action was activated.
    ActionActivated {
        /// Stable artifact identifier.
        id: SharedString,
        /// Stable action identifier.
        action_id: SharedString,
    },
}

/// The side panel for one artifact.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(artifact: Artifact) {
/// ArtifactPanel::new("doc", &artifact)
///     .view(ArtifactView::Preview)
///     .actions([ArtifactAction::new("open", "Open in editor")])
///     .on_event(|event, _, _| { /* ArtifactPanelEvent::Closed { id } … */ });
/// # }
/// ```
#[derive(IntoElement)]
pub struct ArtifactPanel {
    id: ElementId,
    style: StyleRefinement,
    decoration: Decoration,
    artifact: Artifact,
    view: ArtifactView,
    actions: Vec<ArtifactAction>,
    on_event: Option<SharedHandler<ArtifactPanelEvent>>,
}

impl ArtifactPanel {
    /// Layers painted into this panel's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a panel for an artifact, showing its preview.
    pub fn new(id: impl Into<ElementId>, artifact: &Artifact) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            artifact: artifact.clone(),
            view: ArtifactView::Preview,
            actions: Vec::new(),
            on_event: None,
        }
    }

    /// Shows the preview or the source; the switch reports
    /// [`ArtifactPanelEvent::ViewSelected`] so the application can flip this.
    pub fn view(mut self, view: ArtifactView) -> Self {
        self.view = view;
        self
    }

    /// Adds application actions under the body.
    pub fn actions(mut self, actions: impl IntoIterator<Item = ArtifactAction>) -> Self {
        self.actions = actions.into_iter().collect();
        self
    }

    /// Handles typed interactions. Without a handler the panel is static.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ArtifactPanelEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for ArtifactPanel {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ArtifactPanel {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let artifact = self.artifact;
        let handler = self.on_event;
        let artifact_id = artifact.id.clone();
        // Shared by every selector below. A `SharedString` clone is a
        // refcount bump; `to_string` would copy the heap for each one.
        let debug_id = artifact_id.clone();
        let root_id = self.id.clone();
        let label: SharedString = artifact.accessibility_label().into();
        let generating =
            artifact.source.is_streaming() || *artifact.source.state() == ProgressState::Pending;
        let failed = match artifact.source.state() {
            ProgressState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let view = if artifact.kind.has_preview() {
            self.view
        } else {
            ArtifactView::Source
        };

        // Header: kind glyph, title, lifecycle badge, version switcher, close.
        let version_nav = artifact.active_version_index().map(|index| {
            let count = artifact.versions.len();
            let nav_label: SharedString = format!("Version {} of {count}", index + 1).into();
            let prev = (index > 0).then(|| artifact.versions[index - 1].id.clone());
            let next = (index + 1 < count).then(|| artifact.versions[index + 1].id.clone());
            let prev_debug = debug_id.clone();
            let next_debug = debug_id.clone();
            let nav_debug = debug_id.clone();
            let prev_handler = handler.clone();
            let next_handler = handler.clone();
            let prev_artifact = artifact_id.clone();
            let next_artifact = artifact_id.clone();
            h_flex()
                .id((root_id.clone(), "versions"))
                .role(Role::Group)
                .aria_label(nav_label)
                .debug_selector(move || format!("artifact-versions-{nav_debug}"))
                .flex_none()
                .items_center()
                .gap(tokens.spacing.xxs)
                .child(
                    icon_button(
                        (root_id.clone(), "version-prev"),
                        IconName::ChevronLeft,
                        "Previous version",
                        window,
                        cx,
                    )
                    .disabled(prev.is_none() || prev_handler.is_none())
                    .debug_selector(move || format!("artifact-version-prev-{prev_debug}"))
                    .on_click(move |_: &ClickEvent, window, cx| {
                        if let (Some(handler), Some(version_id)) = (&prev_handler, &prev) {
                            handler(
                                &ArtifactPanelEvent::VersionSelected {
                                    id: prev_artifact.clone(),
                                    version_id: version_id.clone(),
                                },
                                window,
                                cx,
                            )
                        }
                    }),
                )
                .child(meta(artifact.versions[index].label.clone(), cx))
                .child(
                    icon_button(
                        (root_id.clone(), "version-next"),
                        IconName::ChevronRight,
                        "Next version",
                        window,
                        cx,
                    )
                    .disabled(next.is_none() || next_handler.is_none())
                    .debug_selector(move || format!("artifact-version-next-{next_debug}"))
                    .on_click(move |_: &ClickEvent, window, cx| {
                        if let (Some(handler), Some(version_id)) = (&next_handler, &next) {
                            handler(
                                &ArtifactPanelEvent::VersionSelected {
                                    id: next_artifact.clone(),
                                    version_id: version_id.clone(),
                                },
                                window,
                                cx,
                            )
                        }
                    }),
                )
        });
        let close = handler.clone().map(|handler| {
            let close_debug = debug_id.clone();
            let close_id = artifact_id.clone();
            icon_button(
                (root_id.clone(), "close"),
                IconName::Close,
                "Close artifact",
                window,
                cx,
            )
            .debug_selector(move || format!("artifact-close-{close_debug}"))
            .on_click(move |_: &ClickEvent, window, cx| {
                handler(
                    &ArtifactPanelEvent::Closed {
                        id: close_id.clone(),
                    },
                    window,
                    cx,
                )
            })
        });
        let generating_debug = debug_id.clone();
        let header = h_flex()
            .items_center()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.md)
            .py(tokens.spacing.sm)
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Icon::new(artifact.kind.icon())
                    .xsmall()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .min_w_0()
                    .flex_1()
                    .child(
                        div()
                            .truncate()
                            .text_token(tokens.typography.sm)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(artifact.title.clone()),
                    )
                    .child(crate::surface::hint(
                        match (&artifact.language, artifact.kind) {
                            (Some(language), ArtifactKind::Code) => {
                                format!("{} · {language}", artifact.kind.label())
                            }
                            _ => artifact.kind.label().to_owned(),
                        },
                        cx,
                    )),
            )
            .when(generating, |this| {
                this.child(
                    div()
                        .debug_selector(move || format!("artifact-generating-{generating_debug}"))
                        .child(
                            StatusBadge::new((root_id.clone(), "generating"), "Generating")
                                .tone(StatusTone::Info)
                                .active(true),
                        ),
                )
            })
            .when_some(failed.clone(), |this, _| {
                this.child(
                    StatusBadge::new((root_id.clone(), "failed"), "Failed")
                        .tone(StatusTone::Danger),
                )
            })
            .children(version_nav)
            .children(close);

        // Preview / Source switch; kinds without a preview pin to Source.
        let tabs = artifact.kind.has_preview().then(|| {
            let tabs_debug = debug_id.clone();
            let tab_artifact = artifact_id.clone();
            let tab_handler = handler.clone();
            h_flex().px(tokens.spacing.md).pt(tokens.spacing.sm).child(
                div()
                    .flex_none()
                    .debug_selector(move || format!("artifact-tabs-{tabs_debug}"))
                    .child(
                        TabBar::new((root_id.clone(), "tabs"))
                            .segmented()
                            .children(["Preview", "Source"])
                            .selected_index(view.index())
                            .on_click(move |index: &usize, window, cx| {
                                if let Some(handler) = &tab_handler {
                                    handler(
                                        &ArtifactPanelEvent::ViewSelected {
                                            id: tab_artifact.clone(),
                                            view: ArtifactView::from_index(*index),
                                        },
                                        window,
                                        cx,
                                    )
                                }
                            }),
                    ),
            )
        });

        let body_id = ElementId::from((root_id.clone(), "body"));
        let content: AnyElement = match view {
            ArtifactView::Preview if artifact.kind == ArtifactKind::Markdown => TextView::markdown(
                (body_id.clone(), "markdown"),
                artifact.source.text().to_owned(),
            )
            .selectable(true)
            .into_any_element(),
            _ => {
                let mut block = CodeBlock::streamed((body_id.clone(), "code"), &artifact.source);
                if let Some(language) = artifact.language.clone() {
                    block = block.language(language);
                } else if artifact.kind == ArtifactKind::Markdown {
                    block = block.language("markdown");
                }
                block.into_any_element()
            }
        };
        let body_debug = debug_id.clone();
        let scroll_handle = window
            .use_keyed_state((root_id.clone(), "scroll"), cx, |_, _| ScrollHandle::new())
            .read(cx)
            .clone();
        let body = div()
            .debug_selector(move || format!("artifact-body-{body_debug}"))
            .flex_1()
            .min_h_0()
            .w_full()
            .policy_vertical_scrollbar(&scroll_handle, cx)
            .child(
                div()
                    .id(body_id)
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&scroll_handle)
                    .child(
                        div()
                            .p(tokens.spacing.md)
                            .text_token(tokens.typography.sm)
                            .child(content),
                    ),
            );

        let footer = (!self.actions.is_empty()).then(|| {
            let mut row = h_flex()
                .flex_wrap()
                .items_center()
                .gap(tokens.spacing.xs)
                .px(tokens.spacing.md)
                .py(tokens.spacing.sm)
                .border_t_1()
                .border_color(cx.theme().border);
            for action in &self.actions {
                let action_debug = action.id.clone();
                let artifact_debug = artifact_id.clone();
                let action_id = action.id.clone();
                let event_artifact = artifact_id.clone();
                let handler = handler.clone();
                row = row.child(
                    div()
                        .debug_selector(move || {
                            format!("artifact-action-{artifact_debug}-{action_debug}")
                        })
                        .child(
                            Button::new((root_id.clone(), format!("action-{}", action.id)))
                                .outline()
                                .small()
                                .control_metrics(cx)
                                .accessibility_id(format!("{artifact_id}-{}", action.id))
                                .text_label(action.label.clone())
                                .disabled(handler.is_none())
                                .on_click(move |_: &ClickEvent, window, cx| {
                                    if let Some(handler) = &handler {
                                        handler(
                                            &ArtifactPanelEvent::ActionActivated {
                                                id: event_artifact.clone(),
                                                action_id: action_id.clone(),
                                            },
                                            window,
                                            cx,
                                        )
                                    }
                                }),
                        ),
                );
            }
            row
        });

        v_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(label)
            .when_some(failed, |this, reason| this.aria_description(reason))
            .debug_selector(move || format!("artifact-{debug_id}"))
            .size_full()
            .min_w_0()
            .min_h_0()
            .card_frame(cx)
            .decoration_under(&mut decoration)
            .overflow_hidden()
            .child(header)
            .children(tabs)
            .child(body)
            .children(footer)
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_active_version_defaults_to_the_latest() {
        let artifact = Artifact::new("doc", "Doc", StreamedContent::done("x")).versions([
            ArtifactVersion::new("v1", "v1"),
            ArtifactVersion::new("v2", "v2"),
        ]);
        assert_eq!(artifact.active_version_index(), Some(1));
        assert_eq!(
            artifact.clone().active_version("v1").active_version_index(),
            Some(0)
        );
        assert_eq!(
            artifact.active_version("missing").active_version_index(),
            Some(1)
        );
        assert_eq!(
            Artifact::new("doc", "Doc", StreamedContent::done("x")).active_version_index(),
            None
        );
    }

    #[test]
    fn accessible_names_carry_kind_and_lifecycle() {
        let running = Artifact::new("doc", "Plan", StreamedContent::running("…".to_owned()))
            .kind(ArtifactKind::Markdown);
        assert_eq!(
            running.accessibility_label(),
            "Artifact: Plan, Document, generating"
        );
        let failed = Artifact::new(
            "doc",
            "Plan",
            StreamedContent::failed(String::new(), "Quota exceeded"),
        );
        assert_eq!(
            failed.accessibility_label(),
            "Artifact: Plan, Code, failed: Quota exceeded"
        );
    }
}
