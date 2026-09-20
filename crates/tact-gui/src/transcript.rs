//! Transcript row model and rendering.
//!
//! The scroller owns virtualization; this module owns the product policy for
//! each row shape. Streaming and agent integration will mutate the same row
//! model without changing the render path.

use std::rc::Rc;

use gpui_kit::assets::IconName;
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{
    ActiveTheme as _, h_flex,
    text::{MarkdownNode, MarkdownParseContext, TextView, markdown_ast},
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use tact_protocol::ToolVisualKind;

use gpui_kit::{
    AnyElement, App, ClipboardItem, InteractiveElement as _, IntoElement, MouseButton,
    ParentElement as _, SharedString, StatefulInteractiveElement as _, Styled as _, Window, div,
    relative, rems,
};

/// Opens or closes one collapsible transcript row.
///
/// A row's click handler runs against a plain [`App`], so the callback carries
/// the row index back to the shell entity, which owns the row model.
pub(crate) type RowToggle = Rc<dyn Fn(usize, &mut App)>;

/// Shows the Diff work pane, for the write row's `.diffBtn`.
///
/// The prototype binds `#openDiff` to the badge rather than the whole summary,
/// so clicking the counts jumps to the diff while clicking the row still opens
/// the tool output.
pub(crate) type OpenDiff = Rc<dyn Fn(&mut App)>;

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
        /// How long the model spent on this block, once its stream finished.
        /// `None` while it is still arriving, and for rows restored without
        /// timing information — the summary then reads "Thinking" rather than
        /// inventing a duration.
        duration_seconds: Option<u64>,
        /// Whether the reasoning body is open. The prototype's `.thinking` is
        /// a `<details>` the reader can collapse from its summary row.
        expanded: bool,
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
    status: ToolStatus,
    visual_kind: ToolVisualKind,
    diff_stats: Option<(u32, u32)>,
) -> String {
    if status == ToolStatus::Running {
        return if duration.is_empty() {
            "live".to_string()
        } else {
            format!("{duration} · live")
        };
    }
    let lines = match visual_kind {
        ToolVisualKind::FileWrite | ToolVisualKind::FileEdit => 0,
        _ => output.lines().count(),
    };
    let badge = diff_badge(visual_kind, diff_stats);
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

/// The prototype's `.diffBtn`: `+142 −0`, using U+2212 like the design.
///
/// Only a write or edit reports a change; a read's line count is not a diff.
fn diff_badge(visual_kind: ToolVisualKind, diff_stats: Option<(u32, u32)>) -> Option<String> {
    match (visual_kind, diff_stats) {
        (ToolVisualKind::FileWrite | ToolVisualKind::FileEdit, Some((added, removed))) => {
            Some(format!("+{added} \u{2212}{removed}"))
        }
        _ => None,
    }
}

/// Custom Markdown block name for the prototype's `.code` card.
const CODE_BLOCK: &str = "tact-code";

/// What a fenced block carries from parsing into rendering.
#[derive(Clone)]
struct CodeBlockData {
    /// The fence's info string, such as `toml`; empty for a bare fence.
    language: String,
    /// The fence's body.
    code: String,
}

/// Turn every fenced code block into a `.code` card node.
///
/// The stock renderer owns only a corner slot for block actions, so the
/// prototype's `.codeHead` band — the language on the left, `Copy` on the right
/// — has nowhere to live inside it. A custom block owns the whole card instead.
fn parse_code_block(
    node: &markdown_ast::Node,
    _context: &MarkdownParseContext<'_>,
) -> Option<MarkdownNode> {
    let markdown_ast::Node::Code(code) = node else {
        return None;
    };
    Some(
        MarkdownNode::new(
            CODE_BLOCK,
            CodeBlockData {
                language: code.lang.clone().unwrap_or_default(),
                code: code.value.clone(),
            },
        )
        .text(code.value.clone()),
    )
}

/// The prototype's `.code`: a bordered card with a `.codeHead` band over the
/// fence body.
///
/// Copy fires on press rather than click: a custom block carries no element id
/// of its own, and `on_click` needs one that is unique among a message's blocks.
fn render_code_block(node: &MarkdownNode, _window: &mut Window, cx: &mut App) -> AnyElement {
    let data = node.data::<CodeBlockData>();
    let language = data
        .map(|data| data.language.trim())
        .filter(|language| !language.is_empty())
        .unwrap_or("code");
    let code = data.map(|data| data.code.clone()).unwrap_or_default();
    let clipboard = code.clone();
    let band_ink = cx.theme().muted_foreground;
    let hover_ink = cx.theme().foreground;

    v_flex()
        .w_full()
        .rounded(rems(0.625))
        .border_1()
        .border_color(cx.theme().border)
        .bg(cx.theme().muted)
        .overflow_hidden()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .gap_2()
                .border_b_1()
                .border_color(cx.theme().border)
                .px(rems(0.625))
                .py(rems(0.4375))
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(rems(0.65625))
                .text_color(band_ink)
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .child(SharedString::from(language.to_string())),
                )
                .child(div().flex_1())
                .child(
                    div()
                        .flex_shrink_0()
                        .cursor_pointer()
                        .hover(move |style| style.text_color(hover_ink))
                        .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(clipboard.clone()));
                        })
                        .child(SharedString::from("Copy")),
                ),
        )
        .child(
            div()
                .w_full()
                .px(rems(0.75))
                .py(rems(0.6875))
                .font_family(cx.theme().mono_font_family.clone())
                .text_size(rems(0.71875))
                .line_height(relative(1.6))
                .text_color(cx.theme().muted_foreground)
                .child(SharedString::from(code)),
        )
        .into_any_element()
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
    open_diff: &OpenDiff,
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
                            .text_size(rems(0.8125))
                            .line_height(relative(1.45))
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
            // The prototype's `.msg` gap between gutter and body. Vertical
            // rhythm belongs to the scroller's 18px row gap, not this row.
            .gap(rems(0.75))
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
                        .text_size(rems(0.8125))
                        .line_height(relative(1.45))
                        .markdown_block_parser(parse_code_block)
                        .markdown_block_renderer(CODE_BLOCK, render_code_block)
                        .stream_fade(*streaming),
                    )
                    .test_support(),
            )
            .test_support()
            .into_any_element(),
        TranscriptRow::Thinking {
            text,
            duration_seconds,
            expanded,
        } => {
            // The prototype's `.thinking` summary is `Thought for 8s` on the
            // left and the open state (`Normal · hidden` / `Expanded ·
            // detailed`) pinned right in mono. A row without a recorded
            // duration keeps the plain label rather than claiming "0s".
            let label = match duration_seconds {
                Some(seconds) => format!("Thought for {seconds}s"),
                None => "Thinking".to_string(),
            };
            let state = if *expanded {
                "Expanded · detailed"
            } else {
                "Normal · hidden"
            };
            let chevron = if *expanded {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            };
            let toggle = toggle.clone();
            v_flex()
                .id(row_id)
                .w_full()
                .rounded(rems(0.625))
                .border_1()
                .border_color(cx.theme().border)
                .bg(cx.theme().muted)
                .overflow_hidden()
                .child(
                    h_flex()
                        .id(SharedString::from(format!("thinking-summary-{index}")))
                        .w_full()
                        .items_center()
                        .gap(rems(0.5))
                        .px(rems(0.6875))
                        .py(rems(0.5625))
                        .text_color(cx.theme().muted_foreground)
                        .on_click(move |_, _, cx| toggle(index, cx))
                        .child(div().size(rems(0.875)).child(chevron))
                        .child(
                            div()
                                .text_size(rems(0.71875))
                                .font_semibold()
                                .child(SharedString::from(label)),
                        )
                        .child(
                            div()
                                .ml_auto()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(rems(0.65625))
                                .child(SharedString::from(state)),
                        ),
                )
                .when(*expanded, |card| {
                    card.child(
                        // `.content` indents to the chevron's text column.
                        div()
                            .pl(rems(2.1875))
                            .pr(rems(0.8125))
                            .pb(rems(0.75))
                            .text_size(rems(0.84375))
                            .line_height(relative(1.62))
                            .text_color(cx.theme().muted_foreground)
                            .child(SharedString::from(text.clone())),
                    )
                })
                .test_support()
                .into_any_element()
        }
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
                .when(open, |this| {
                    this.bg(cx.theme().muted).border_color(cx.theme().input)
                })
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
                        .when_some(*diff_stats, |summary, (added, removed)| {
                            // `.diffBtn`: the counts sit in their own bordered
                            // chip, green for additions and red for removals,
                            // and open the Diff pane instead of the tool body.
                            let open_diff = open_diff.clone();
                            summary.child(
                                h_flex()
                                    .id(SharedString::from(format!("tool-diff-{index}")))
                                    .test_support()
                                    .flex_shrink_0()
                                    .items_center()
                                    .gap(rems(0.25))
                                    .h(rems(1.375))
                                    .px(rems(0.375))
                                    .rounded(rems(0.375))
                                    .border_1()
                                    .border_color(cx.theme().border)
                                    .bg(cx.theme().popover)
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_size(rems(0.625))
                                    .on_click(move |_, _, cx| {
                                        cx.stop_propagation();
                                        open_diff(cx);
                                    })
                                    .child(
                                        div()
                                            .text_color(cx.theme().success)
                                            .child(SharedString::from(format!("+{added}"))),
                                    )
                                    .child(
                                        div().text_color(cx.theme().danger).child(
                                            SharedString::from(format!("\u{2212}{removed}")),
                                        ),
                                    ),
                            )
                        })
                        .child(
                            div()
                                .flex_shrink_0()
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_size(rems(0.625))
                                .text_color(cx.theme().muted_foreground)
                                .child(SharedString::from(tool_meta(
                                    duration,
                                    output,
                                    *status,
                                    *visual_kind,
                                    None,
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
                            .line_height(relative(1.6))
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
            tool_meta(
                "1.2s",
                output,
                ToolStatus::Succeeded,
                ToolVisualKind::FileRead,
                None
            ),
            "1.2s · 3 lines"
        );
        assert_eq!(
            tool_meta(
                "1.2s",
                output,
                ToolStatus::Succeeded,
                ToolVisualKind::Command,
                None
            ),
            "1.2s · 3 lines"
        );
        assert_eq!(
            tool_meta(
                "1.2s",
                "only line",
                ToolStatus::Succeeded,
                ToolVisualKind::FileRead,
                None
            ),
            "1.2s · 1 line"
        );
        assert_eq!(
            tool_meta(
                "18.4s",
                "checking",
                ToolStatus::Running,
                ToolVisualKind::Command,
                None
            ),
            "18.4s · live"
        );
    }

    #[test]
    fn tool_meta_omits_the_count_for_writes_and_empty_output() {
        // A write shows the prototype's diff badge in that column instead.
        assert_eq!(
            tool_meta(
                "0.8s",
                "+ a\n+ b\n",
                ToolStatus::Succeeded,
                ToolVisualKind::FileWrite,
                None
            ),
            "0.8s"
        );
        assert_eq!(
            tool_meta(
                "0.8s",
                "+ a\n",
                ToolStatus::Succeeded,
                ToolVisualKind::FileEdit,
                None
            ),
            "0.8s"
        );
        // Nothing has arrived yet: the duration stands alone.
        assert_eq!(
            tool_meta(
                "1.2s",
                "",
                ToolStatus::Succeeded,
                ToolVisualKind::Command,
                None
            ),
            "1.2s"
        );
        assert_eq!(
            tool_meta("", "", ToolStatus::Succeeded, ToolVisualKind::Command, None),
            ""
        );
    }

    #[test]
    fn tool_meta_shows_the_diff_badge_for_a_write() {
        // The prototype's `.diffBtn` sits where the line count sits, so a
        // write reports the change rather than how much text arrived.
        assert_eq!(
            tool_meta(
                "0.8s",
                "",
                ToolStatus::Succeeded,
                ToolVisualKind::FileWrite,
                Some((142, 0)),
            ),
            "0.8s · +142 \u{2212}0"
        );
        assert_eq!(
            tool_meta(
                "",
                "",
                ToolStatus::Succeeded,
                ToolVisualKind::FileEdit,
                Some((3, 1))
            ),
            "+3 \u{2212}1"
        );
        // A write with no recorded change keeps the plain duration.
        assert_eq!(
            tool_meta(
                "0.8s",
                "",
                ToolStatus::Succeeded,
                ToolVisualKind::FileWrite,
                None
            ),
            "0.8s"
        );
    }

    #[test]
    fn the_diff_badge_belongs_to_writes_that_reported_a_change() {
        assert_eq!(
            diff_badge(ToolVisualKind::FileWrite, Some((142, 0))).as_deref(),
            Some("+142 \u{2212}0")
        );
        assert_eq!(
            diff_badge(ToolVisualKind::FileEdit, Some((3, 1))).as_deref(),
            Some("+3 \u{2212}1")
        );
        // A read has no change to open, and a write without a counted diff
        // has nothing to badge either.
        assert_eq!(diff_badge(ToolVisualKind::FileRead, Some((1, 1))), None);
        assert_eq!(diff_badge(ToolVisualKind::FileWrite, None), None);
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
