use gpui::{
    Element as _, IntoElement as _, Render, RenderOnce as _, Role, TestAppContext, Window,
    accesskit, canvas,
};
use gpui_ai::{image_generation::ImageGeneration, loading::LoadingState};
use std::sync::{Arc, Mutex};

struct CapturedNode {
    has_id: bool,
    role: Option<Role>,
    node: accesskit::Node,
}

#[derive(Clone, Copy)]
enum ProgressProbeKind {
    Loading,
    Image,
}

struct ProgressProbe {
    kind: ProgressProbeKind,
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for ProgressProbe {
    fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl gpui::IntoElement {
        let kind = self.kind;
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let mut node = accesskit::Node::new(Role::ProgressIndicator);
                let (has_id, role) = match kind {
                    ProgressProbeKind::Loading => {
                        let element = LoadingState::new()
                            .label("Reasoning about suppliers")
                            .render(window, cx)
                            .into_element();
                        let has_id = element.id().is_some();
                        let role = element.a11y_role();
                        element.write_a11y_info(&mut node);
                        (has_id, role)
                    }
                    ProgressProbeKind::Image => {
                        let element = ImageGeneration::new("image")
                            .label("Alpine meadow")
                            .progress(0.42)
                            .render(window, cx)
                            .into_element();
                        let has_id = element.id().is_some();
                        let role = element.a11y_role();
                        element.write_a11y_info(&mut node);
                        (has_id, role)
                    }
                };
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { has_id, role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(kind: ProgressProbeKind, cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| ProgressProbe { kind, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("progress node should be captured")
}

#[gpui::test]
fn loading_state_is_a_named_indeterminate_progress_indicator(cx: &mut TestAppContext) {
    let captured = capture(ProgressProbeKind::Loading, cx);

    assert!(captured.has_id);
    assert_eq!(captured.role, Some(Role::ProgressIndicator));
    assert_eq!(captured.node.label(), Some("Reasoning about suppliers"));
    assert_eq!(captured.node.numeric_value(), None);
}

#[gpui::test]
fn image_generation_exposes_its_percentage(cx: &mut TestAppContext) {
    let captured = capture(ProgressProbeKind::Image, cx);

    assert!(captured.has_id);
    assert_eq!(captured.role, Some(Role::ProgressIndicator));
    assert_eq!(captured.node.label(), Some("Alpine meadow"));
    assert_eq!(captured.node.numeric_value(), Some(42.0));
    assert_eq!(captured.node.min_numeric_value(), Some(0.0));
    assert_eq!(captured.node.max_numeric_value(), Some(100.0));
}
