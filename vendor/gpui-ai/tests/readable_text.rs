use gpui::{
    AnyElement, AnyView, AppContext as _, Context, InteractiveElement as _, IntoElement, Modifiers,
    MouseButton, ParentElement as _, Render, Styled as _, TestAppContext, VisualTestContext,
    Window, div, point, prelude::FluentBuilder as _, px,
};
use gpui_ai::prelude::{
    Chat, ChatMessage, ChatRole, DiffCell, DiffChangeKind, DiffColumn, DiffRow, DiffTable,
    PromptBar, RecordCell, RecordColumn, RecordRow, RecordsTable,
};
use gpui_ai::{
    code_block::CodeBlock,
    insight::InsightCard,
    selection_actions::{SelectionAction, SelectionActions},
    stream::Progressive,
    streaming_text::{CitationRef, FollowUp, StreamingText, StreamingTextEvent},
    thinking::{StepStatus, Thinking, ThinkingStep, ThinkingTrace},
};
use std::{cell::RefCell, rc::Rc, sync::Arc};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Surface {
    Answer,
    CitationAnswer,
    CitationLabel,
    Code,
    ThinkingProse,
    ThinkingDetail,
    Insight,
    SelectionActions,
    ChatUser,
}

struct ReadableSurface {
    surface: Surface,
    selection: Option<gpui::Entity<SelectionActions>>,
    chat: Option<gpui::Entity<Chat>>,
}

struct FollowUpSelectionSurface {
    events: Rc<RefCell<Vec<StreamingTextEvent>>>,
}

struct RecordsReadableSurface {
    table: gpui::Entity<RecordsTable>,
}

struct DiffReadableSurface {
    table: gpui::Entity<DiffTable>,
}

impl DiffReadableSurface {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table = cx.new(|cx| DiffTable::new("readable-diff", "Readable diff", window, cx));
        table.update(cx, |table, cx| {
            table.set_columns(
                [DiffColumn::new("value", "Value").width(px(640.))],
                window,
                cx,
            );
            table.set_rows(
                Progressive::complete(Arc::from([DiffRow::new(
                    "proposal",
                    "Proposal",
                    DiffChangeKind::Changed,
                )
                .cells([DiffCell::changed(
                    "value",
                    "selectable_before *literal*",
                    "*literal* selectable_after",
                )])])),
                window,
                cx,
            );
        });
        Self { table }
    }
}

impl Render for DiffReadableSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(px(16.)).child(
            div()
                .debug_selector(|| "readable-diff-surface".into())
                .w_full()
                .h(px(160.))
                .child(self.table.clone()),
        )
    }
}

impl RecordsReadableSurface {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let table =
            cx.new(|cx| RecordsTable::new("readable-records", "Readable records", window, cx));
        table.update(cx, |table, cx| {
            table.set_columns(
                [RecordColumn::new("company", "Company").width(px(560.))],
                window,
                cx,
            );
            table.set_records(
                Progressive::complete(Arc::from([RecordRow::new("supplier", "Supplier").cells([
                    RecordCell::new("company", "selectable_record *literal* selectable_record"),
                ])])),
                window,
                cx,
            );
        });
        Self { table }
    }
}

impl Render for RecordsReadableSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size_full().p(px(16.)).child(
            div()
                .debug_selector(|| "readable-records-surface".into())
                .w_full()
                .h(px(160.))
                .child(self.table.clone()),
        )
    }
}

struct SelectionTestRoot {
    view: AnyView,
}

impl SelectionTestRoot {
    fn new(view: impl Into<AnyView>) -> Self {
        Self { view: view.into() }
    }
}

impl Render for SelectionTestRoot {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
            .id("selection-test-root")
            .relative()
            .size_full()
            .child(gpui_base::TextSelectionLayer)
            .child(self.view.clone())
    }
}

