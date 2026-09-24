//! Transcript row model and rendering.
//!
//! The scroller owns virtualization; this module owns the product policy for
//! each row shape. Streaming and agent integration will mutate the same row
//! model without changing the render path.

use std::{rc::Rc, time::Duration};

use gpui_ai::{
    stream::{Progressive, StreamedContent},
    streaming_text::StreamingText,
    thinking::{Thinking, ThinkingEvent, ThinkingTrace},
    tool_call::{ToolCall, ToolCallEvent, ToolGroup, ToolInvocation, ToolOutputFormat},
};
use gpui_kit::assets::IconName;
use gpui_kit::base::animation::cubic_bezier;
use gpui_kit::base::motion::{Presence, Transition};
use gpui_kit::base::{StyledExt as _, TestSupportExt as _};
use gpui_kit::component::{ActiveTheme as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use tact_protocol::ToolVisualKind;

use crate::session::Request;

use gpui_kit::{
    AnyElement, App, InteractiveElement as _, IntoElement, ParentElement as _, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, div, px, relative, rems,
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
#[allow(dead_code)]
pub(crate) type OpenDiff = Rc<dyn Fn(&mut App)>;

/// Renders an answered approval row's card.
///
/// The card's own controls are built beside the live request panel in the
/// shell, so the row renderer only decides where the record sits.
pub(crate) type ApprovalCard = Rc<dyn Fn(&Request, &str, &App) -> AnyElement>;

/// How much supporting detail the transcript shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
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
        /// `None` for a row restored from history, which has no timing to
        /// show — the summary then reads "Thoughts" rather than inventing a
        /// duration.
        duration_seconds: Option<u64>,
        /// Whether the block is still arriving.
        ///
        /// Deliberately not derived from `duration_seconds`: a restored row
        /// has no duration *and* is not live, and reading liveness off the
        /// duration made every reopened session's reasoning render as a
        /// still-streaming "Thinking…" card.
        live: bool,
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
    /// A permission or question card from stored history produced by an older
    /// build. Live answers remove the card instead of archiving a row.
    #[allow(dead_code)]
    Approval {
        request: Request,
        result: String,
    },
}

/// The prototype's `.toolMeta`: the duration, then how much arrived.
///
/// A write shows the diff badge in that column instead, and a row that has not
/// produced output yet has no line count to report.
#[allow(dead_code)]
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
#[allow(dead_code)]
fn diff_badge(visual_kind: ToolVisualKind, diff_stats: Option<(u32, u32)>) -> Option<String> {
    match (visual_kind, diff_stats) {
        (ToolVisualKind::FileWrite | ToolVisualKind::FileEdit, Some((added, removed))) => {
            Some(format!("+{added} \u{2212}{removed}"))
        }
        _ => None,
    }
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
        .text_color(crate::theme::ink3(cx));
    row = row.child(
        div()
            .text_size(rems(0.6875))
            .font_semibold()
            .text_color(cx.theme().muted_foreground)
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
    // A row redrawn from stored history has no stamp of its own; showing one
    // would date the message to whatever moment the session was reopened.
    if unix_seconds <= 0 {
        return String::new();
    }
    match chrono::DateTime::from_timestamp(unix_seconds, 0) {
        Some(utc) => utc
            .with_timezone(&chrono::Local)
            .format("%H:%M")
            .to_string(),
        None => String::new(),
    }
}

/// `.msg{animation:rise 320ms var(--ease) both}` -- the user and assistant
/// message bodies lift into place on insertion.
const MESSAGE_RISE: Duration = Duration::from_millis(320);

fn message_rise_policy() -> Transition {
    Transition::new(MESSAGE_RISE).ease(cubic_bezier(0.23, 1.0, 0.32, 1.0))
}

/// Build the streamed Markdown source with gpui-ai's lifecycle semantics.
///
/// The desktop transcript must keep Tact's custom Mermaid block renderer, so it
/// cannot mount `gpui_ai::StreamingText` wholesale; it does adopt the same
/// `StreamedContent` state and streaming cursor that component uses.
fn streamed_markdown(markdown: &str, streaming: bool) -> (StreamedContent, bool) {
    let content = if streaming {
        StreamedContent::running(markdown.to_string())
    } else {
        StreamedContent::done(markdown.to_string())
    };
    (content, streaming)
}

fn parse_duration_label(label: &str) -> Option<Duration> {
    let label = label.trim();
    if let Some(ms) = label.strip_suffix("ms") {
        return ms
            .trim()
            .parse::<f64>()
            .ok()
            .map(|ms| Duration::from_secs_f64(ms / 1_000.0));
    }
    if let Some(seconds) = label.strip_suffix('s') {
        return seconds
            .trim()
            .parse::<f64>()
            .ok()
            .map(Duration::from_secs_f64);
    }
    None
}

/// Render one transcript row at its virtual-list index.
/// The callbacks a row's own controls need, bundled so the renderer's
/// signature stays readable as rows grow more interactive.
#[allow(dead_code)]
pub(crate) struct RowActions<'a> {
    /// Opens or closes one collapsible row.
    pub(crate) toggle: &'a RowToggle,
    /// Jumps to the Diff pane from a write row's badge.
    pub(crate) open_diff: &'a OpenDiff,
    /// Draws an answered approval row's card.
    pub(crate) approval: &'a ApprovalCard,
}

pub(crate) fn render_row(
    row: &TranscriptRow,
    index: usize,
    detail: TranscriptDetail,
    actions: RowActions<'_>,
    // Content that belongs *inside* this row's own card, when the row owns
    // one. A pending approval or question raised by a tool invocation reads as
    // part of that invocation, so it is handed to the card rather than stacked
    // underneath it.
    tail: Option<AnyElement>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let RowActions {
        toggle,
        open_diff,
        approval,
    } = actions;
    let row_id = SharedString::from(format!("transcript-row-{index}"));
    // `.msg` is the prototype's only inserted-row rise. Thinking, tool, and
    // system rows keep their own entrance policy, so sample presence only for
    // the user and assistant bodies.
    let message_enter = match row {
        TranscriptRow::User { .. } | TranscriptRow::Assistant { .. } => Some(
            Presence::new((index, "transcript-msg-enter"), true)
                .transition(message_rise_policy())
                .sample(window, cx),
        ),
        _ => None,
    };
    // Verbose expands every tool card's output, per this enum's contract; a
    // click on one card only toggles that card.
    let verbose = detail == TranscriptDetail::Verbose;

    // Normal mode shows the thinking card too, collapsed: hiding it entirely
    // made a turn look like it reasoned about nothing, and the summary line
    // ("Thought for 8s") is exactly the reassurance a reader wants by default.
    // The card's own `expanded` flag decides whether the body is open.

    match row {
        TranscriptRow::User { text, sent_at } => div()
            .id(row_id)
            .w_full()
            .flex()
            .justify_end()
            .when_some(message_enter, |this, sample| {
                this.opacity(sample.progress)
                    .top(px(5.0 * (1.0 - sample.progress)))
            })
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
        } => {
            let (streamed, _) = streamed_markdown(markdown, *streaming);
            h_flex()
                .id(row_id)
                .w_full()
                .min_w_0()
                .items_start()
                .when_some(message_enter, |this, sample| {
                    this.opacity(sample.progress)
                        .top(px(5.0 * (1.0 - sample.progress)))
                })
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
                        .text_color(cx.theme().accent_foreground)
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
                            StreamingText::new(
                                SharedString::from(format!("assistant-{index}")),
                                &streamed,
                            )
                            .w_full(),
                        )
                        .test_support(),
                )
                .test_support()
                .into_any_element()
        }
        TranscriptRow::Thinking {
            text,
            duration_seconds,
            live,
            expanded,
        } => {
            let live = *live;
            let open = *expanded || live;
            let mut trace = ThinkingTrace::new().prose(text.clone());
            if let Some(seconds) = duration_seconds {
                trace = trace.thought_for(Duration::from_secs(*seconds));
            }
            let trace = if live {
                Progressive::running(trace)
            } else {
                Progressive::complete(trace)
            };
            let toggle = toggle.clone();
            v_flex()
                .id(row_id)
                .test_support()
                .w_full()
                .child(
                    div()
                        .id(SharedString::from(format!("thinking-summary-{index}")))
                        .test_support()
                        .w_full()
                        .child(
                            Thinking::new(
                                SharedString::from(format!("thinking-row-{index}")),
                                &trace,
                            )
                            .open(open)
                            .body_max_height(px(220.))
                            .on_event(move |event, _, cx| match event {
                                ThinkingEvent::Toggled { .. } => toggle(index, cx),
                            })
                            .w_full()
                            .rounded(rems(0.625))
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().muted),
                        ),
                )
                .into_any_element()
        }
        TranscriptRow::Tool { .. } => v_flex()
            .id(row_id)
            .test_support()
            .w_full()
            .child(tool_call_element(
                row, index, verbose, toggle, open_diff, tail, cx,
            ))
            .into_any_element(),
        TranscriptRow::Approval { request, result } => div()
            .id(row_id)
            .w_full()
            .child(approval(request, result, cx))
            .into_any_element(),
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
            .role(gpui_kit::Role::Alert)
            .aria_label(SharedString::from(text.clone()))
            .w_full()
            .min_w_0()
            .items_start()
            .gap_2()
            .rounded(rems(0.625))
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

