//! Small leaf surfaces shared by the desktop shell.
//!
//! These replace the gpui-ai leaf components with the same public shape the
//! shell uses, so the shell keeps its own event and identity model while the
//! rendering stays on `gpui-component` primitives.

use std::{rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt as _, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ObjectFit, ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, h_flex, progress::ProgressCircle,
    spinner::Spinner, v_flex,
};

type EventHandler<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;

/// A pixel-grid loader with an optional elapsed readout.
#[derive(IntoElement)]
pub struct LoadingState {
    id: ElementId,
    label: SharedString,
    elapsed: Option<Duration>,
}

impl LoadingState {
    pub fn new() -> Self {
        Self {
            id: ElementId::Name("loading-state".into()),
            label: "Working…".into(),
            elapsed: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }
}

impl Default for LoadingState {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for LoadingState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let primary = cx.theme().primary;
        let grid = div().flex().flex_col().gap(px(2.)).with_animation(
            "loading-grid",
            Animation::new(Duration::from_millis(900)).repeat(),
            move |grid, delta| {
                grid.children((0..3).map(|row| {
                    h_flex().gap(px(2.)).children((0..3).map(move |col| {
                        let phase = (row + col) as f32 / 6.0;
                        let wave = ((delta - phase).rem_euclid(1.0) * 2.0 - 1.0).abs();
                        div()
                            .size(px(4.))
                            .rounded(px(1.))
                            .bg(primary)
                            .opacity(0.15 + 0.85 * wave)
                    }))
                }))
            },
        );
        h_flex()
            .id(self.id)
            .role(Role::ProgressIndicator)
            .aria_label(self.label.clone())
            .items_center()
            .gap_2()
            .child(grid)
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label),
            )
            .when_some(self.elapsed, |row, elapsed| {
                row.child(
                    div()
                        .text_xs()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{:.0}s", elapsed.as_secs_f64())),
                )
            })
    }
}

/// Lifecycle of the tool a chip represents.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolStatus {
    #[default]
    Pending,
    Running,
    Success,
    Failed,
    Cancelled,
    Unknown,
}

/// An interaction emitted by [`ToolChip`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChipEvent {
    Activated { id: SharedString },
}

/// A compact tool-call chip for the run inspector.
#[derive(IntoElement)]
pub struct ToolChip {
    id: SharedString,
    label: SharedString,
    detail: Option<SharedString>,
    status: ToolStatus,
    status_label: Option<SharedString>,
    on_event: Option<EventHandler<ToolChipEvent>>,
}

impl ToolChip {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            status: ToolStatus::Pending,
            status_label: None,
            on_event: None,
        }
    }

    pub fn status(mut self, status: ToolStatus) -> Self {
        self.status = status;
        self
    }

    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn status_label(mut self, label: impl Into<SharedString>) -> Self {
        self.status_label = Some(label.into());
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ToolChipEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    fn status_color(&self, cx: &App) -> gpui::Hsla {
        match self.status {
            ToolStatus::Pending => cx.theme().muted_foreground,
            ToolStatus::Running => cx.theme().info,
            ToolStatus::Success => cx.theme().success,
            ToolStatus::Failed => cx.theme().danger,
            ToolStatus::Cancelled => cx.theme().warning,
            ToolStatus::Unknown => cx.theme().muted_foreground,
        }
    }
}

impl RenderOnce for ToolChip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self.status_color(cx);
        let id = self.id.clone();
        let event = ToolChipEvent::Activated {
            id: self.id.clone(),
        };
        let handler = self.on_event;
        let accessibility = match &self.status_label {
            Some(status) => format!("{}, {}", self.label, status),
            None => self.label.to_string(),
        };
        h_flex()
            .id(self.id)
            .debug_selector(move || format!("tool-chip-{id}"))
            .role(Role::Button)
            .aria_label(accessibility)
            .items_center()
            .gap_1()
            .rounded(px(6.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().secondary)
            .px_2()
            .py(px(2.))
            .when(self.status == ToolStatus::Running, |chip| {
                chip.child(Spinner::new().xsmall().color(color))
            })
            .when(self.status != ToolStatus::Running, |chip| {
                chip.child(div().flex_none().size(px(6.)).rounded_full().bg(color))
            })
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .child(self.label.clone()),
            )
            .when_some(self.detail, |chip, detail| {
                chip.child(
                    div()
                        .flex_none()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                )
            })
            .when_some(handler, |chip, handler| {
                chip.cursor_pointer()
                    .hover(|style| style.bg(cx.theme().muted))
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
            })
    }
}

/// One suggested prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
}

impl Suggestion {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
}

/// An interaction emitted by [`Suggestions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionsEvent {
    Selected { id: SharedString },
}

