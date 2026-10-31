use gpui::{
    Context, Element as _, IntoElement as _, Render, RenderOnce as _, Role, TestAppContext, Window,
    accesskit, canvas,
};
use gpui_ai::context_meter::{ContextMeter, ContextMeterVariant, ContextUsage};
use std::sync::{Arc, Mutex};

struct CapturedNode {
    role: Option<Role>,
    node: accesskit::Node,
}

struct Probe {
    variant: ContextMeterVariant,
    captured: Arc<Mutex<Option<CapturedNode>>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl gpui::IntoElement {
        let variant = self.variant;
        let captured = self.captured.clone();
        canvas(
            move |_, window, cx| {
                let usage = ContextUsage::new(84_300, 200_000);
                let element = ContextMeter::new("context", &usage)
                    .variant(variant)
                    .render(window, cx)
                    .into_element();
                let role = element.a11y_role();
                let mut node = accesskit::Node::new(Role::Unknown);
                element.write_a11y_info(&mut node);
                *captured.lock().expect("capture mutex should be available") =
                    Some(CapturedNode { role, node });
            },
            |_, _, _, _| {},
        )
    }
}

fn capture(variant: ContextMeterVariant, cx: &mut TestAppContext) -> CapturedNode {
    cx.update(gpui_ai::init);
    let captured = Arc::new(Mutex::new(None));
    let result = captured.clone();
    let (_, cx) = cx.add_window_view(move |_, _| Probe { variant, captured });
    cx.update(|window, cx| window.draw(cx).clear(cx));
    result
        .lock()
        .expect("capture mutex should be available")
        .take()
        .expect("meter node should be captured")
}

#[gpui::test]
fn every_variant_is_a_named_progress_indicator_with_spoken_numbers(cx: &mut TestAppContext) {
    for variant in [
        ContextMeterVariant::Ring,
        ContextMeterVariant::Bar,
        ContextMeterVariant::Text,
    ] {
        let captured = capture(variant, cx);
        assert_eq!(captured.role, Some(Role::ProgressIndicator));
        assert_eq!(captured.node.label(), Some("Context usage"));
        assert_eq!(
            captured.node.description(),
            Some("84.3K of 200K tokens used, 42%")
        );
        assert_eq!(captured.node.numeric_value(), Some(42.0));
        assert_eq!(captured.node.min_numeric_value(), Some(0.0));
        assert_eq!(captured.node.max_numeric_value(), Some(100.0));
    }
}