#[allow(clippy::too_many_arguments)]
fn tool_call_element(
    row: &TranscriptRow,
    index: usize,
    verbose: bool,
    toggle: &RowToggle,
    open_diff: &OpenDiff,
    tail: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let TranscriptRow::Tool {
        display_name,
        detail,
        output,
        duration,
        status,
        expanded,
        visual_kind,
        diff_stats,
        ..
    } = row
    else {
        return div().into_any_element();
    };
    let live = *status == ToolStatus::Running;
    let open = *expanded || verbose || live;
    let mut invocation = ToolInvocation::new(
        SharedString::from(format!("tool-{index}")),
        display_name.clone(),
    )
    .summary(detail.clone())
    .icon(match visual_kind {
        ToolVisualKind::FileRead => IconName::File,
        ToolVisualKind::FileWrite | ToolVisualKind::FileEdit => IconName::FilePenLine,
        _ => IconName::SquareTerminal,
    });
    if !output.trim().is_empty() {
        invocation = invocation.output(output.clone());
    }
    if let Some(elapsed) = parse_duration_label(duration) {
        invocation = invocation.elapsed(elapsed);
    }
    let invocation = match status {
        ToolStatus::Running => Progressive::running(invocation),
        ToolStatus::Succeeded => Progressive::complete(invocation),
        ToolStatus::Failed => Progressive::failed(invocation, "Tool failed"),
    };
    let toggle = toggle.clone();
    let open_diff = open_diff.clone();
    let diff_stats = *diff_stats;

    div()
        .id(SharedString::from(format!("tool-summary-{index}")))
        .test_support()
        .w_full()
        .child(
            div()
                .relative()
                .child(
                    ToolCall::new(&invocation)
                        .open(open)
                        .output_max_height(px(190.))
                        .output_format(ToolOutputFormat::Plain)
                        .on_event(move |event, _, cx| {
                            if let ToolCallEvent::Toggled { .. } = event {
                                toggle(index, cx);
                            }
                        })
                        .children(tail)
                        .w_full()
                        .rounded(rems(0.625))
                        .border_1()
                        .border_color(cx.theme().border)
                        .bg(cx.theme().muted),
                )
                .when_some(diff_stats, |this, (added, removed)| {
                    this.child(
                        h_flex()
                            .id(SharedString::from(format!("tool-diff-{index}")))
                            .test_support()
                            .absolute()
                            .right(px(86.))
                            .top(px(8.))
                            .h(px(22.))
                            .items_center()
                            .gap(px(4.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(cx.theme().border)
                            .bg(cx.theme().popover)
                            .px(px(6.))
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_size(rems(0.625))
                            .text_color(cx.theme().muted_foreground)
                            .cursor_pointer()
                            .on_click(move |_, _, cx| open_diff(cx))
                            .child(
                                div()
                                    .text_color(cx.theme().success)
                                    .child(SharedString::from(format!("+{added}"))),
                            )
                            .child(
                                div()
                                    .text_color(cx.theme().danger)
                                    .child(SharedString::from(format!("\u{2212}{removed}"))),
                            ),
                    )
                }),
        )
        .into_any_element()
}

/// Render a run of consecutive tool calls as one gpui-ai `ToolGroup`.
///
/// The transcript scroller still keeps the first row's identity, so existing
/// scroll anchors stay stable while the tools inside the run share one block
/// and one row gap instead of reading as separate paragraphs.
pub(crate) fn render_tool_group(
    rows: &[TranscriptRow],
    start: usize,
    detail: TranscriptDetail,
    actions: RowActions<'_>,
    // See `render_row`: content that belongs inside the group's card.
    tail: Option<AnyElement>,
    cx: &App,
) -> AnyElement {
    let RowActions {
        toggle,
        open_diff,
        approval: _,
    } = actions;
    let verbose = detail == TranscriptDetail::Verbose;
    let live = rows.iter().any(|row| {
        matches!(
            row,
            TranscriptRow::Tool {
                status: ToolStatus::Running,
                ..
            }
        )
    });
    let children = rows
        .iter()
        .enumerate()
        .map(|(offset, row)| {
            tool_call_element(row, start + offset, verbose, toggle, open_diff, None, cx)
        })
        .chain(tail);

    div()
        .id(SharedString::from(format!("transcript-row-{start}")))
        .test_support()
        .w_full()
        .child(
            div()
                .id(SharedString::from(format!("tool-group-{start}")))
                .test_support()
                .w_full()
                .child(
                    ToolGroup::new(SharedString::from(format!("tool-group-{start}")))
                        .count(rows.len())
                        .active(live)
                        .open(true)
                        .children(children),
                ),
        )
        .into_any_element()
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
        let label = clock_label(1_700_000_000);
        assert_eq!(label.len(), 5, "HH:MM is five columns: {label:?}");
        assert_eq!(label.as_bytes()[2], b':');
        assert_eq!(
            clock_label(crate::session::NO_TIMESTAMP),
            "",
            "a row redrawn from history prints no clock rather than the epoch"
        );
    }
}