/// A wrapping row of starter or follow-up prompt chips.
#[derive(IntoElement)]
pub struct Suggestions {
    id: ElementId,
    label: SharedString,
    items: Vec<Suggestion>,
    on_event: Option<EventHandler<SuggestionsEvent>>,
}

impl Suggestions {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: "Suggestions".into(),
            items: Vec::new(),
            on_event: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = Suggestion>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&SuggestionsEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Suggestions {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        let mut row = h_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(self.label)
            .flex_wrap()
            .gap_1();
        for item in self.items {
            let id = item.id.clone();
            let event = SuggestionsEvent::Selected { id: id.clone() };
            let handler = handler.clone();
            let mut chip = div()
                .id(ElementId::Name(format!("suggestion-{id}").into()))
                .debug_selector(move || format!("suggestion-{id}"))
                .role(Role::Button)
                .aria_label(item.label.clone())
                .rounded_full()
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .px_3()
                .py(px(4.))
                .text_xs()
                .child(item.label);
            if let Some(description) = item.description {
                chip = chip.aria_description(description);
            }
            if let Some(handler) = handler {
                chip = chip
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().muted))
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
            }
            row = row.child(chip);
        }
        row
    }
}

/// An interaction emitted by [`ContextCard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextCardEvent {
    Opened { id: SharedString },
}

/// A retrieved context chunk with its source attribution.
#[derive(IntoElement)]
pub struct ContextCard {
    id: SharedString,
    source: SharedString,
    snippet: Option<SharedString>,
    relevance: Option<f32>,
    on_event: Option<EventHandler<ContextCardEvent>>,
}

impl ContextCard {
    pub fn new(id: impl Into<SharedString>, source: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            source: source.into(),
            snippet: None,
            relevance: None,
            on_event: None,
        }
    }

    pub fn snippet(mut self, snippet: impl Into<SharedString>) -> Self {
        self.snippet = Some(snippet.into());
        self
    }

    pub fn relevance(mut self, relevance: f32) -> Self {
        self.relevance = Some(relevance.clamp(0.0, 1.0));
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ContextCardEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ContextCard {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let event = ContextCardEvent::Opened {
            id: self.id.clone(),
        };
        let handler = self.on_event;
        let mut card = v_flex()
            .id(self.id)
            .debug_selector(move || format!("context-card-{id}"))
            .role(Role::Group)
            .aria_label(self.source.clone())
            .w_full()
            .gap_1()
            .rounded(px(8.))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().popover)
            .p_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
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
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(self.source),
                    )
                    .when_some(self.relevance, |row, relevance| {
                        row.child(
                            div()
                                .flex_none()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{:.0}%", relevance * 100.0)),
                        )
                    }),
            )
            .when_some(self.snippet, |card, snippet| {
                card.child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().foreground)
                        .child(snippet),
                )
            });
        if let Some(handler) = handler {
            card = card
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
        }
        card
    }
}

/// Context-window usage and its optional token breakdown.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ContextUsage {
    used: u64,
    limit: u64,
    input: Option<u64>,
    output: Option<u64>,
    reasoning: Option<u64>,
    cached: Option<u64>,
}

impl ContextUsage {
    pub fn new(used: u64, limit: u64) -> Self {
        Self {
            used,
            limit,
            ..Self::default()
        }
    }

    pub fn input(mut self, tokens: u64) -> Self {
        self.input = Some(tokens);
        self
    }

    pub fn output(mut self, tokens: u64) -> Self {
        self.output = Some(tokens);
        self
    }

    pub fn reasoning(mut self, tokens: u64) -> Self {
        self.reasoning = Some(tokens);
        self
    }

    pub fn cached(mut self, tokens: u64) -> Self {
        self.cached = Some(tokens);
        self
    }

    fn ratio(&self) -> f32 {
        if self.limit == 0 {
            0.0
        } else {
            (self.used as f32 / self.limit as f32).clamp(0.0, 1.0)
        }
    }
}

/// A context-window meter with a token breakdown.
#[derive(IntoElement)]
pub struct ContextMeter {
    id: ElementId,
    label: SharedString,
    usage: ContextUsage,
}

