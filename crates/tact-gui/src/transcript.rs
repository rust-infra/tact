//! Transcript row model and rendering.
//!
//! The scroller owns virtualization; this module owns the product policy for
//! each row shape. Streaming and agent integration will mutate the same row
//! model without changing the render path.

use gpui_kit::assets::IconName;
use gpui_kit::base::TestSupportExt as _;
use gpui_kit::component::{
    ActiveTheme as _,
    bubble::{Bubble, BubbleVariant},
    h_flex,
    message::MessageAlignment,
    text::TextView,
    v_flex,
};
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    Styled as _, div, relative,
};

/// How much supporting detail the transcript shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum TranscriptDetail {
    /// The default: reasoning stays hidden.
    #[default]
    Normal,
    /// Reasoning blocks are visible.
    Thinking,
    /// Reasoning and expanded tool detail are visible.
    Verbose,
}

impl TranscriptDetail {
    /// The next state in the Normal → Thinking → Verbose cycle.
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Normal => Self::Thinking,
            Self::Thinking => Self::Verbose,
            Self::Verbose => Self::Normal,
        }
    }

    /// Whether reasoning rows are shown.
    pub(crate) fn shows_thinking(self) -> bool {
        !matches!(self, Self::Normal)
    }

    /// Short label used by controls and the status surface.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Normal => "Normal",
            Self::Thinking => "Thinking",
            Self::Verbose => "Verbose",
        }
    }
}

/// Visual state for one tool activity row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolStatus {
    Running,
    Succeeded,
    Failed,
}

/// One semantic row in the transcript.
///
/// This is deliberately transport-neutral: protocol events map into these
/// rows in the application layer, while the view only decides how to present
/// each variant.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum TranscriptRow {
    User {
        text: String,
    },
    Assistant {
        markdown: String,
        streaming: bool,
    },
    Thinking {
        text: String,
    },
    Tool {
        display_name: String,
        detail: String,
        duration: String,
        status: ToolStatus,
    },
    System {
        text: String,
    },
    Error {
        text: String,
    },
}

/// Render one transcript row at its virtual-list index.
pub(crate) fn render_row(
    row: &TranscriptRow,
    index: usize,
    detail: TranscriptDetail,
    cx: &App,
) -> AnyElement {
    let row_id = SharedString::from(format!("transcript-row-{index}"));

    if !detail.shows_thinking() && matches!(row, TranscriptRow::Thinking { .. }) {
        return div().id(row_id).w_full().into_any_element();
    }

    match row {
        TranscriptRow::User { text } => div()
            .id(row_id)
            .w_full()
            .flex()
            .justify_end()
            .child(
                div().w(relative(0.8)).child(
                    Bubble::new()
                        .w_full()
                        .max_w_full()
                        .alignment(MessageAlignment::End)
                        .with_variant(BubbleVariant::Secondary)
                        .child(
                            div()
                                .min_w_0()
                                .w_full()
                                .child(SharedString::from(text.clone())),
                        ),
                ),
            )
            .test_support()
            .into_any_element(),
        TranscriptRow::Assistant {
            markdown,
            streaming,
        } => div()
            .id(row_id)
            .w_full()
            .min_w_0()
            .py_2()
            .child(
                TextView::markdown(
                    SharedString::from(format!("assistant-{index}")),
                    SharedString::from(markdown.clone()),
                )
                .selectable(true)
                .stream_fade(*streaming),
            )
            .test_support()
            .into_any_element(),
        TranscriptRow::Thinking { text } => v_flex()
            .id(row_id)
            .w_full()
            .gap_1()
            .py_1()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .text_color(cx.theme().muted_foreground)
                    .child(IconName::Cpu)
                    .child(SharedString::from("Thinking")),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(text.clone())),
            )
            .test_support()
            .into_any_element(),
        TranscriptRow::Tool {
            display_name,
            detail,
            duration,
            status,
        } => {
            let (icon, status_color) = match status {
                ToolStatus::Running => (IconName::LoaderCircle, cx.theme().primary),
                ToolStatus::Succeeded => (IconName::Check, cx.theme().success),
                ToolStatus::Failed => (IconName::TriangleAlert, cx.theme().danger),
            };

            h_flex()
                .id(row_id)
                .w_full()
                .min_w_0()
                .items_center()
                .gap_2()
                .rounded(cx.theme().radius_2xl())
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .px_3()
                .py_2()
                .child(div().text_color(status_color).child(icon))
                .child(
                    div()
                        .text_sm()
                        .child(SharedString::from(display_name.clone())),
                )
                .child(
                    div()
                        .min_w_0()
                        .flex_1()
                        .truncate()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(detail.clone())),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(SharedString::from(duration.clone())),
                )
                .test_support()
                .into_any_element()
        }
        TranscriptRow::System { text } => div()
            .id(row_id)
            .w_full()
            .py_1()
            .text_sm()
            .text_color(cx.theme().muted_foreground)
            .child(SharedString::from(text.clone()))
            .test_support()
            .into_any_element(),
        TranscriptRow::Error { text } => h_flex()
            .id(row_id)
            .w_full()
            .min_w_0()
            .items_start()
            .gap_2()
            .rounded(cx.theme().radius_2xl())
            .border_1()
            .border_color(cx.theme().danger)
            .bg(cx.theme().popover)
            .px_3()
            .py_2()
            .child(
                div()
                    .text_color(cx.theme().danger)
                    .child(IconName::TriangleAlert),
            )
            .child(
                div()
                    .min_w_0()
                    .flex_1()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(SharedString::from(text.clone())),
            )
            .test_support()
            .into_any_element(),
    }
}
