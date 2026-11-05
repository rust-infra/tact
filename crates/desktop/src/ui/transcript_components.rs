//! Transcript primitives: streamed answers, reasoning traces and tool calls.
//!
//! These are local, presentation-only replacements for the gpui-ai transcript
//! components. The shell still owns the state and the expansion decisions.

use std::{rc::Rc, time::Duration};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, ClickEvent, ElementId, InteractiveElement as _,
    IntoElement, ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    Styled, Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, text::TextView, v_flex,
};

use tact_desktop_core::RowStatus;

type EventHandler<E> = Rc<dyn Fn(&E, &mut Window, &mut App)>;

/// Inline tool bodies are previews. The reader owns the full output.
const TOOL_BODY_MAX_HEIGHT: f32 = 300.;

/// The lifecycle of streamed content.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StreamState {
    Pending,
    Running,
    Complete,
    Cancelled,
    Unknown,
    Failed(String),
}

/// Markdown that may still be arriving.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamedContent {
    text: String,
    state: StreamState,
}

impl StreamedContent {
    pub fn done(text: String) -> Self {
        Self {
            text,
            state: StreamState::Complete,
        }
    }

    pub fn pending(text: String) -> Self {
        Self {
            text,
            state: StreamState::Pending,
        }
    }

    pub fn running(text: String) -> Self {
        Self {
            text,
            state: StreamState::Running,
        }
    }

    pub fn unknown(text: String) -> Self {
        Self {
            text,
            state: StreamState::Unknown,
        }
    }

    pub fn cancelled(text: String) -> Self {
        Self {
            text,
            state: StreamState::Cancelled,
        }
    }

    pub fn failed(text: String, reason: impl Into<String>) -> Self {
        Self {
            text,
            state: StreamState::Failed(reason.into()),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn state(&self) -> &StreamState {
        &self.state
    }
}

/// A streamed Markdown answer.
#[derive(IntoElement)]
pub struct StreamingText {
    id: SharedString,
    content: StreamedContent,
    label: SharedString,
}

impl StreamingText {
    pub fn new(id: impl Into<SharedString>, content: &StreamedContent) -> Self {
        Self {
            id: id.into(),
            content: content.clone(),
            label: "Assistant response".into(),
        }
    }

    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }
}

impl RenderOnce for StreamingText {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let running = matches!(self.content.state(), StreamState::Running);
        let failure = match self.content.state() {
            StreamState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .debug_selector(|| "streaming-text-body".into())
            .role(Role::Article)
            .aria_label(self.label)
            .w_full()
            .min_w_0()
            .gap_1()
            .child(
                TextView::markdown(
                    ElementId::Name(format!("{}-body", self.id).into()),
                    self.content.text().to_string(),
                )
                .selectable(true),
            )
            .when(running, |column| {
                let primary = cx.theme().primary;
                column.child(
                    div()
                        .w(px(7.))
                        .h(px(14.))
                        .rounded(px(2.))
                        .bg(primary)
                        .with_animation(
                            "streaming-cursor",
                            Animation::new(Duration::from_millis(800)).repeat(),
                            |cursor, delta| cursor.opacity(0.2 + 0.8 * delta),
                        ),
                )
            })
            .when_some(failure, |column, reason| {
                column.child(div().text_xs().text_color(cx.theme().danger).child(reason))
            })
    }
}

/// User-visible copy for a reasoning trace.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThinkingLabels {
    pub thinking: SharedString,
    pub stopped: SharedString,
    pub thought_for_prefix: SharedString,
    pub seconds_suffix: SharedString,
    pub thoughts: SharedString,
    pub step_running: SharedString,
    pub step_done: SharedString,
}

impl Default for ThinkingLabels {
    fn default() -> Self {
        Self {
            thinking: "Thinking…".into(),
            stopped: "Thinking stopped".into(),
            thought_for_prefix: "Thought for ".into(),
            seconds_suffix: "s".into(),
            thoughts: "Thoughts".into(),
            step_running: "in progress".into(),
            step_done: "complete".into(),
        }
    }
}