impl ContextMeter {
    pub fn new(id: impl Into<ElementId>, usage: &ContextUsage) -> Self {
        Self {
            id: id.into(),
            label: "Context".into(),
            usage: usage.clone(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }
}

impl RenderOnce for ContextMeter {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let ratio = self.usage.ratio();
        let tone = if ratio >= 0.85 {
            cx.theme().danger
        } else if ratio >= 0.65 {
            cx.theme().warning
        } else {
            cx.theme().primary
        };
        let ring = ProgressCircle::new(ElementId::Name("context-usage".into()))
            .value(ratio * 100.0)
            .color(tone)
            .xsmall();
        v_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(self.label.clone())
            .w_full()
            .gap_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(ring)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.label),
                            )
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!("{} / {}", self.usage.used, self.usage.limit)),
                            ),
                    )
                    .child(
                        div()
                            .flex_none()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_xs()
                            .text_color(tone)
                            .child(format!("{:.0}%", ratio * 100.0)),
                    ),
            )
            .when(
                self.usage.input.is_some()
                    || self.usage.output.is_some()
                    || self.usage.reasoning.is_some()
                    || self.usage.cached.is_some(),
                |meter| {
                    meter.child(
                        h_flex()
                            .flex_wrap()
                            .gap_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .when_some(self.usage.input, |row, tokens| {
                                row.child(div().child(format!("in {tokens}")))
                            })
                            .when_some(self.usage.output, |row, tokens| {
                                row.child(div().child(format!("out {tokens}")))
                            })
                            .when_some(self.usage.cached, |row, tokens| {
                                row.child(div().child(format!("cached {tokens}")))
                            })
                            .when_some(self.usage.reasoning, |row, tokens| {
                                row.child(div().child(format!("reasoning {tokens}")))
                            }),
                    )
                },
            )
    }
}

/// The visual shape of a [`Toggle`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToggleShape {
    #[default]
    Check,
    Switch,
}

/// An interaction emitted by [`Toggle`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToggleEvent {
    Toggled { id: SharedString, on: bool },
}

/// A labelled on/off control.
#[derive(IntoElement)]
pub struct Toggle {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
    shape: ToggleShape,
    on: bool,
    disabled: bool,
    on_event: Option<EventHandler<ToggleEvent>>,
}

impl Toggle {
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
            shape: ToggleShape::Check,
            on: false,
            disabled: false,
            on_event: None,
        }
    }

    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn shape(mut self, shape: ToggleShape) -> Self {
        self.shape = shape;
        self
    }

    pub fn on(mut self, on: bool) -> Self {
        self.on = on;
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ToggleEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for Toggle {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let event = ToggleEvent::Toggled {
            id: self.id.clone(),
            on: !self.on,
        };
        let handler = self.on_event.filter(|_| !self.disabled);
        let role = match self.shape {
            ToggleShape::Check => Role::CheckBox,
            ToggleShape::Switch => Role::Switch,
        };
        let indicator = match self.shape {
            ToggleShape::Check => div()
                .flex_none()
                .size(px(14.))
                .rounded(px(3.))
                .border_1()
                .border_color(if self.on {
                    cx.theme().primary
                } else {
                    cx.theme().border
                })
                .bg(if self.on {
                    cx.theme().primary
                } else {
                    cx.theme().popover
                })
                .when(self.on, |box_| {
                    box_.flex().items_center().justify_center().child(
                        Icon::new(IconName::Check)
                            .xsmall()
                            .text_color(cx.theme().primary_foreground),
                    )
                }),
            ToggleShape::Switch => div()
                .flex_none()
                .h(px(16.))
                .w(px(28.))
                .rounded_full()
                .bg(if self.on {
                    cx.theme().primary
                } else {
                    cx.theme().muted
                })
                .p(px(2.))
                .child(
                    div()
                        .size(px(12.))
                        .rounded_full()
                        .bg(cx.theme().popover)
                        .when(self.on, |dot| dot.ml(px(12.))),
                ),
        };
        let mut row = h_flex()
            .id(self.id)
            .debug_selector(move || format!("toggle-{id}"))
            .role(role)
            .aria_label(self.label.clone())
            .aria_toggled(self.on.into())
            .w_full()
            .items_start()
            .gap_2()
            .px_2()
            .py_1()
            .child(indicator)
            .child(
                v_flex()
                    .min_w_0()
                    .child(div().text_sm().child(self.label))
                    .when_some(self.description, |body, description| {
                        body.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(description),
                        )
                    }),
            );
        if let Some(handler) = handler {
            row = row
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
        }
        row.when(self.disabled, |row| row.opacity(0.5))
    }
}

/// One file attached to a prompt or message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttachmentItem {
    id: SharedString,
    name: SharedString,
    image: Option<std::path::PathBuf>,
    size_bytes: Option<u64>,
}

impl AttachmentItem {
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            image: None,
            size_bytes: None,
        }
    }

    pub fn size_bytes(mut self, size_bytes: u64) -> Self {
        self.size_bytes = Some(size_bytes);
        self
    }

    /// The source used to render an image thumbnail instead of the file name.
    pub fn image(mut self, source: impl Into<std::path::PathBuf>) -> Self {
        self.image = Some(source.into());
        self
    }

    pub fn id(&self) -> &SharedString {
        &self.id
    }

    pub fn name(&self) -> &SharedString {
        &self.name
    }

    pub fn image_source(&self) -> Option<&std::path::PathBuf> {
        self.image.as_ref()
    }
}

