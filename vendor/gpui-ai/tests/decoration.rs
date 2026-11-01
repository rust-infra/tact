//! The contract a framed component owes a decoration.
//!
//! Paint order is child order, so the under layer has to be a frame's first
//! child and the over layer its last — two placements a component makes by
//! hand and can therefore forget. This is what makes forgetting a failure
//! rather than a thing nobody notices until an application reports that half
//! its decoration never appeared.

use gpui::{
    App, Context, InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _,
    TestAppContext, VisualTestContext, Window, div, px,
};
use gpui_ai::prelude::{
    ApprovalCard, CodeBlock, CodeDiff, ContextCard, Decoration, DiffFile, PlanCard, Question,
    QuestionFlow, RecommendationCard, SearchResults, TodoList,
};
use gpui_ai::prelude::{Artifact, ArtifactPanel, StreamedContent};

/// A marker pair a component must place, both filling its frame.
fn layers() -> Decoration {
    Decoration::behind(
        div()
            .size_full()
            .debug_selector(|| "decoration-under".to_owned()),
    )
    .and_above(
        div()
            .size_full()
            .debug_selector(|| "decoration-over".to_owned()),
    )
}

/// Builds the one component under test.
type Build = Box<dyn Fn(&mut Window, &mut App) -> gpui::AnyElement>;

/// One component under test, rendered on its own.
struct Probe {
    build: Build,
}

impl Render for Probe {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .w(px(520.))
            .debug_selector(|| "probe-frame".to_owned())
            .child((self.build)(window, cx))
    }
}

fn check(cx: &mut TestAppContext, name: &str, build: impl Fn() -> gpui::AnyElement + 'static) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(move |_, _| Probe {
        build: Box::new(move |_, _| build()),
    });
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    let under = cx.debug_bounds("decoration-under");
    let over = cx.debug_bounds("decoration-over");
    assert!(under.is_some(), "{name} never places the under layer");
    assert!(over.is_some(), "{name} never places the over layer");

    // Both fill the frame rather than sitting in the content flow. A layer
    // that took part in layout would move the component it decorates.
    let (under, over) = (under.expect("under"), over.expect("over"));
    assert!(
        under.size.width > px(0.) && under.size.height > px(0.),
        "{name}'s under layer has no area"
    );
    assert_eq!(
        (under.origin, under.size),
        (over.origin, over.size),
        "{name}'s two layers should cover the same frame"
    );
}

#[gpui::test]
fn every_framed_component_places_both_decoration_layers(cx: &mut TestAppContext) {
    check(cx, "CodeBlock", || {
        CodeBlock::new("code", "fn main() {}")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "TodoList", || {
        TodoList::new("todos")
            .title("Plan")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "ApprovalCard", || {
        ApprovalCard::new("gate", "Delete the branch?")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "PlanCard", || {
        PlanCard::new("plan", "Launch plan")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "RecommendationCard", || {
        RecommendationCard::new("pick", "Alpenrose Dairy")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "ContextCard", || {
        ContextCard::new("source", "pricing.md")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "SearchResults", || {
        SearchResults::new("search", "gpui components")
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "CodeDiff", || {
        CodeDiff::new("diff", &DiffFile::new("main.rs"))
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "QuestionFlow", || {
        QuestionFlow::new("flow", "Before I start")
            .questions([Question::new("q", "Which one?")])
            .decoration(layers())
            .into_any_element()
    });
    check(cx, "ArtifactPanel", || {
        let artifact = Artifact::new(
            "doc",
            "Report",
            StreamedContent::complete("# Report".to_owned()),
        );
        ArtifactPanel::new("panel", &artifact)
            .decoration(layers())
            .into_any_element()
    });
}

/// A component with no decoration adds no elements for one.
///
/// The slot is opt-in, and the cost of not using it should be exactly zero —
/// not an empty absolutely-positioned wrapper per frame, on every card, in
/// every list, forever.
#[gpui::test]
fn an_undecorated_component_places_nothing(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let (_, cx) = cx.add_window_view(move |_, _| Probe {
        build: Box::new(|_, _| CodeBlock::new("code", "fn main() {}").into_any_element()),
    });
    let cx: &mut VisualTestContext = cx;
    cx.update(|window, cx| window.draw(cx).clear(cx));

    assert!(cx.debug_bounds("decoration-under").is_none());
    assert!(cx.debug_bounds("decoration-over").is_none());
}