/// An interaction emitted by [`Thinking`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ThinkingEvent {
    Toggled { id: SharedString, open: bool },
}

/// An expandable reasoning trace.
#[derive(IntoElement)]
pub struct Thinking {
    id: SharedString,
    markdown: SharedString,
    status: RowStatus,
    duration_ms: Option<u64>,
    labels: ThinkingLabels,
    open: bool,
    on_event: Option<EventHandler<ThinkingEvent>>,
}

impl Thinking {
    pub fn new(id: impl Into<SharedString>, markdown: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            markdown: markdown.into(),
            status: RowStatus::Success,
            duration_ms: None,
            labels: ThinkingLabels::default(),
            open: false,
            on_event: None,
        }
    }

    pub fn status(mut self, status: RowStatus) -> Self {
        self.status = status;
        self
    }

    pub fn duration_ms(mut self, duration_ms: Option<u64>) -> Self {
        self.duration_ms = duration_ms;
        self
    }

    pub fn labels(mut self, labels: ThinkingLabels) -> Self {
        self.labels = labels;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ThinkingEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    fn title(&self) -> SharedString {
        match self.status {
            RowStatus::Running | RowStatus::Pending => self.labels.thinking.clone(),
            RowStatus::Cancelled => self.labels.stopped.clone(),
            _ => match self.duration_ms {
                Some(duration) => format!(
                    "{}{:.0}{}",
                    self.labels.thought_for_prefix,
                    duration as f64 / 1000.0,
                    self.labels.seconds_suffix
                )
                .into(),
                None => self.labels.thoughts.clone(),
            },
        }
    }
}

impl RenderOnce for Thinking {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let event = ThinkingEvent::Toggled {
            id: self.id.clone(),
            open: !self.open,
        };
        let handler = self.on_event.clone();
        let title = self.title();
        let active = matches!(self.status, RowStatus::Running | RowStatus::Pending);
        let compact = !self.open && !active;
        let mut header = h_flex()
            .id(ElementId::Name(format!("thinking-toggle-{id}").into()))
            .debug_selector(move || format!("thinking-toggle-{id}"))
            .role(Role::Button)
            .aria_label(title.clone())
            .aria_expanded(self.open)
            .w_full()
            .items_center()
            .gap_2()
            .py(px(if compact { 2. } else { 4. }))
            .child(
                Icon::new(if self.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_size(px(if compact { 12. } else { 13. }))
                    .text_color(cx.theme().muted_foreground)
                    .child(title),
            );
        if let Some(handler) = handler {
            header = header
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
        }
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .w_full()
            .min_w_0()
            .child(header)
            .when(self.open, |column| {
                column.child(
                    div()
                        .debug_selector({
                            let id = self.id.clone();
                            move || format!("thinking-body-{id}")
                        })
                        .w_full()
                        .min_w_0()
                        .pl(px(20.))
                        .py_1()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(
                            TextView::markdown(
                                ElementId::Name(format!("{}-body", self.id).into()),
                                self.markdown,
                            )
                            .selectable(true),
                        ),
                )
            })
    }
}

/// User-visible copy for a tool call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolCallLabels {
    pub input: SharedString,
    pub output: SharedString,
    pub approval: SharedString,
    pub waiting: SharedString,
    pub allow: SharedString,
    pub deny: SharedString,
    pub needs_approval: SharedString,
    pub denied: SharedString,
    pub awaiting_approval: SharedString,
    pub pending: SharedString,
    pub running: SharedString,
    pub completed: SharedString,
    pub failed: SharedString,
    pub cancelled: SharedString,
    pub unknown: SharedString,
}

