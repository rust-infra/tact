//! Transcript row model and rendering.
//!
//! The scroller owns virtualization; this module owns the product policy for
//! each row shape. Streaming and agent integration will mutate the same row
//! model without changing the render path.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{ActiveTheme as _, h_flex, text::TextView, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, div, relative, rems,
};

/// Opens or closes one collapsible transcript row.
///
/// A row's click handler runs against a plain [`App`], so the callback carries
/// the row index back to the shell entity, which owns the row model.
pub(crate) type RowToggle = Rc<dyn Fn(usize, &mut App)>;

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
        /// Raw stream text accumulated from `ToolProgress`, shown by the card's
        /// output block.
        output: String,
        duration: String,
        status: ToolStatus,
        /// Whether the reader opened this card's output block.
        expanded: bool,
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
    toggle: &RowToggle,
    cx: &App,
) -> AnyElement {
    let row_id = SharedString::from(format!("transcript-row-{index}"));
    // Verbose expands every tool card's output, per this enum's contract; a
    // click on one card only toggles that card.
    let verbose = detail == TranscriptDetail::Verbose;

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
                // The prototype's `.msg.user .body`: a 72% cap, 10/12 padding,
                // and a tail corner where the bubble meets the right edge.
                div()
                    .id(SharedString::from(format!("user-bubble-{index}")))
                    .max_w(relative(0.72))
                    .rounded_tl(rems(0.75))
                    .rounded_tr(rems(0.75))
                    .rounded_br(rems(0.25))
                    .rounded_bl(rems(0.75))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted)
                    .px(rems(0.75))
                    .py(rems(0.625))
                    .child(
                        div()
                            .min_w_0()
                            .w_full()
                            .child(SharedString::from(text.clone())),
                    )
                    .test_support(),
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
            .rounded(rems(0.625))
            .border_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().muted)
            .overflow_hidden()
            .child(
                // The prototype's `.thinking` summary: chevron, label, and the
                // active detail level pinned to the right in mono.
                h_flex()
                    .w_full()
                    .items_center()
                    .gap(rems(0.5))
                    .px(rems(0.6875))
                    .py(rems(0.5625))
                    .text_color(cx.theme().muted_foreground)
                    .child(div().size(rems(0.875)).child(IconName::ChevronDown))
                    .child(
                        div()
                            .text_size(rems(0.71875))
                            .font_semibold()
                            .child(SharedString::from("Thinking")),
                    )
                    .child(
                        div()
                            .ml_auto()
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(rems(0.65625))
                            .child(SharedString::from(detail.label())),
                    ),
            )
            .child(
                // `.content` is indented to sit under the chevron's text column.
                div()
                    .pl(rems(2.1875))
                    .pr(rems(0.8125))
                    .pb(rems(0.75))
                    .text_size(rems(0.84375))
                    .text_color(cx.theme().muted_foreground)
                    .child(SharedString::from(text.clone())),
            )
            .test_support()
            .into_any_element(),
        TranscriptRow::Tool {
            display_name,
            detail,
            output,
            duration,
            status,
            expanded,
        } => {
            // The prototype's `.tool`: a card whose summary is a 38px row of
            // tinted icon chip, name, argument, and mono meta, over an output
            // block the reader opens.
            let (icon, tint, tone) = match status {
                ToolStatus::Running => (
                    IconName::LoaderCircle,
                    cx.theme().primary.opacity(0.12),
                    cx.theme().primary,
                ),
                ToolStatus::Succeeded => (
                    IconName::Check,
                    cx.theme().success.opacity(0.12),
                    cx.theme().success,
                ),
                ToolStatus::Failed => (
                    IconName::TriangleAlert,
                    cx.theme().danger.opacity(0.12),
                    cx.theme().danger,
                ),
            };
            let open = *expanded || verbose;
            let chevron = if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let summary_id = SharedString::from(format!("tool-summary-{index}"));
            // The scroller renders rows from a plain `App`, so the click travels
            // back through the app entity instead of this view's context.
            let toggle = toggle.clone();

            v_flex()
                .id(row_id)
                .w_full()
                .min_w_0()
                .rounded(rems(0.625))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().popover)
                .overflow_hidden()
                .child(
                    h_flex()
                        .id(summary_id)
                        .w_full()
                        .min_w_0()
                        .min_h(rems(2.375))
                        .items_center()
                        .gap(rems(0.5))
                        .pl(rems(0.6875))
                        .pr(rems(0.625))
                        .py(rems(0.3125))
                        .on_click(move |_, _, cx| toggle(index, cx))
                        .child(
                            div()
                                .flex()
                                .size(rems(1.25))
                                .flex_shrink_0()
                                .items_center()
                                .justify_center()
                                .rounded(rems(0.375))
                                .bg(tint)
                                .text_color(tone)
                                .child(icon),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_size(rems(0.75))
                                .font_semibold()
                                .child(SharedString::from(display_name.clone())),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .flex_1()
                                .truncate()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(rems(0.65625))
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(detail.clone())),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(rems(0.625))
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(duration.clone())),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_color(cx.theme().muted_foreground)
                                .child(div().size(rems(0.875)).child(chevron)),
                        )
                        .test_support(),
                )
                .when(open && !output.trim().is_empty(), |card| {
                    card.child(
                        // The prototype's `.out`: a scrollable mono window
                        // indented past the icon column.
                        v_flex()
                            .id(SharedString::from(format!("tool-output-{index}")))
                            .ml(rems(2.4375))
                            .mr(rems(0.75))
                            .mb(rems(0.75))
                            .max_h(rems(9.375))
                            .overflow_hidden()
                            .rounded(rems(0.4375))
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().popover)
                            .px(rems(0.625))
                            .py(rems(0.5625))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(rems(0.65625))
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(output.clone()))
                            .test_support(),
                    )
                })
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
