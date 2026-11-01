//! Message queue: prompts waiting their turn while the agent is busy.
//!
//! Agent harnesses let people keep typing while a turn runs; those prompts
//! wait in a queue the application owns. [`MessageQueue`] renders the queue
//! above the composer with position, text, optional note, and controls to
//! reorder, edit, send now, or remove, reporting each as a [`QueueEvent`]
//! keyed by stable ID.

use crate::control::ControlMetricsExt as _;
use crate::{
    ButtonLabelExt as _,
    handlers::SharedHandler,
    motion::{ArrivalRoster, MotionTokens},
    surface::{eyebrow, icon_button, meta},
    theme::SemanticStyledExt as _,
};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, InteractiveElement as _, IntoElement,
    ParentElement as _, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};
use std::rc::Rc;

/// One prompt waiting in the queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueuedMessage {
    id: SharedString,
    text: SharedString,
    note: Option<SharedString>,
}

impl QueuedMessage {
    /// Creates a queued prompt with a stable identifier.
    pub fn new(id: impl Into<SharedString>, text: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            text: text.into(),
            note: None,
        }
    }

    /// Adds a short note ("after the current step", "steering").
    pub fn note(mut self, note: impl Into<SharedString>) -> Self {
        self.note = Some(note.into());
        self
    }

    /// Returns the stable identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the prompt text.
    pub fn text(&self) -> &SharedString {
        &self.text
    }

    /// Returns the note, if any.
    pub fn note_text(&self) -> Option<&SharedString> {
        self.note.as_ref()
    }
}

/// An interaction emitted by [`MessageQueue`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueEvent {
    /// Remove one queued prompt.
    Removed {
        /// Stable message identifier.
        id: SharedString,
    },
    /// Send one queued prompt immediately, ahead of its turn.
    SentNow {
        /// Stable message identifier.
        id: SharedString,
    },
    /// Move one queued prompt one place earlier.
    MovedUp {
        /// Stable message identifier.
        id: SharedString,
    },
    /// Move one queued prompt one place later.
    MovedDown {
        /// Stable message identifier.
        id: SharedString,
    },
    /// Edit one queued prompt before it is sent.
    EditRequested {
        /// Stable message identifier.
        id: SharedString,
    },
    /// Drop every queued prompt.
    Cleared,
}

/// User-visible copy for queued prompts.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
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

/// The queue of waiting prompts.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # fn example(queued: Vec<QueuedMessage>) {
/// MessageQueue::new("queue")
///     .items(queued.iter().cloned())
///     .editable(true)
///     .on_event(|event, _, _| { /* QueueEvent::SentNow { id } … */ });
/// # }
/// ```
#[derive(IntoElement)]
pub struct MessageQueue {
    id: SharedString,
    style: StyleRefinement,
    items: Vec<QueuedMessage>,
    editable: bool,
    on_event: Option<SharedHandler<QueueEvent>>,
    labels: MessageQueueLabels,
}