impl Default for ToolCallLabels {
    fn default() -> Self {
        Self {
            input: "Input".into(),
            output: "Output".into(),
            approval: "Approval".into(),
            waiting: "This call is waiting for your decision".into(),
            allow: "Allow".into(),
            deny: "Deny".into(),
            needs_approval: "Needs approval".into(),
            denied: "Denied".into(),
            awaiting_approval: "awaiting approval".into(),
            pending: "Pending".into(),
            running: "Running".into(),
            completed: "Completed".into(),
            failed: "Failed".into(),
            cancelled: "Cancelled".into(),
            unknown: "Unknown".into(),
        }
    }
}

/// An interaction emitted by [`ToolCall`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolCallEvent {
    Toggled { id: SharedString, open: bool },
    Approved { id: SharedString },
    Rejected { id: SharedString },
}

/// One collapsible tool invocation.
#[derive(IntoElement)]
pub struct ToolCall {
    id: SharedString,
    name: SharedString,
    summary: Option<SharedString>,
    input: Option<SharedString>,
    output: Option<SharedString>,
    status: RowStatus,
    labels: ToolCallLabels,
    grouped: bool,
    open: bool,
    on_event: Option<EventHandler<ToolCallEvent>>,
}

impl ToolCall {
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            summary: None,
            input: None,
            output: None,
            status: RowStatus::Pending,
            labels: ToolCallLabels::default(),
            grouped: false,
            open: false,
            on_event: None,
        }
    }

    pub fn summary(mut self, summary: impl Into<SharedString>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    pub fn input(mut self, input: impl Into<SharedString>) -> Self {
        self.input = Some(input.into());
        self
    }

    pub fn output(mut self, output: impl Into<SharedString>) -> Self {
        self.output = Some(output.into());
        self
    }

    pub fn status(mut self, status: RowStatus) -> Self {
        self.status = status;
        self
    }

    pub fn labels(mut self, labels: ToolCallLabels) -> Self {
        self.labels = labels;
        self
    }

    pub fn grouped(mut self) -> Self {
        self.grouped = true;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ToolCallEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    fn status_label(&self) -> &SharedString {
        match self.status {
            RowStatus::Pending => &self.labels.pending,
            RowStatus::Running => &self.labels.running,
            RowStatus::Success | RowStatus::Resolved => &self.labels.completed,
            RowStatus::Failed => &self.labels.failed,
            RowStatus::Cancelled => &self.labels.cancelled,
            RowStatus::Unknown => &self.labels.unknown,
        }
    }

    fn status_color(&self, cx: &App) -> gpui::Hsla {
        match self.status {
            RowStatus::Pending => cx.theme().muted_foreground,
            RowStatus::Running => cx.theme().info,
            RowStatus::Success | RowStatus::Resolved => cx.theme().success,
            RowStatus::Failed => cx.theme().danger,
            RowStatus::Cancelled => cx.theme().warning,
            RowStatus::Unknown => cx.theme().muted_foreground,
        }
    }
}

impl RenderOnce for ToolCall {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let event = ToolCallEvent::Toggled {
            id: self.id.clone(),
            open: !self.open,
        };
        let handler = self.on_event.clone();
        let color = self.status_color(cx);
        let mut header = h_flex()
            .id(ElementId::Name(format!("tool-call-toggle-{id}").into()))
            .debug_selector(move || format!("tool-call-toggle-{id}"))
            .role(Role::Button)
            .aria_label(self.name.clone())
            .aria_expanded(self.open)
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .when(self.grouped, |row| row.h(px(37.)))
            .child(
                Icon::new(if self.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .flex_none()
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(self.name.clone()),
            )
            .when_some(self.summary.clone(), |row, summary| {
                row.child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(summary),
                )
            })
            .when(!self.grouped, |row| {
                row.child(
                    h_flex()
                        .flex_none()
                        .items_center()
                        .gap_1()
                        .child(div().size(px(6.)).rounded_full().bg(color))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(self.status_label().clone()),
                        ),
                )
            });
        if let Some(handler) = handler {
            header = header
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
        }
        v_flex()
            .id(ElementId::Name(self.id.clone()))
            .w_full()
            .min_w_0()
            .child(header)
            .when(self.open, |column| {
                let mut body = v_flex()
                    .debug_selector({
                        let id = self.id.clone();
                        move || format!("tool-call-body-{id}")
                    })
                    .w_full()
                    .min_w_0()
                    .max_h(px(TOOL_BODY_MAX_HEIGHT))
                    .overflow_hidden()
                    .gap_1()
                    .px_2()
                    .pb_2();
                if let Some(input) = self.input {
                    body = body.child(
                        v_flex()
                            .debug_selector({
                                let id = self.id.clone();
                                move || format!("tool-call-input-{id}")
                            })
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.labels.input.clone()),
                            )
                            .child(
                                TextView::markdown(
                                    ElementId::Name(format!("{}-input", self.id).into()),
                                    format!("```\n{input}\n```"),
                                )
                                .selectable(true),
                            ),
                    );
                }
                if let Some(output) = self.output {
                    body = body.child(
                        v_flex()
                            .debug_selector({
                                let id = self.id.clone();
                                move || format!("tool-call-output-{id}")
                            })
                            .gap_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(self.labels.output.clone()),
                            )
                            .child(
                                TextView::markdown(
                                    ElementId::Name(format!("{}-output", self.id).into()),
                                    output.to_string(),
                                )
                                .selectable(true),
                            ),
                    );
                }
                column.child(body)
            })
    }
}

