//! Code diff viewer: accessible naming, typed review events, keyboard reach,
//! gutter/code alignment, and bounded width for long lines.

use gpui::{
    Context, Element as _, IntoElement as _, KeyDownEvent, KeyUpEvent, Keystroke, Modifiers,
    ParentElement as _, Render, RenderOnce as _, Role, Styled as _, TestAppContext,
    VisualTestContext, Window, accesskit, canvas, div, px, size,
};
use gpui_ai::code_diff::{
    CodeDiff, CodeDiffEvent, DiffFile, DiffHunk, DiffLine, DiffLineKind, HunkReview,
};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

const PATCH: &str = "--- a/src/pricing.rs\n+++ b/src/pricing.rs\n@@ -1,4 +1,5 @@ fn unit_price\n fn unit_price(order: &Order) -> Money {\n-    order.total / order.units\n+    let units = order.units.max(1);\n+    order.total / units\n }\n@@ -10,3 +11,3 @@\n fn discount() -> f32 {\n-    0.05\n+    0.07\n }\n";

fn file() -> DiffFile {
    DiffFile::from_unified(PATCH).remove(0)
}

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

struct A11yProbe {
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for A11yProbe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::Unknown);
                let element = CodeDiff::new("diff", &file())
                    .reviewable(true)
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

#[gpui::test]
fn the_diff_is_a_group_named_by_path_and_stats(cx: &mut TestAppContext) {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let (_, cx) = cx.add_window_view({
        let captured = captured.clone();
        move |_, _| A11yProbe { captured }
    });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    let captured = captured
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("probe should capture its element");
    assert_eq!(captured.role, Some(Role::Group));
    assert_eq!(
        captured.node.label(),
        Some("Diff of src/pricing.rs, +3 \u{2212}2")
    );
}

#[derive(Clone, Copy)]
enum ProbeKind {
    Reviewable,
    Resolved,
    Collapsed,
    Narrow,
}

struct Probe {
    kind: ProbeKind,
    events: Rc<RefCell<Vec<CodeDiffEvent>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let events = self.events.clone();
        let handler = move |event: &CodeDiffEvent, _: &mut Window, _: &mut gpui::App| {
            events.borrow_mut().push(event.clone());
        };
        match self.kind {
            ProbeKind::Reviewable => div().size_full().child(
                CodeDiff::new("diff", &file())
                    .reviewable(true)
                    .on_event(handler),
            ),
            ProbeKind::Resolved => {
                let mut file = file();
                let hunks: Vec<DiffHunk> = file
                    .hunk_refs()
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(index, hunk)| {
                        hunk.review(if index == 0 {
                            HunkReview::Accepted
                        } else {
                            HunkReview::Rejected
                        })
                    })
                    .collect();
                file = file.hunks(hunks);
                div().size_full().child(
                    CodeDiff::new("diff", &file)
                        .reviewable(true)
                        .on_event(handler),
                )
            }
            ProbeKind::Collapsed => div()
                .size_full()
                .child(CodeDiff::new("diff", &file()).open(false).on_event(handler)),
            ProbeKind::Narrow => {
                let file = DiffFile::new("src/long.rs").hunks([DiffHunk::new("@@ -1 +1 @@")
                    .lines([
                        DiffLine::new(DiffLineKind::Removed, "let short = 1;").old_number(1),
                        DiffLine::new(
                            DiffLineKind::Added,
                            "let a_very_long_binding_name_that_keeps_going = compute_something_elaborate(first_argument, second_argument, third_argument);",
                        )
                        .new_number(1),
                    ])]);
                div()
                    .size_full()
                    .child(div().w(px(320.)).child(CodeDiff::new("diff", &file)))
            }
        }
    }
}

fn harness(
    kind: ProbeKind,
    cx: &mut TestAppContext,
) -> (Rc<RefCell<Vec<CodeDiffEvent>>>, &mut VisualTestContext) {
    cx.update(gpui_ai::init);
    let events = Rc::new(RefCell::new(Vec::new()));
    let (_, cx) = cx.add_window_view({
        let events = events.clone();
        move |_, _| Probe { kind, events }
    });
    cx.update(|_, cx| cx.set_reduce_motion(true));
    cx.simulate_resize(size(px(720.), px(520.)));
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
fn review_controls_report_the_hunk_by_path_and_index(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Reviewable, cx);
    click_center(cx, "code-diff-accept-src/pricing.rs-0");
    click_center(cx, "code-diff-reject-src/pricing.rs-1");
    click_center(cx, "code-diff-toggle-src/pricing.rs");
    assert_eq!(
        events.borrow().as_slice(),
        &[
            CodeDiffEvent::HunkAccepted {
                path: "src/pricing.rs".into(),
                hunk: 0
            },
            CodeDiffEvent::HunkRejected {
                path: "src/pricing.rs".into(),
                hunk: 1
            },
            CodeDiffEvent::Toggled {
                path: "src/pricing.rs".into()
            },
        ]
    );
}

#[gpui::test]
fn resolved_hunks_show_their_decision_instead_of_controls(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Resolved, cx);
    assert!(
        cx.debug_bounds("code-diff-accept-src/pricing.rs-0")
            .is_none()
    );
    assert!(
        cx.debug_bounds("code-diff-reject-src/pricing.rs-1")
            .is_none()
    );
    assert!(cx.debug_bounds("code-diff-hunk-src/pricing.rs-0").is_some());
}

#[gpui::test]
fn collapsed_diffs_keep_only_their_header(cx: &mut TestAppContext) {
    let (events, cx) = harness(ProbeKind::Collapsed, cx);
    assert!(cx.debug_bounds("code-diff-hunk-src/pricing.rs-0").is_none());
    cx.update(|window, cx| window.focus_next(cx));
    cx.update(|window, cx| window.draw(cx).clear(cx));
    activate_key(cx, "enter");
    assert_eq!(
        events.borrow().as_slice(),
        &[CodeDiffEvent::Toggled {
            path: "src/pricing.rs".into()
        }]
    );
}

#[gpui::test]
fn gutters_match_the_code_height_line_for_line(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Reviewable, cx);
    for index in 0..2 {
        let gutter = cx
            .debug_bounds(if index == 0 {
                "code-diff-gutter-src/pricing.rs-0"
            } else {
                "code-diff-gutter-src/pricing.rs-1"
            })
            .expect("gutter renders");
        let text = cx
            .debug_bounds(if index == 0 {
                "code-diff-text-src/pricing.rs-0"
            } else {
                "code-diff-text-src/pricing.rs-1"
            })
            .expect("code renders");
        let delta = (gutter.size.height - text.size.height).abs();
        assert!(
            delta <= px(1.),
            "hunk {index}: gutter {:?} vs code {:?}",
            gutter.size.height,
            text.size.height
        );
        assert!((gutter.origin.y - text.origin.y).abs() <= px(1.));
    }
}

#[gpui::test]
fn long_lines_scroll_inside_the_host_instead_of_widening_it(cx: &mut TestAppContext) {
    let (_, cx) = harness(ProbeKind::Narrow, cx);
    let root = cx
        .debug_bounds("code-diff-src/long.rs")
        .expect("diff renders");
    assert!(
        root.size.width <= px(320.),
        "root width {:?}",
        root.size.width
    );
    let text = cx
        .debug_bounds("code-diff-text-src/long.rs-0")
        .expect("code renders");
    assert!(
        text.size.width < px(320.),
        "code column {:?} should stay inside the host",
        text.size.width
    );
}