impl MessageQueue {
    /// Creates an empty queue.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            items: Vec::new(),
            editable: false,
            on_event: None,
            labels: MessageQueueLabels::default(),
        }
    }

    /// Sets the queued prompts, first to send first.
    pub fn items(mut self, items: impl IntoIterator<Item = QueuedMessage>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Offers an edit control per prompt.
    pub fn editable(mut self, editable: bool) -> Self {
        self.editable = editable;
        self
    }

    /// Handles typed interactions. Without a handler the queue is read-only.
    pub fn on_event(
        mut self,
        handler: impl Fn(&QueueEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    /// Replaces user-visible copy for this queue.
    pub fn labels(mut self, labels: MessageQueueLabels) -> Self {
        self.labels = labels;
        self
    }
}

impl Styled for MessageQueue {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for MessageQueue {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = ElementId::from(self.id.clone());
        let debug_id = self.id.to_string();
        let handler = self.on_event;
        let labels = self.labels;
        let count = self.items.len();

        // Rows the queue has already shown stay at rest; stable IDs queued
        // onto a mounted surface settle in on the capped cascade, and the
        // restored initial snapshot joins at rest. Reorders exchange no
        // motion state — Spike D's measured reorder was reverted because a
        // layout offset paints one frame at the destination first, so a
        // moved row simply lands where its controls put it.
        let motion = MotionTokens::read(cx).clone();
        let roster = window.use_keyed_state((root_id.clone(), "arrivals"), cx, |_, _| {
            ArrivalRoster::new()
        });
        roster.update(cx, |roster, cx| {
            roster.note(
                self.items.iter().map(|item| {
                    ElementId::Name(SharedString::from(format!("queue-arrive-{}", item.id)))
                }),
                true,
                &motion,
                cx.background_executor().now(),
            );
        });

        if count == 0 {
            return div().id(root_id).hidden().refine_style(&self.style);
        }

        let mut rows = Vec::with_capacity(count);
        for (index, item) in self.items.iter().enumerate() {
            let arrival = roster.update(cx, |roster, cx| {
                roster.progress(
                    &ElementId::Name(SharedString::from(format!("queue-arrive-{}", item.id))),
                    window,
                    cx,
                )
            });
            rows.push(render_row(
                &root_id,
                &debug_id,
                index,
                count,
                item,
                self.editable,
                arrival,
                &labels,
                handler.clone(),
                window,
                cx,
            ));
        }
        let clear = handler.clone().map(|handler| {
            let clear_debug = debug_id.clone();
            div()
                .debug_selector(move || format!("queue-clear-{clear_debug}"))
                .child(
                    Button::new((root_id.clone(), "clear"))
                        .ghost()
                        .xsmall()
                        .control_metrics(cx)
                        .accessibility_id(format!("{debug_id}-clear"))
                        .text_label(labels.clear.clone())
                        .on_click(move |_: &ClickEvent, window, cx| {
                            handler(&QueueEvent::Cleared, window, cx)
                        }),
                )
        });
        let root_debug = debug_id.clone();

        v_flex()
            .id(root_id)
            .role(Role::List)
            .aria_label(format!(
                "{}, {count} {}",
                labels.queued, labels.waiting
            ))
            .debug_selector(move || format!("queue-{root_debug}"))
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.xs)
            .p(tokens.spacing.sm)
            .bg(cx.theme().muted.opacity(0.35))
            .border_1()
            .border_color(cx.theme().border)
            .rounded(tokens.radius.lg)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap(tokens.spacing.sm)
                    .child(eyebrow(format!("{} · {count}", labels.queued), cx))
                    .children(clear),
            )
            .children(rows)
            .refine_style(&self.style)
    }
}

