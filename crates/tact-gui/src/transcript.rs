//! Transcript row model and rendering.
//!
//! The scroller owns virtualization; this module owns the product policy for
//! each row shape. Streaming and agent integration will mutate the same row
//! model without changing the render path.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{
    ActiveTheme as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    text::{TextView, TextViewStyle},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use tact_protocol::ToolVisualKind;

use gpui_kit::{
    AnyElement, App, ClipboardItem, InteractiveElement as _, IntoElement, ParentElement as _,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled as _, div, relative,
    rems,
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
        /// Local wall-clock time the row appeared, as a Unix timestamp. The
        /// prototype's `.msgMeta` prints it as `You 10:42`.
        sent_at: i64,
    },
    Assistant {
        markdown: String,
        streaming: bool,
        /// Local wall-clock time the first chunk arrived.
        sent_at: i64,
        /// Model that produced the turn, when the session had reported one.
        model: Option<String>,
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
        /// What kind of tool produced this row. The summary meta reads it to
        /// decide between a line count and the diff badge the prototype shows
        /// for writes.
        visual_kind: ToolVisualKind,
        /// Lines the change added and removed, counted from the diff body the
        /// tool reported. `None` until a write or edit delivers one.
        diff_stats: Option<(u32, u32)>,
    },
    System {
        text: String,
    },
    Error {
        text: String,
    },
}

/// The prototype's `.toolMeta`: the duration, then how much arrived.
///
/// A write shows the diff badge in that column instead, and a row that has not
/// produced output yet has no line count to report.
fn tool_meta(
    duration: &str,
    output: &str,
    visual_kind: ToolVisualKind,
    diff_stats: Option<(u32, u32)>,
) -> String {
    let lines = match visual_kind {
        ToolVisualKind::FileWrite | ToolVisualKind::FileEdit => 0,
        _ => output.lines().count(),
    };
    // The prototype's `.diffBtn`: `+142 −0`, using U+2212 like the design.
    let badge = match (visual_kind, diff_stats) {
        (ToolVisualKind::FileWrite | ToolVisualKind::FileEdit, Some((added, removed))) => {
            Some(format!("+{added} \u{2212}{removed}"))
        }
        _ => None,
    };
    match (duration.is_empty(), lines, badge) {
        (_, _, Some(badge)) if duration.is_empty() => badge,
        (_, _, Some(badge)) => format!("{duration} · {badge}"),
        (true, 0, None) => String::new(),
        (false, 0, None) => duration.to_string(),
        (true, 1, None) => "1 line".to_string(),
        (true, lines, None) => format!("{lines} lines"),
        (false, 1, None) => format!("{duration} · 1 line"),
        (false, lines, None) => format!("{duration} · {lines} lines"),
    }
}

/// The prototype's `.code` card: a bordered, tinted block around a fence.
///
/// `.codeHead` is a header row, which this renderer does not offer; the card
/// reserves that band as top padding and puts the copy action in it, so the
/// code itself still starts below the corner button.
fn code_block_style(cx: &App) -> TextViewStyle {
    TextViewStyle::default().code_block(
        StyleRefinement::default()
            .border_1()
            .border_color(cx.theme().border)
            .rounded(rems(0.625))
            .bg(cx.theme().muted)
            .pt(rems(2.))
            .px(rems(0.75))
            .pb(rems(0.6875)),
    )
}

/// The prototype's `.msgMeta`: the author, the clock, and an optional suffix.
///
/// `.msgMeta` is 10.5px in the muted ink with an 11px semibold author, so the
/// author reads one step stronger than the time and model beside it.
fn msg_meta(
    cx: &App,
    id: SharedString,
    author: &str,
    clock: String,
    suffix: Option<&str>,
) -> AnyElement {
    let mut row = h_flex()
        .id(id)
        .items_center()
        .gap(rems(0.4375))
        .mb(rems(0.4375))
        .text_size(rems(0.65625))
        .text_color(cx.theme().muted_foreground);
    row = row.child(
        div()
            .text_size(rems(0.6875))
            .font_semibold()
            .text_color(cx.theme().foreground)
            .child(SharedString::from(author.to_string())),
    );
    if !clock.is_empty() {
        row = row.child(div().child(SharedString::from(clock)));
    }
    if let Some(suffix) = suffix
        && !suffix.is_empty()
    {
        row = row.child(div().child(SharedString::from(suffix.to_string())));
    }
    row.test_support().into_any_element()
}

/// Lines a unified-diff body adds and removes.
///
/// Only the body a write or edit tool reported is scanned, and the file
/// headers (`+++` / `---`) are not changes. `None` means the text carried no
/// hunks at all, so the card has no counts to show rather than showing zero.
pub(crate) fn diff_line_counts(text: &str) -> Option<(u32, u32)> {
    let mut added = 0;
    let mut removed = 0;
    for line in text.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added > 0 || removed > 0).then_some((added, removed))
}

