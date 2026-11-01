//! Artifact panel: accessible naming, typed close / view / version / action
//! events, keyboard reach, streaming badge, and bounded scrolling body.

use gpui::{
    Context, Element as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement as _, Render, RenderOnce as _, Role, Styled as _, TestAppContext,
    VisualTestContext, Window, accesskit, canvas, div, point, px, size,
};
use gpui_ai::{
    artifact::{
        Artifact, ArtifactAction, ArtifactKind, ArtifactPanel, ArtifactPanelEvent, ArtifactVersion,
        ArtifactView,
    },
    stream::StreamedContent,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

fn document(source: StreamedContent) -> Artifact {
    Artifact::new("comparison", "Supplier comparison", source)
        .kind(ArtifactKind::Markdown)
        .versions([
            ArtifactVersion::new("v1", "v1"),
            ArtifactVersion::new("v2", "v2"),
            ArtifactVersion::new("v3", "v3"),
        ])
        .active_version("v2")
}

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

struct A11yProbe {
    generating: bool,
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for A11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        let generating = self.generating;
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                let source = if generating {
                    StreamedContent::running("# Draft".to_owned())
                } else {
                    StreamedContent::done("# Done")
                };
                let element = ArtifactPanel::new("panel", &document(source))
                    .on_event(|_, _, _| {})
                    .render(window, cx)
                    .into_element();
                let role = element.a11y_role();
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(generating: bool, cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let (_, cx) = cx.add_window_view({
        let captured = captured.clone();
        move |_, _| A11yProbe {
            generating,
            captured,
        }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    captured
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("probe should capture its element")
}

#[gpui::test]
fn the_panel_is_a_group_named_by_title_kind_and_lifecycle(cx: &mut TestAppContext) {
    let settled = capture(false, cx);
    assert_eq!(settled.role, Some(Role::Group));
    assert_eq!(
        settled.node.label(),
        Some("Artifact: Supplier comparison, Document")
    );
    let generating = capture(true, cx);
    assert_eq!(
        generating.node.label(),
        Some("Artifact: Supplier comparison, Document, generating")
    );
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Settled,
    Generating,
    Short,
}

struct Probe {
    kind: ProbeKind,
    events: Rc<RefCell<Vec<ArtifactPanelEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        let handler = move |event: &ArtifactPanelEvent, _: &mut Window, _: &mut gpui::App| {
            events.borrow_mut().push(event.clone());
        };
        let long_body = (1..=80)
            .map(|line| format!("Paragraph {line} of the comparison, long enough to wrap."))
            .collect::<Vec<_>>()
            .join("\n\n");
        let source = match self.kind {
            ProbeKind::Settled => StreamedContent::done(long_body),
            ProbeKind::Generating => StreamedContent::running(long_body),
            ProbeKind::Short => StreamedContent::done("# Short"),
        };
        div().size_full().p(px(16.)).child(
            div().w(px(420.)).h(px(300.)).child(
                ArtifactPanel::new("panel", &document(source))
                    .view(ArtifactView::Preview)
                    .actions([
                        ArtifactAction::new("open", "Open in editor"),
                        ArtifactAction::new("export", "Export"),
                    ])
                    .on_event(handler),
            ),
        )
    }
}

fn harness(
    kind: ProbeKind,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<ArtifactPanelEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| Probe { kind, events }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(640.), px(480.)));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    (events, cx)
}

fn activate_key(cx: &mut VisualTestContext, key: &str) {
    let keystroke = Keystroke::parse(key).expect("test key should parse");
    cx.simulate_event(KeyDownEvent {
        keystroke: keystroke.clone(),
        is_held: false,
        prefer_character_input: false,
    });
    cx.simulate_event(KeyUpEvent { keystroke });
}

fn click_center(cx: &mut VisualTestContext, selector: &'static str) {
    let bounds = cx
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} should render"));
    cx.simulate_click(bounds.center(), Modifiers::default());
    cx.update(|window, cx| window.draw(cx).clear(cx));
}

#[gpui::test]
fn controls_report_close_versions_views_and_actions(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Settled, cx);
    click_center(cx, "artifact-version-prev-comparison");
    click_center(cx, "artifact-version-next-comparison");
    click_center(cx, "artifact-action-comparison-export");
    let tabs = cx
        .debug_bounds("artifact-tabs-comparison")
        .expect("tabs render");
    cx.simulate_click(
        point(
            tabs.origin.x + tabs.size.width * 0.75,
            tabs.origin.y + tabs.size.height / 2.,
        ),
        Modifiers::default(),
    );
    cx.update(|window, cx| window.draw(cx).clear(cx));
    click_center(cx, "artifact-close-comparison");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            ArtifactPanelEvent::VersionSelected {
                id: "comparison".into(),
                version_id: "v1".into()
            },
            ArtifactPanelEvent::VersionSelected {
                id: "comparison".into(),
                version_id: "v3".into()
            },
            ArtifactPanelEvent::ActionActivated {
                id: "comparison".into(),
                action_id: "export".into()
            },
            ArtifactPanelEvent::ViewSelected {
                id: "comparison".into(),
                view: ArtifactView::Source
            },
            ArtifactPanelEvent::Closed {
                id: "comparison".into()
            },
        ]
    );
}

#[gpui::test]
fn keyboard_reaches_the_version_switcher(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Settled, cx);
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    activate_key(cx, "enter");
    assert_eq!(
        events.borrow().as_slice(),
        &[ArtifactPanelEvent::VersionSelected {
            id: "comparison".into(),
            version_id: "v1".into()
        }]
    );
}

#[gpui::test]
fn generating_artifacts_show_a_live_badge(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Generating, cx);
    assert!(cx.debug_bounds("artifact-generating-comparison").is_some());
    let (_, cx) = harness(ProbeKind::Short, cx);
    assert!(cx.debug_bounds("artifact-generating-comparison").is_none());
}

#[gpui::test]
fn long_bodies_scroll_inside_the_panel_instead_of_growing_it(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Settled, cx);
    let root = cx
        .debug_bounds("artifact-comparison")
        .expect("panel renders");
    assert!(
        root.size.height <= px(300.),
        "panel height {:?}",
        root.size.height
    );
    let body = cx
        .debug_bounds("artifact-body-comparison")
        .expect("body renders");
    assert!(
        body.size.height < px(300.),
        "body height {:?}",
        body.size.height
    );
    assert!(body.size.height > px(60.), "body keeps room to read");
}