#[allow(clippy::too_many_arguments)]
fn render_row(
    root_id: &ElementId,
    queue_debug: &str,
    index: usize,
    count: usize,
    item: &QueuedMessage,
    editable: bool,
    arrival: Option<f32>,
    labels: &MessageQueueLabels,
    handler: Option<SharedHandler<QueueEvent>>,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let tokens = cx.theme().semantic_tokens();
    let row_id = ElementId::from((root_id.clone(), format!("item-{}", item.id)));
    let label: SharedString = format!(
        "{} {} {} {count}: {}",
        labels.queued_item,
        index + 1,
        labels.of,
        item.text
    )
    .into();

    let mut control = |suffix: &'static str,
                       icon: IconName,
                       name: String,
                       enabled: bool,
                       event: QueueEvent,
                       cx: &mut App| {
        let handler = handler.clone();
        let row_debug = item.id.clone();
        icon_button((row_id.clone(), suffix), icon, name, window, cx)
            .disabled(!enabled || handler.is_none())
            .debug_selector(move || format!("queue-{suffix}-{queue_debug}-{row_debug}"))
            .on_click(move |_: &ClickEvent, window, cx| {
                if let Some(handler) = &handler {
                    handler(&event, window, cx)
                }
            })
    };
    let id = item.id.clone();
    let controls = h_flex()
        .flex_none()
        .items_center()
        .gap(tokens.spacing.xxs)
        .child(control(
            "up",
            IconName::ChevronUp,
            format!("{}: {}", labels.move_earlier, item.text),
            index > 0,
            QueueEvent::MovedUp { id: id.clone() },
            cx,
        ))
        .child(control(
            "down",
            IconName::ChevronDown,
            format!("{}: {}", labels.move_later, item.text),
            index + 1 < count,
            QueueEvent::MovedDown { id: id.clone() },
            cx,
        ))
        .when(editable, |this| {
            this.child(control(
                "edit",
                IconName::Replace,
                format!("{}: {}", labels.edit, item.text),
                true,
                QueueEvent::EditRequested { id: id.clone() },
                cx,
            ))
        })
        .child(control(
            "send",
            IconName::ArrowUp,
            format!("{}: {}", labels.send_now, item.text),
            true,
            QueueEvent::SentNow { id: id.clone() },
            cx,
        ))
        .child(control(
            "remove",
            IconName::Close,
            format!("{}: {}", labels.remove, item.text),
            true,
            QueueEvent::Removed { id: id.clone() },
            cx,
        ));
    let row_debug = item.id.clone();
    // The position number seats on the text's first line while the
    // action cluster centers itself against the whole row; the note
    // truncates like the text above it instead of stretching the row.
    let row = h_flex()
        .id(row_id.clone())
        .role(Role::ListItem)
        .aria_label(label)
        .debug_selector(move || format!("queue-item-{queue_debug}-{row_debug}"))
        .items_start()
        .w_full()
        .min_w_0()
        .gap(tokens.spacing.sm)
        .px(tokens.spacing.sm)
        .py(tokens.spacing.xs)
        .rounded(tokens.radius.sm)
        // The surface step against the container's muted wash separates
        // the row on its own; a second border inside the framed list was
        // the double the audit flagged.
        .bg(tokens.colors.surface)
        .child(crate::surface::leading_glyph_slot(
            crate::sizing::slot_md(cx),
            meta((index + 1).to_string(), cx),
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w_0()
                .flex_1()
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().foreground)
                        .child(item.text.clone()),
                )
                .when_some(item.note.clone(), |this, note| {
                    this.child(crate::surface::hint(note, cx).min_w_0().truncate())
                }),
        )
        .child(controls.self_center());
    let row = match arrival {
        Some(progress) => row
            .opacity(progress)
            .top(tokens.spacing.xxs * (1.0 - progress) * crate::motion::travel(cx)),
        None => row,
    };
    div().w_full().min_w_0().child(row).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, VisualTestContext, px};

    struct QueueProbe {
        count: usize,
    }

    impl Render for QueueProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(360.))
                .h(px(320.))
                .child(
                    MessageQueue::new("probe-queue").items(
                        (0..self.count)
                            .map(|ix| QueuedMessage::new(format!("q-{ix}"), "Queued prompt")),
                    ),
                )
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    #[gpui::test]
    fn a_restored_queue_rests_and_a_newly_queued_prompt_settles_in(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| QueueProbe { count: 3 });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a restored queue joins at rest"
        );

        probe.update(cx, |probe, cx| {
            probe.count = 4;
            cx.notify();
        });
        draw(cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "a prompt queued onto a mounted surface must settle in"
        );

        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        draw(cx);
        crate::motion::take_reveal_frame_requests();
        probe.update(cx, |_, cx| cx.notify());
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a re-render must not replay an acknowledged arrival"
        );
    }

    struct SiblingQueues {
        counts: [usize; 2],
    }

    impl Render for SiblingQueues {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            v_flex().children(self.counts.iter().enumerate().map(|(owner, count)| {
                MessageQueue::new(format!("queue-{owner}")).items(
                    (0..*count).map(|item| QueuedMessage::new(format!("item-{item}"), "Prompt")),
                )
            }))
        }
    }

    #[gpui::test]
    fn sibling_queues_do_not_share_arrival_clocks(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| SiblingQueues { counts: [1, 1] });
        draw(cx);
        probe.update(cx, |probe, cx| {
            probe.counts[0] = 2;
            cx.notify();
        });
        draw(cx);
        cx.executor()
            .advance_clock(std::time::Duration::from_secs(2));
        draw(cx);
        crate::motion::take_reveal_frame_requests();

        probe.update(cx, |probe, cx| {
            probe.counts[1] = 2;
            cx.notify();
        });
        draw(cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "the second queue's arrival must not inherit the first queue's settled clock"
        );
    }

    #[test]
    fn notes_are_optional() {
        let item = QueuedMessage::new("a", "Compare prices").note("after the current step");
        assert_eq!(
            item.note_text().map(|n| n.as_ref()),
            Some("after the current step")
        );
        assert!(QueuedMessage::new("b", "x").note_text().is_none());
    }
}