impl Render for FollowUpSelectionSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let content = Progressive::complete(
            "Selectable supplier comparison remains available while choosing the next step."
                .to_owned(),
        );
        let events = self.events.clone();
        div().size_full().p(px(16.)).child(
            div()
                .debug_selector(|| "follow-up-selection-surface".into())
                .w_full()
                .child(
                    StreamingText::new("follow-up-selection", &content)
                        .follow_ups([FollowUp::new("compare", "Compare suppliers")])
                        .on_event(move |event, _, _| events.borrow_mut().push(event.clone())),
                ),
        )
    }
}

impl ReadableSurface {
    fn new(surface: Surface, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let selection = (surface == Surface::SelectionActions).then(|| {
            let selection = cx.new(|cx| {
                SelectionActions::new(
                    "readable-selection",
                    "selectable_action selectable_action",
                    window,
                    cx,
                )
            });
            selection.update(cx, |selection, cx| {
                selection.set_actions([SelectionAction::new("ask", "Ask")], cx)
            });
            selection
        });
        let chat = (surface == Surface::ChatUser).then(|| {
            let prompt = cx.new(|cx| PromptBar::new("readable-chat-prompt", window, cx));
            let chat = cx.new(|cx| Chat::new("readable-chat", prompt, window, cx));
            chat.update(cx, |chat, cx| {
                chat.set_messages(
                    Arc::from([ChatMessage::new(
                        "readable-chat-user",
                        ChatRole::User,
                        Progressive::complete(
                            "selectable_chat_message selectable_chat_message".to_owned(),
                        ),
                    )]),
                    window,
                    cx,
                );
            });
            chat
        });
        Self {
            surface,
            selection,
            chat,
        }
    }
}

impl Render for ReadableSurface {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let surface: AnyElement = match self.surface {
            Surface::Answer => {
                let content = Progressive::complete("Selectable answer text".to_owned());
                StreamingText::new("answer", &content).into_any_element()
            }
            Surface::CitationAnswer => {
                let content = Progressive::complete(
                    "Readable prose cites [[cite:pricing]] without losing selection.".to_owned(),
                );
                StreamingText::new("citation-answer", &content)
                    .citations([CitationRef::new(
                        "pricing",
                        "Pricing report",
                        "Open the pricing report",
                        "app://reports/pricing",
                    )])
                    .on_event(|_, _, _| {})
                    .into_any_element()
            }
            Surface::CitationLabel => {
                let content = Progressive::complete(
                    "Readable prose cites [[cite:pricing]] without a handler.".to_owned(),
                );
                StreamingText::new("citation-label", &content)
                    .citations([CitationRef::new(
                        "pricing",
                        "Pricing report",
                        "Open the pricing report",
                        "app://reports/pricing",
                    )])
                    .into_any_element()
            }
            Surface::Code => CodeBlock::new("code", "selectable_code selectable_code")
                .language("rust")
                .into_any_element(),
            Surface::ThinkingProse => {
                let trace =
                    Progressive::complete(ThinkingTrace::new().prose("Selectable reasoning prose"));
                Thinking::new("thinking-prose", &trace)
                    .open(true)
                    .into_any_element()
            }
            Surface::ThinkingDetail => {
                let trace = Progressive::complete(
                    ThinkingTrace::new().steps([ThinkingStep::new("Inspect evidence")
                        .detail("Selectable reasoning detail")
                        .status(StepStatus::Done)]),
                );
                Thinking::new("thinking-detail", &trace)
                    .open(true)
                    .into_any_element()
            }
            Surface::Insight => InsightCard::new("insight", "Demand changed")
                .body("selectable_insight selectable_insight")
                .into_any_element(),
            Surface::SelectionActions => self
                .selection
                .as_ref()
                .expect("selection entity is created for the selection surface")
                .clone()
                .into_any_element(),
            Surface::ChatUser => self
                .chat
                .as_ref()
                .expect("chat entity is created for the chat surface")
                .clone()
                .into_any_element(),
        };