pub fn is_image_path(path: &std::path::Path) -> bool {
    matches!(
        path.extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.to_ascii_lowercase())
            .as_deref(),
        Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "heic")
    )
}

/// An interaction emitted by [`AttachmentStrip`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachmentEvent {
    Opened { id: SharedString },
    Removed { id: SharedString },
}

/// A wrapping strip of attachment chips.
#[derive(IntoElement)]
pub struct AttachmentStrip {
    id: ElementId,
    label: SharedString,
    items: Vec<AttachmentItem>,
    removable: bool,
    on_event: Option<EventHandler<AttachmentEvent>>,
}

impl AttachmentStrip {
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            label: "Attachments".into(),
            items: Vec::new(),
            removable: false,
            on_event: None,
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    pub fn items(mut self, items: impl IntoIterator<Item = AttachmentItem>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    pub fn removable(mut self, removable: bool) -> Self {
        self.removable = removable;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&AttachmentEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for AttachmentStrip {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        let mut strip = h_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(self.label)
            .flex_wrap()
            .gap_1();
        for item in self.items {
            let id = item.id.clone();
            let image = item.image.clone();
            let event = if self.removable {
                AttachmentEvent::Removed { id: id.clone() }
            } else {
                AttachmentEvent::Opened { id: id.clone() }
            };
            let handler = handler.clone();
            let mut chip = h_flex()
                .id(ElementId::Name(format!("attachment-{id}").into()))
                .debug_selector(move || format!("attachment-{id}"))
                .role(Role::Button)
                .aria_label(item.name.clone())
                .items_center()
                .gap_1()
                .rounded(px(6.))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .when(image.is_some(), |chip| chip.p(px(2.)))
                .when(image.is_none(), |chip| chip.px_2().py(px(2.)));
            chip = if let Some(source) = image {
                chip.child(
                    div()
                        .flex_none()
                        .size(px(32.))
                        .overflow_hidden()
                        .rounded(px(4.))
                        .child(img(source).size_full().object_fit(ObjectFit::Cover)),
                )
            } else {
                chip.child(
                    Icon::new(IconName::File)
                        .xsmall()
                        .text_color(cx.theme().muted_foreground),
                )
                .child(div().text_xs().child(item.name.clone()))
            };
            chip = chip.when_some(item.size_bytes, |chip, size| {
                chip.child(
                    div()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format_bytes(size)),
                )
            });
            if let Some(handler) = handler {
                chip = chip
                    .cursor_pointer()
                    .hover(|style| style.bg(cx.theme().muted))
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
            }
            strip = strip.child(chip);
        }
        strip
    }
}

/// A small three-orb activity signal.
#[derive(IntoElement)]
pub struct Orbs {
    id: ElementId,
    diameter: gpui::Pixels,
}

impl Orbs {
    pub fn new() -> Self {
        Self {
            id: ElementId::Name("orbs".into()),
            diameter: px(24.),
        }
    }

    pub fn diameter(mut self, diameter: gpui::Pixels) -> Self {
        self.diameter = diameter;
        self
    }
}

impl Default for Orbs {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for Orbs {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let dot = (self.diameter / 5.0).max(px(3.));
        h_flex()
            .id(self.id)
            .role(Role::Status)
            .aria_label("Activity")
            .items_center()
            .gap(px(2.))
            .children((0..3).map(|index| {
                div()
                    .flex_none()
                    .size(dot)
                    .rounded_full()
                    .bg(cx.theme().primary)
                    .with_animation(
                        ("orb", index as u64),
                        Animation::new(Duration::from_millis(900)).repeat(),
                        move |dot, delta| {
                            let phase = (delta + index as f32 / 3.0).rem_euclid(1.0);
                            dot.opacity(0.25 + 0.75 * (1.0 - (phase * 2.0 - 1.0).abs()))
                        },
                    )
            }))
    }
}

fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    let bytes = bytes as f64;
    if bytes >= MIB {
        format!("{:.1} MB", bytes / MIB)
    } else if bytes >= KIB {
        format!("{:.0} KB", bytes / KIB)
    } else {
        format!("{bytes:.0} B")
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::is_image_path;

    #[test]
    fn image_attachments_are_detected_by_extension() {
        assert!(is_image_path(Path::new("shot.PNG")));
        assert!(is_image_path(Path::new("a/b/photo.jpeg")));
        assert!(is_image_path(Path::new("anim.webp")));
        assert!(!is_image_path(Path::new("notes.md")));
        assert!(!is_image_path(Path::new("Makefile")));
    }
}
