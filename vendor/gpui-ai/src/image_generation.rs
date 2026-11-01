//! Progressive reveal for image generation.

use crate::motion::{MotionTokens, VisibleAnimationExt as _};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{ActiveTheme as _, Icon, IconName, StyledExt as _, h_flex, v_flex};

/// An image-generation frame: shimmer while working, top-down reveal as
/// progress advances, the finished image when done.
///
/// The component is presentation-only: the caller owns generation progress
/// (`0.0..=1.0`) and supplies the image element when (or as) it is
/// available via [`Self::image`].
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::{img, Styled};
/// # fn example(generation_progress: f32) {
/// ImageGeneration::new("gen-1")
///     .label("A lighthouse at dusk, oil on canvas")
///     .progress(generation_progress)
///     .image(img("out/lighthouse.png").size_full());
/// # }
/// ```
#[derive(IntoElement)]
pub struct ImageGeneration {
    id: ElementId,
    style: StyleRefinement,
    width: Pixels,
    height: Pixels,
    progress: f32,
    label: Option<SharedString>,
    image: Option<AnyElement>,
}

/// Default frame width: a 3:2 media box that fits a chat column.
/// Both dimensions are builder-overridable per instance.
const DEFAULT_FRAME_WIDTH: gpui::Pixels = px(240.);

/// Default frame height for the 3:2 media box.
const DEFAULT_FRAME_HEIGHT: gpui::Pixels = px(160.);

impl ImageGeneration {
    /// Creates a 240×160 frame at zero progress.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            width: DEFAULT_FRAME_WIDTH,
            height: DEFAULT_FRAME_HEIGHT,
            progress: 0.0,
            label: None,
            image: None,
        }
    }

    /// Sets the frame dimensions.
    pub fn frame(mut self, width: impl Into<Pixels>, height: impl Into<Pixels>) -> Self {
        self.width = width.into();
        self.height = height.into();
        self
    }

    /// Sets generation progress in `0.0..=1.0` (clamped). Below `1.0` the
    /// frame stays partially veiled with a percentage readout.
    pub fn progress(mut self, progress: f32) -> Self {
        self.progress = progress.clamp(0.0, 1.0);
        self
    }

    /// Sets the prompt or caption shown under the frame.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = Some(label.into());
        self
    }

    /// Supplies the image element (typically `gpui::img(...)`). The caller
    /// sizes it; `size_full()` fills the frame.
    pub fn image(mut self, image: impl IntoElement) -> Self {
        self.image = Some(image.into_any_element());
        self
    }
}

impl Styled for ImageGeneration {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ImageGeneration {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let full = crate::motion::motion_is_full(cx);
        let tokens = cx.theme().semantic_tokens();
        let generating = self.progress < 1.0;
        let veil_height = self.height * (1.0 - self.progress);
        let has_image = self.image.is_some();
        let accessibility_label = self
            .label
            .clone()
            .unwrap_or_else(|| "Image generation".into());

        v_flex()
            .id(self.id)
            .role(Role::ProgressIndicator)
            .aria_label(accessibility_label)
            .aria_min_numeric_value(0.)
            .aria_max_numeric_value(100.)
            .aria_numeric_value((self.progress * 100.0) as f64)
            .gap(tokens.spacing.xs)
            .child(
                div()
                    .relative()
                    .w(self.width)
                    .h(self.height)
                    .overflow_hidden()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(tokens.radius.lg)
                    .bg(cx.theme().muted.opacity(0.4))
                    .map(|this| match self.image {
                        Some(image) => this.child(div().size_full().child(image)),
                        None => this.child(
                            // Placeholder: a centered icon breathing until
                            // pixels arrive.
                            div()
                                .size_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(
                                    Icon::new(IconName::Palette)
                                        .text_color(cx.theme().muted_foreground),
                                )
                                .with_visible_animation(
                                    "image-pulse",
                                    // Frame demand: active only on this
                                    // placeholder branch. Once the caller
                                    // supplies an image the animated element
                                    // is not built at all, so a finished
                                    // frame demands nothing. Reduced motion
                                    // holds delta at 0 — a fully opaque icon.
                                    MotionTokens::read(cx).image_pulse().looping(),
                                    move |this, delta| {
                                        let delta = if full { delta } else { 0.0 };
                                        let wave = (delta * 2.0 - 1.0).abs();
                                        this.opacity(0.35 + 0.65 * wave)
                                    },
                                ),
                        ),
                    })
                    .when(generating && has_image, |this| {
                        // Top-down reveal: the unrevealed remainder stays
                        // veiled in the theme background.
                        this.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .bottom_0()
                                .h(veil_height)
                                .bg(cx.theme().background.opacity(0.85)),
                        )
                    })
                    .when(generating, |this| {
                        this.child(
                            div()
                                .absolute()
                                .bottom(tokens.spacing.xs)
                                .right(tokens.spacing.xs)
                                .px(tokens.spacing.xs)
                                .py(tokens.spacing.xxs)
                                .rounded(tokens.radius.full)
                                .bg(cx.theme().background.opacity(0.8))
                                .text_token(tokens.typography.xs)
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{:.0}%", self.progress * 100.0)),
                        )
                    }),
            )
            .when_some(self.label, |this, label| {
                this.child(
                    h_flex().w(self.width).child(
                        div()
                            .truncate()
                            .text_token(tokens.typography.xs)
                            .italic()
                            .text_color(cx.theme().muted_foreground)
                            .child(label),
                    ),
                )
            })
            .refine_style(&self.style)
    }
}