        div().size_full().p(px(16.)).child(
            div()
                .debug_selector(|| "readable-surface".into())
                .w_full()
                .when(self.surface == Surface::ChatUser, |surface| {
                    surface.h(px(360.))
                })
                .child(surface),
        )
    }
}

fn select_text(surface: Surface, cx: &mut TestAppContext) -> String {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(move |window, cx| {
        let content = cx.new(|cx| ReadableSurface::new(surface, window, cx));
        SelectionTestRoot::new(content)
    });
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let bounds = cx
        .debug_bounds(if surface == Surface::ChatUser {
            // The semantic row spans the transcript, but trailing messages
            // now paint a bounded bubble. Drag inside the visible selectable
            // surface instead of relying on the former full-row background.
            "chat-message-bubble-readable-chat-user"
        } else {
            "readable-surface"
        })
        .expect("readable surface should be rendered");
    let (content_x, content_y) = match surface {
        Surface::Answer | Surface::CitationAnswer | Surface::CitationLabel => {
            (bounds.left() + px(1.), bounds.top() + px(1.))
        }
        Surface::Code => (
            bounds.left() + px(48.),
            bounds.top() + bounds.size.height * 0.68,
        ),
        Surface::ThinkingProse => (
            bounds.left() + px(21.),
            bounds.top() + bounds.size.height * 0.70,
        ),
        Surface::ThinkingDetail => (
            bounds.left() + px(37.),
            bounds.top() + bounds.size.height * 0.74,
        ),
        Surface::Insight => (bounds.left() + px(17.), bounds.top() + px(80.)),
        Surface::SelectionActions => (bounds.left() + px(14.), bounds.top() + px(14.)),
        Surface::ChatUser => (bounds.left() + px(14.), bounds.top() + px(48.)),
    };
    let from = point(content_x, content_y);
    let to = if surface == Surface::ChatUser {
        point(bounds.right() - px(14.), bounds.bottom() - px(14.))
    } else {
        point(bounds.left() + px(600.), bounds.bottom() - px(1.))
    };

    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });

    cx.update(gpui_base::TextSelection::selected_text)
}

#[gpui::test]
fn streamed_answers_export_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::Answer, cx);

    assert!(selected.contains("Selectable answer text"), "{selected:?}");
}

#[gpui::test]
fn inline_citations_preserve_readable_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::CitationAnswer, cx);

    assert!(selected.contains("Readable prose cites"), "{selected:?}");
    assert!(selected.contains("Pricing report"), "{selected:?}");
    assert!(!selected.contains("cite:pricing"), "{selected:?}");
    assert!(!selected.contains("gpui-ai-citation"), "{selected:?}");
}

#[gpui::test]
fn citations_without_handlers_render_readable_non_link_labels(cx: &mut TestAppContext) {
    let selected = select_text(Surface::CitationLabel, cx);

    assert!(selected.contains("[Pricing report]"), "{selected:?}");
    assert!(!selected.contains("cite:pricing"), "{selected:?}");
    assert!(!selected.contains("gpui-ai-citation"), "{selected:?}");
}

#[gpui::test]
fn code_blocks_export_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::Code, cx);

    assert!(selected.contains("selectable_code"), "{selected:?}");
}

#[gpui::test]
fn thinking_prose_exports_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::ThinkingProse, cx);

    assert!(
        selected.contains("Selectable reasoning prose"),
        "{selected:?}"
    );
}

#[gpui::test]
fn thinking_details_export_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::ThinkingDetail, cx);

    assert!(selected.contains("reasoning detail"), "{selected:?}");
}

#[gpui::test]
fn insight_bodies_export_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::Insight, cx);

    assert!(selected.contains("selectable_insight"), "{selected:?}");
}

#[gpui::test]
fn selection_action_surfaces_export_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::SelectionActions, cx);

    assert!(selected.contains("selectable_action"), "{selected:?}");
}

