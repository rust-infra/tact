//! A clipped repeating region keeps its layout but releases its frame demand.

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, Bounds, Element, ElementId, Entity,
    GlobalElementId, InspectorElementId, IntoElement, LayoutId, Pixels, RenderOnce, Window,
};

pub(crate) trait VisibleAnimationExt: IntoElement + 'static {
    fn with_visible_animation(
        self,
        id: impl Into<ElementId>,
        animation: Animation,
        animator: impl Fn(Self, f32) -> Self + 'static,
    ) -> impl IntoElement {
        VisibleAnimation {
            element: self,
            id: id.into(),
            animation,
            animator,
        }
    }
}
impl<E: IntoElement + 'static> VisibleAnimationExt for E {}

#[derive(IntoElement)]
struct VisibleAnimation<E: IntoElement + 'static, F: Fn(E, f32) -> E + 'static> {
    element: E,
    id: ElementId,
    animation: Animation,
    animator: F,
}

impl<E: IntoElement + 'static, F: Fn(E, f32) -> E + 'static> RenderOnce for VisibleAnimation<E, F> {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let visible = window.use_keyed_state((self.id.clone(), "visibility"), cx, |_, _| false);
        let child = if *visible.read(cx) {
            self.element
                .with_animation(self.id, self.animation, self.animator)
                .into_any_element()
        } else {
            (self.animator)(self.element, 0.0).into_any_element()
        };
        Visibility { child, visible }
    }
}

struct Visibility {
    child: AnyElement,
    visible: Entity<bool>,
}

impl IntoElement for Visibility {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for Visibility {
    type RequestLayoutState = ();
    type PrepaintState = ();
    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }
    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }
    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let visible = bounds.intersects(&window.content_mask().bounds)
            && bounds.intersects(&Bounds::new(Default::default(), window.viewport_size()));
        if *self.visible.read(cx) != visible {
            // Prepaint has the real clip. Schedule one boundary frame for
            // this region's owning view; do not also notify it synchronously.
            self.visible.update(cx, |state, _| {
                *state = visible;
            });
            if !cx.reduce_motion() {
                window.request_animation_frame();
            }
        }
        self.child.prepaint(window, cx);
    }
    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.paint(window, cx);
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        loading::LoadingState,
        orbs::Orbs,
        voice::{VoiceControls, VoiceState},
    };
    use gpui::{
        Context, IntoElement, ParentElement as _, Render, Styled as _, TestAppContext, Window, px,
        size,
    };

    struct Probe {
        clipped: bool,
    }
    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            gpui::div().size_full().overflow_hidden().child(
                gpui::div()
                    .relative()
                    .top(if self.clipped { px(400.) } else { px(0.) })
                    .child(LoadingState::new())
                    .child(Orbs::new())
                    .child(VoiceControls::new(
                        "voice",
                        VoiceState::Listening { level: 0.6 },
                    )),
            )
        }
    }

    #[gpui::test]
    fn mounted_but_clipped_regions_stop_and_resume_frame_demand(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let window = cx.open_window(size(px(500.), px(240.)), |_, _| Probe { clipped: false });
        cx.run_until_parked();
        let tick = |cx: &mut TestAppContext| {
            let count = window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .expect("window");
            cx.run_until_parked();
            count
        };
        tick(cx);
        assert_eq!(tick(cx), 3, "one clock per visible region");
        for clipped in [true, false, true] {
            window
                .update(cx, |probe, _, cx| {
                    probe.clipped = clipped;
                    cx.notify();
                })
                .expect("window");
            cx.run_until_parked();
            tick(cx);
            tick(cx);
            assert_eq!(
                tick(cx),
                if clipped { 0 } else { 3 },
                "actual clipping controls demand"
            );
        }
    }

    #[gpui::test]
    fn sibling_orbs_keep_independent_visibility(cx: &mut TestAppContext) {
        struct Siblings;
        impl Render for Siblings {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                gpui::div().size_full().overflow_hidden().children(
                    ["shown", "clipped"]
                        .into_iter()
                        .enumerate()
                        .map(|(ix, id)| {
                            gpui::div()
                                .relative()
                                .top(px(ix as f32 * 400.))
                                .child(Orbs::new().id(id))
                        }),
                )
            }
        }
        cx.update(crate::init);
        let window = cx.open_window(size(px(240.), px(160.)), |_, _| Siblings);
        cx.run_until_parked();
        for ix in 0..6 {
            let count = window
                .update(cx, |_, window, cx| window.simulate_next_frame(cx))
                .expect("window");
            cx.run_until_parked();
            if ix > 1 {
                assert_eq!(count, 1, "only the visible sibling keeps ticking");
            }
        }
    }
}