/// An interaction emitted by [`ToolGroup`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ToolGroupEvent {
    Toggled { id: SharedString, open: bool },
}

/// A collapsible group of consecutive tool calls.
#[derive(IntoElement)]
pub struct ToolGroup {
    id: SharedString,
    title: SharedString,
    count: usize,
    compact: bool,
    quiet: bool,
    active: bool,
    open: bool,
    on_event: Option<EventHandler<ToolGroupEvent>>,
    children: Vec<AnyElement>,
}

impl ToolGroup {
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: SharedString::default(),
            count: 0,
            compact: false,
            quiet: false,
            active: false,
            open: true,
            on_event: None,
            children: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = title.into();
        self
    }

    pub fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    pub fn compact(mut self) -> Self {
        self.compact = true;
        self
    }

    pub fn quiet(mut self, quiet: bool) -> Self {
        self.quiet = quiet;
        self
    }

    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    pub fn open(mut self, open: bool) -> Self {
        self.open = open;
        self
    }

    pub fn on_event(
        mut self,
        handler: impl Fn(&ToolGroupEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl ParentElement for ToolGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for ToolGroup {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let id = self.id.clone();
        let count_id = self.id.clone();
        let event = ToolGroupEvent::Toggled {
            id: self.id.clone(),
            open: !self.open,
        };
        let handler = self.on_event;
        let mut header = h_flex()
            .id(ElementId::Name(format!("tool-group-toggle-{id}").into()))
            .debug_selector(move || format!("tool-group-toggle-{id}"))
            .role(Role::Button)
            .aria_label(self.title.clone())
            .aria_expanded(self.open)
            .w_full()
            .min_w_0()
            .items_center()
            .gap_2()
            .px_2()
            .when(self.quiet, |row| row.h(px(32.)))
            .when(self.compact && !self.quiet, |row| row.h(px(37.)))
            .child(
                Icon::new(if self.open {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                })
                .xsmall()
                .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .truncate()
                    .text_xs()
                    .font_semibold()
                    .when(self.active, |title| title.text_color(cx.theme().info))
                    .child(self.title.clone()),
            )
            .when(self.count > 1, |row| {
                row.child(
                    div()
                        .debug_selector(move || format!("tool-group-count-{count_id}"))
                        .flex_none()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{}", self.count)),
                )
            });
        if let Some(handler) = handler {
            header = header
                .cursor_pointer()
                .hover(|style| style.bg(cx.theme().muted.opacity(0.5)))
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx));
        }
        v_flex()
            .id(ElementId::Name(self.id))
            .w_full()
            .min_w_0()
            .child(header)
            .when(self.open, |column| {
                column.child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .children(self.children),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Context, Modifiers, MouseButton, Render, TestAppContext, div, point, px, size};

    use super::*;

    struct SelectionProbe;

    impl Render for SelectionProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().child(gpui_base::TextSelectionLayer).child(
                v_flex()
                    .gap_2()
                    .child(StreamingText::new(
                        "streaming-test",
                        &StreamedContent::done("Alpha beta gamma".into()),
                    ))
                    .child(Thinking::new("thinking-test", "Alpha beta gamma").open(true))
                    .child(
                        ToolCall::new("tool-test", "bash")
                            .output("Alpha beta gamma")
                            .open(true),
                    ),
            )
        }
    }

    struct ToolGroupProbe;

    impl Render for ToolGroupProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex()
                .gap_2()
                .child(ToolGroup::new("single").count(1).quiet(true))
                .child(ToolGroup::new("multi").count(3))
        }
    }

    struct LongToolProbe;

    impl Render for LongToolProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let long = (0..120)
                .map(|line| format!("line {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            div().w(px(800.)).child(
                ToolCall::new("long-tool", "bash")
                    .input(long.clone())
                    .output(long)
                    .open(true),
            )
        }
    }

    #[gpui::test]
    fn one_tool_group_hides_the_redundant_count(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (_probe, window) = cx.add_window_view(|_, _| ToolGroupProbe);
        window.simulate_resize(size(px(400.), px(300.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            window.debug_bounds("tool-group-count-single").is_none(),
            "a one-tool group does not need a trailing count"
        );
        assert!(
            window.debug_bounds("tool-group-count-multi").is_some(),
            "multi-tool groups keep their count"
        );
    }

    #[gpui::test]
    fn an_expanded_tool_body_is_a_bounded_preview(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (_probe, window) = cx.add_window_view(|_, _| LongToolProbe);
        window.simulate_resize(size(px(800.), px(1200.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        let body = window
            .debug_bounds("tool-call-body-long-tool")
            .expect("long tool body");
        assert!(
            body.size.height <= px(TOOL_BODY_MAX_HEIGHT),
            "tool body {:?} must stay a preview",
            body.size.height
        );
    }

    fn drag_select(window: &mut gpui::VisualTestContext, selector: &'static str) {
        let bounds = window.debug_bounds(selector).expect(selector);
        let y = bounds.origin.y + bounds.size.height - px(10.);
        let start = point(bounds.origin.x + px(28.), y);
        let end = point(bounds.origin.x + px(180.), y);
        window.simulate_event(gpui::MouseDownEvent {
            position: start,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: true,
        });
        window.simulate_event(gpui::MouseMoveEvent {
            position: end,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        window.simulate_event(gpui::MouseUpEvent {
            position: end,
            modifiers: Modifiers::default(),
            button: MouseButton::Left,
            click_count: 1,
        });
        window.run_until_parked();
    }

    #[gpui::test]
    fn read_only_transcript_text_is_selectable(cx: &mut TestAppContext) {
        cx.update(gpui_component::init);
        let (_probe, window) = cx.add_window_view(|_, _| SelectionProbe);
        window.simulate_resize(size(px(800.), px(900.)));
        window.update(|window, cx| window.draw(cx).clear(cx));
        for selector in [
            "streaming-text-body",
            "thinking-body-thinking-test",
            "tool-call-output-tool-test",
        ] {
            drag_select(window, selector);
            let selected = window.update(gpui_base::TextSelection::selected_text);
            assert!(
                !selected.trim().is_empty(),
                "{selector} must expose selectable text"
            );
            window.update(gpui_base::TextSelection::clear);
            window.update(|window, cx| window.draw(cx).clear(cx));
        }
    }
}