/// The `.msgMeta` clock: local `HH:MM` for a Unix timestamp.
pub(crate) fn clock_label(unix_seconds: i64) -> String {
    match chrono::DateTime::from_timestamp(unix_seconds, 0) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string(),
        None => String::new(),
    }
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
        TranscriptRow::User { text, sent_at } => div()
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
                    .child(msg_meta(
                        cx,
                        SharedString::from(format!("user-meta-{index}")),
                        "You",
                        clock_label(*sent_at),
                        None,
                    ))
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
            sent_at,
            model,
        } => h_flex()
            .id(row_id)
            .w_full()
            .min_w_0()
            .items_start()
            // The prototype's `.msg` gap between gutter and body.
            .gap(rems(0.75))
            .py_2()
            .child(
                // `.gutter`: a 24px rounded square holding the agent's initial,
                // nudged 1px so it sits level with the first line of text.
                div()
                    .id(SharedString::from(format!("assistant-gutter-{index}")))
                    .flex_shrink_0()
                    .size(rems(1.5))
                    .mt(rems(0.0625))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(rems(0.4375))
                    .border_1()
                    .border_color(cx.theme().border)
                    .bg(cx.theme().muted)
                    .text_color(cx.theme().primary)
                    .text_size(rems(0.625))
                    .font_semibold()
                    .child(SharedString::from("T"))
                    .test_support(),
            )
            .child(
                div()
                    .id(SharedString::from(format!("assistant-body-{index}")))
                    .min_w_0()
                    .flex_1()
                    .child(msg_meta(
                        cx,
                        SharedString::from(format!("assistant-meta-{index}")),
                        "Tact",
                        clock_label(*sent_at),
                        model.as_deref(),
                    ))
                    .child(
                        TextView::markdown(
                            SharedString::from(format!("assistant-{index}")),
                            SharedString::from(markdown.clone()),
                        )
                        .selectable(true)
                        .style(code_block_style(cx))
                        .code_block_actions(|code_block, _, _cx| {
                            // `.codeHead`'s Copy. The fence is the only text a
                            // code block owns, so it is what the button copies.
                            let code = code_block.code().to_string();
                            Button::new("code-copy")
                                .icon(IconName::Copy)
                                .ghost()
                                .compact()
                                .tooltip("Copy code")
                                .accessibility_label("Copy code")
                                .on_click(move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(code.clone()))
                                })
                        })
                        .stream_fade(*streaming),
                    )
                    .test_support(),
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
            visual_kind,
            diff_stats,
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
                                .child(SharedString::from(tool_meta(
                                    duration,
                                    output,
                                    *visual_kind,
                                    *diff_stats,
                                ))),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_meta_reports_duration_and_line_count() {
        let output = "one\ntwo\nthree\n";
        assert_eq!(
            tool_meta("1.2s", output, ToolVisualKind::FileRead, None),
            "1.2s · 3 lines"
        );
        assert_eq!(
            tool_meta("1.2s", output, ToolVisualKind::Command, None),
            "1.2s · 3 lines"
        );
        assert_eq!(
            tool_meta("1.2s", "only line", ToolVisualKind::FileRead, None),
            "1.2s · 1 line"
        );
    }

    #[test]
    fn tool_meta_omits_the_count_for_writes_and_empty_output() {
        // A write shows the prototype's diff badge in that column instead.
        assert_eq!(
            tool_meta("0.8s", "+ a\n+ b\n", ToolVisualKind::FileWrite, None),
            "0.8s"
        );
        assert_eq!(
            tool_meta("0.8s", "+ a\n", ToolVisualKind::FileEdit, None),
            "0.8s"
        );
        // Nothing has arrived yet: the duration stands alone.
        assert_eq!(tool_meta("1.2s", "", ToolVisualKind::Command, None), "1.2s");
        assert_eq!(tool_meta("", "", ToolVisualKind::Command, None), "");
    }

    #[test]
    fn tool_meta_shows_the_diff_badge_for_a_write() {
        // The prototype's `.diffBtn` sits where the line count sits, so a
        // write reports the change rather than how much text arrived.
        assert_eq!(
            tool_meta("0.8s", "", ToolVisualKind::FileWrite, Some((142, 0)),),
            "0.8s · +142 \u{2212}0"
        );
        assert_eq!(
            tool_meta("", "", ToolVisualKind::FileEdit, Some((3, 1))),
            "+3 \u{2212}1"
        );
        // A write with no recorded change keeps the plain duration.
        assert_eq!(
            tool_meta("0.8s", "", ToolVisualKind::FileWrite, None),
            "0.8s"
        );
    }

    #[test]
    fn diff_line_counts_ignore_file_headers() {
        let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n context\n-removed\n+added\n+more\n";
        assert_eq!(diff_line_counts(diff), Some((2, 1)));
        // A read's file contents are not a diff, so there is nothing to badge.
        assert_eq!(diff_line_counts("fn main() {}\n"), None);
        assert_eq!(diff_line_counts(""), None);
    }

    #[test]
    fn clock_labels_render_local_hh_mm() {
        let label = clock_label(0);
        assert_eq!(label.len(), 5, "HH:MM is five columns: {label:?}");
        assert_eq!(label.as_bytes()[2], b':');
    }
}