#[gpui::test]
fn chat_user_prose_exports_selected_text(cx: &mut TestAppContext) {
    let selected = select_text(Surface::ChatUser, cx);

    assert!(selected.contains("selectable_chat_message"), "{selected:?}");
}

#[gpui::test]
fn record_cells_export_literal_selected_text(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| RecordsReadableSurface::new(window, cx));
        SelectionTestRoot::new(content)
    });
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let cell = cx
        .debug_bounds("records-cell-16:readable-records8:suppliercompany")
        .expect("the readable record cell should render");
    let from = point(cell.left() + px(4.), cell.top() + px(4.));
    let to = point(cell.right() - px(4.), cell.bottom() - px(4.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let selected = cx.update(gpui_base::TextSelection::selected_text);
    assert!(
        selected.contains("selectable_record *literal* selectable_record"),
        "{selected:?}"
    );
}

#[gpui::test]
fn diff_before_and_after_values_export_literal_selected_text(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(|window, cx| {
        let content = cx.new(|cx| DiffReadableSurface::new(window, cx));
        SelectionTestRoot::new(content)
    });
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let cell = cx
        .debug_bounds("records-cell-40:diff-table-records-13:readable-difftable8:proposalvalue")
        .expect("the readable diff cell should render");
    // The drag starts past the change cell's tone-dot gutter so the first
    // point already touches text: a drag that only ever crosses blank
    // space is deliberately treated as no selection at all.
    let from = point(cell.left() + px(10.), cell.top() + px(4.));
    let to = point(cell.right() - px(4.), cell.bottom() - px(4.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let selected = cx.update(gpui_base::TextSelection::selected_text);
    // Both values export as the literal text they show. They share one
    // selectable view — with the arrow between them — so a drag anywhere
    // in the cell stays anchored to a single selection participant, and
    // assistive technology still reads the change as a sentence.
    assert!(
        selected.contains("selectable_before *literal*"),
        "{selected:?}"
    );
    assert!(selected.contains("selectable_after"), "{selected:?}");
}

#[gpui::test]
fn follow_up_pointer_drag_does_not_select_streaming_prose(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let captured = events.clone();
    let (_, cx) = cx.add_window_view(move |_, cx| {
        let content = cx.new(|_| FollowUpSelectionSurface { events });
        SelectionTestRoot::new(content)
    });
    let cx: &mut VisualTestContext = cx;
    cx.run_until_parked();
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let prose = cx
        .debug_bounds("streaming-text-body")
        .expect("selectable streaming prose should render");
    let from = point(prose.left() + px(1.), prose.top() + px(1.));
    let to = point(prose.left() + px(600.), prose.bottom() - px(1.));
    cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_mouse_move(to, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, cx| {
        let _ = window.draw(cx);
    });
    cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let selected = cx.update(gpui_base::TextSelection::selected_text);
    assert!(
        selected.contains("Selectable supplier comparison"),
        "{selected:?}"
    );

    let follow_up = cx
        .debug_bounds("streaming-follow-up-compare")
        .expect("rendered follow-up should remain reachable");
    cx.simulate_mouse_down(follow_up.center(), MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));

    // The pinned window selection layer clears an existing selection during
    // capture. The control must still own the press before the bubble-phase
    // selection handler can begin a new drag from the control into prose.
    cx.simulate_mouse_move(from, Some(MouseButton::Left), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let during_drag = cx.update(gpui_base::TextSelection::selected_text);
    assert!(
        during_drag.is_empty(),
        "dragging from a follow-up selected transcript text: {during_drag:?}"
    );

    cx.simulate_mouse_up(from, MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        captured.borrow().is_empty(),
        "dragging out must not activate"
    );

    cx.simulate_mouse_down(follow_up.center(), MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    cx.simulate_mouse_up(follow_up.center(), MouseButton::Left, Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        captured.borrow().as_slice(),
        &[StreamingTextEvent::FollowUpSelected {
            id: "compare".into(),
        }]
    );
}
