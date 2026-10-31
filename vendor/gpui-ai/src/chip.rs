//! Compact chips representing tool calls and code edits.

use crate::control::{PressReleaseExt as _, composed_button};
use crate::handlers::SharedHandler;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, spinner::Spinner};
use std::rc::Rc;

/// The lifecycle status of the tool call a [`ToolChip`] represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolStatus {
    /// Queued but not started.
    #[default]
    Pending,
    /// Currently executing; the chip shows a spinner.
    Running,
    /// Completed successfully.
    Success,
    /// Ended with an error.
    Failed,
}

/// A compact, pill-shaped representation of a tool call or code edit.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// ToolChip::new("edit-1", "edit main.rs")
///     .status(ToolStatus::Running)
///     .detail("+12 −3");
/// ```
#[derive(IntoElement)]
pub struct ToolChip {
    id: SharedString,
    style: StyleRefinement,
    label: SharedString,
    detail: Option<SharedString>,
    status: ToolStatus,
    on_event: Option<SharedHandler<ToolChipEvent>>,
}

impl ToolChip {
    /// Creates a chip with a unique id and a short label (for example the
    /// tool name or the file being edited).
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            label: label.into(),
            detail: None,
            status: ToolStatus::default(),
            on_event: None,
        }
    }

    /// Sets the lifecycle status. Default is [`ToolStatus::Pending`].
    pub fn status(mut self, status: ToolStatus) -> Self {
        self.status = status;
        self
    }

    /// Adds a muted trailing detail, such as a diff stat (`+12 −3`).
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Makes the chip clickable (for example to open the tool call).
    pub fn on_event(
        mut self,
        handler: impl Fn(&ToolChipEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    fn accessibility_label(&self) -> SharedString {
        match &self.detail {
            Some(detail) => format!(
                "{}, {}, {}",
                self.label,
                self.status.accessibility_label(),
                detail
            )
            .into(),
            None => format!("{}, {}", self.label, self.status.accessibility_label()).into(),
        }
    }
}

impl ToolStatus {
    fn accessibility_label(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Running => "running",
            Self::Success => "completed",
            Self::Failed => "failed",
        }
    }
}

impl Styled for ToolChip {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// The bordered pill a tool chip wears, whichever branch renders it.
///
/// Deliberately not [`crate::status::chip_frame`]: that vocabulary is
/// borderless and fully round, and this one is bordered at the medium
/// radius. Whether the two should become one is a visual decision, not a
/// refactor — until it is made, this at least stops the pill being stated
/// twice inside one function, where the two copies are what drift.
fn tool_chip_frame<E: Styled>(element: E, cx: &App) -> E {
    let tokens = cx.theme().semantic_tokens();
    element
        .px(tokens.spacing.sm)
        .py(tokens.spacing.xxs)
        .bg(cx.theme().secondary)
        .border_1()
        .border_color(cx.theme().border)
        .rounded(tokens.radius.md)
}

impl RenderOnce for ToolChip {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let status_color = match self.status {
            ToolStatus::Pending => cx.theme().muted_foreground,
            ToolStatus::Running => cx.theme().info,
            ToolStatus::Success => cx.theme().success,
            ToolStatus::Failed => cx.theme().danger,
        };

        // A status change settles its indicator in rather than popping it:
        // the acknowledgment is keyed by the status, so it plays once per
        // transition and never on a re-render, and the status the chip
        // mounts with is exempt — a first render is not a transition. The
        // chip's label never changes with status, so the indicator is the
        // whole swap.
        let status_progress = crate::motion::acknowledged_state(
            ElementId::from((ElementId::from(self.id.clone()), "chip-status")),
            self.status as u64,
            window,
            cx,
        );
        let acknowledged = if self.status == ToolStatus::Running {
            // A running chip's signal is its spinner; fading the spinner in
            // would stack two motions on one slot.
            1.0
        } else {
            status_progress
        };

        let event = ToolChipEvent::Activated {
            id: self.id.clone(),
        };
        let accessibility_label = self.accessibility_label();
        let content = h_flex()
            .items_center()
            .gap(tokens.spacing.xs)
            .text_token(tokens.typography.xs)
            .font_family(cx.theme().mono_font_family.clone())
            .text_color(cx.theme().foreground)
            .child(
                // A fixed square slot: the spinner and the dot centre in the
                // same box, so running↔settled never nudges the label.
                div()
                    .flex_none()
                    .size(crate::sizing::slot_xs(cx))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(match self.status {
                        ToolStatus::Running => Spinner::new()
                            .xsmall()
                            .color(status_color)
                            .into_any_element(),
                        _ => div()
                            .size_1p5()
                            .rounded(tokens.radius.full)
                            .bg(status_color)
                            .opacity(acknowledged)
                            .into_any_element(),
                    }),
            )
            .child(self.label)
            .when_some(self.detail, |this, detail| {
                this.child(div().text_color(cx.theme().muted_foreground).child(detail))
            })
            .refine_style(&self.style);

        if let Some(handler) = self.on_event {
            tool_chip_frame(composed_button(self.id.clone(), accessibility_label), cx)
                .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                .active(|style| style.bg(cx.theme().accent))
                .focus_visible(|style| style.border_color(cx.theme().ring))
                .press_release(
                    ElementId::from((ElementId::from(self.id.clone()), "press")),
                    tokens.radius.md,
                    window,
                    cx,
                )
                .child(content)
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                .into_any_element()
        } else {
            tool_chip_frame(content, cx)
                .id(self.id)
                .role(Role::Status)
                .aria_label(accessibility_label)
                .into_any_element()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, px};
    use gpui_component::h_flex;

    struct ChipProbe {
        status: ToolStatus,
    }

    impl Render for ChipProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(320.)).h(px(80.)).child(
                h_flex()
                    .debug_selector(|| "chip-hug".into())
                    .flex_none()
                    .child(ToolChip::new("probe-chip", "read_file").status(self.status)),
            )
        }
    }

    #[gpui::test]
    fn chip_width_holds_across_every_status(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| ChipProbe {
            status: ToolStatus::Pending,
        });
        let cx: &mut VisualTestContext = cx;
        cx.update(|window, cx| window.draw(cx).clear(cx));
        let width_at = |cx: &mut VisualTestContext| {
            cx.debug_bounds("chip-hug")
                .expect("the chip should render")
                .size
                .width
        };
        let pending = width_at(cx);

        for status in [ToolStatus::Running, ToolStatus::Success, ToolStatus::Failed] {
            probe.update(cx, |probe, cx| {
                probe.status = status;
                cx.notify();
            });
            cx.executor()
                .advance_clock(crate::motion::MotionTokens::DEFAULT.quick() * 2);
            cx.update(|window, cx| window.draw(cx).clear(cx));
            assert_eq!(
                width_at(cx),
                pending,
                "the indicator slot must hold one width; {status:?} moved it"
            );
        }
    }

    #[test]
    fn accessibility_name_carries_status_and_detail() {
        assert_eq!(
            ToolChip::new("read", "read pricing.md")
                .status(ToolStatus::Running)
                .detail("12 files")
                .accessibility_label(),
            "read pricing.md, running, 12 files"
        );
        assert_eq!(
            ToolChip::new("save", "save changes")
                .status(ToolStatus::Success)
                .accessibility_label(),
            "save changes, completed"
        );
    }
}
/// An interaction emitted by [`ToolChip`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolChipEvent {
    /// The represented tool call was selected.
    Activated {
        /// Stable tool-call identifier.
        id: SharedString,
    },
}
