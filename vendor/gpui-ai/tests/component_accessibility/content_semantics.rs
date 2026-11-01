//! The AccessKit node contract the content components write.
//!
//! Each component is rendered into a bare `accesskit::Node` through a canvas,
//! so role, label, and description are read as a screen reader would receive
//! them — without layout, pointer, or focus behaviour standing in between.

use gpui::{
    Element as _, IntoElement as _, Render, RenderOnce as _, Role, TestAppContext, Window,
    accesskit, canvas,
};
use gpui_ai::{
    approval::ApprovalCard,
    code_block::CodeBlock,
    insight::{InsightCard, InsightMetric, InsightPoint},
    recommendation::RecommendationCard,
    search_results::{SearchResult, SearchResults},
    stream::Progressive,
    streaming_text::StreamingText,
    task::{TaskRow, TaskSnapshot},
    thinking::{Thinking, ThinkingTrace},
    todo_list::{TodoItem, TodoList},
};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum ComponentProbeKind {
    Approval,
    Recommendation,
    Task,
    Code,
    Search,
    Todo,
    StreamingText,
    Thinking,
    Insight,
}

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

struct ComponentProbe {
    kind: ComponentProbeKind,
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for ComponentProbe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let kind = self.kind;
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                macro_rules! write_element {
                    ($element:expr) => {{
                        let element = $element.into_element();
                        let role = element.a11y_role();
                        element.write_a11y_info(&mut node);
                        role
                    }};
                }
                let role = match kind {
                    ComponentProbeKind::Approval => write_element!(
                        ApprovalCard::new("approval", "Send supplier confirmations?")
                            .description("This action cannot be recalled")
                            .render(window, cx)
                    ),
                    ComponentProbeKind::Recommendation => write_element!(
                        RecommendationCard::new("recommendation", "Choose Alpenrose Dairy")
                            .description("Lowest price at the required volume")
                            .render(window, cx)
                    ),
                    ComponentProbeKind::Task => write_element!(
                        TaskRow::new(&Progressive::failed(
                            TaskSnapshot::new("index", "Index repository").detail("3,214 files"),
                            "Disk unavailable",
                        ))
                        .render(window, cx)
                    ),
                    ComponentProbeKind::Code => write_element!(
                        CodeBlock::new("code", "fn main() {}")
                            .language("rust")
                            .render(window, cx)
                    ),
                    ComponentProbeKind::Search => write_element!(
                        SearchResults::new("search", "gpui wasm")
                            .results([SearchResult::new("result", "GPUI Component")])
                            .render(window, cx)
                    ),
                    ComponentProbeKind::Todo => write_element!(
                        TodoList::new("todo")
                            .title("Migration plan")
                            .items([
                                TodoItem::new("read", "Read schema").done(),
                                TodoItem::new("write", "Write migration"),
                            ])
                            .render(window, cx)
                    ),
                    ComponentProbeKind::StreamingText => write_element!(
                        StreamingText::new("answer", &Progressive::complete("Forty-two".into()))
                            .render(window, cx)
                    ),
                    ComponentProbeKind::Thinking => write_element!(
                        Thinking::new("thinking", &Progressive::running(ThinkingTrace::new()))
                            .render(window, cx)
                    ),
                    ComponentProbeKind::Insight => write_element!(
                        InsightCard::new("insight", "Demand changed")
                            .page(2, 3)
                            .metrics([InsightMetric::new("mint", "Mint Chip", "$2,377.66")])
                            .series(
                                "Weekly demand",
                                [
                                    InsightPoint::new("Mon", 18.0),
                                    InsightPoint::new("Tue", 24.0)
                                ],
                            )
                            .chart_summary("Weekly demand rose from 18 to 24 orders.")
                            .follow_up("Rebalance flavors")
                            .on_event(|_, _, _| {})
                            .render(window, cx)
                    ),
                };
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(kind: ComponentProbeKind, cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ComponentProbe { kind, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("component node should be captured")
}

#[gpui::test]
fn content_cards_are_named_groups(cx: &mut TestAppContext) {
    let approval = capture(ComponentProbeKind::Approval, cx);
    assert_eq!(approval.role, Some(Role::Group));
    assert_eq!(approval.node.label(), Some("Send supplier confirmations?"));
    assert_eq!(
        approval.node.description(),
        Some("This action cannot be recalled")
    );

    let recommendation = capture(ComponentProbeKind::Recommendation, cx);
    assert_eq!(recommendation.role, Some(Role::Group));
    assert_eq!(recommendation.node.label(), Some("Choose Alpenrose Dairy"));
    assert_eq!(
        recommendation.node.description(),
        Some("Lowest price at the required volume")
    );

    let insight = capture(ComponentProbeKind::Insight, cx);
    assert_eq!(insight.role, Some(Role::Group));
    assert_eq!(insight.node.label(), Some("Demand changed"));
    assert_eq!(
        insight.node.description(),
        Some("Insights · 2 of 3. Weekly demand rose from 18 to 24 orders.")
    );
}

#[gpui::test]
fn task_state_is_available_without_color_or_icons(cx: &mut TestAppContext) {
    let task = capture(ComponentProbeKind::Task, cx);
    assert_eq!(task.role, Some(Role::ListItem));
    assert_eq!(task.node.label(), Some("Index repository, failed"));
    assert_eq!(task.node.description(), Some("Disk unavailable"));
}

#[gpui::test]
fn composite_content_has_a_named_semantic_boundary(cx: &mut TestAppContext) {
    let code = capture(ComponentProbeKind::Code, cx);
    assert_eq!(code.role, Some(Role::Code));
    assert_eq!(code.node.label(), Some("rust code"));

    let search = capture(ComponentProbeKind::Search, cx);
    assert_eq!(search.role, Some(Role::Search));
    assert_eq!(search.node.label(), Some("1 result for “gpui wasm”"));

    let todo = capture(ComponentProbeKind::Todo, cx);
    assert_eq!(todo.role, Some(Role::List));
    assert_eq!(todo.node.label(), Some("Migration plan, 1 of 2 complete"));

    let answer = capture(ComponentProbeKind::StreamingText, cx);
    assert_eq!(answer.role, Some(Role::Article));
    assert_eq!(answer.node.label(), Some("Answer"));

    let thinking = capture(ComponentProbeKind::Thinking, cx);
    assert_eq!(thinking.role, Some(Role::Group));
    assert_eq!(thinking.node.label(), Some("Thinking…"));
}
