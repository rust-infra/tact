//! Agent suggestion cards with confidence and alternatives.

use crate::ButtonLabelExt as _;
use crate::control::ControlMetricsExt as _;
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::handlers::Handler;
use crate::surface::{card, description, eyebrow, title};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _, relative,
};

/// An interaction emitted by [`RecommendationCard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecommendationEvent {
    /// The recommendation was accepted.
    Accepted {
        /// Stable recommendation identifier.
        id: SharedString,
    },
}
use crate::motion::MotionTokens;
use gpui_base::motion::{Transition, transition};
use gpui_component::{
    ActiveTheme as _, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

/// An agent suggestion: what it proposes, how confident it is, and what else
/// it considered.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// RecommendationCard::new("rec-1", "Switch supplier to Alpenrose Dairy")
///     .description("Lower unit cost at equal volume; delivery risk unchanged.")
///     .confidence(0.87)
///     .alternatives(["Keep current supplier", "Split volume 60/40"])
///     .on_event(|event, _, _| { /* apply the stable event id */ });
/// ```
#[derive(IntoElement)]
pub struct RecommendationCard {
    id: SharedString,
    style: StyleRefinement,
    decoration: Decoration,
    title: SharedString,
    description: Option<SharedString>,
    confidence: Option<f32>,
    alternatives: Vec<SharedString>,
    accept_label: SharedString,
    on_event: Option<Handler<RecommendationEvent>>,
}

impl RecommendationCard {
    /// Layers painted into this card's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a card with the recommendation headline.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            title: title.into(),
            description: None,
            confidence: None,
            alternatives: Vec::new(),
            accept_label: "Apply".into(),
            on_event: None,
        }
    }

    /// Sets the supporting explanation.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Sets the confidence in `0.0..=1.0`, rendered as a meter. Values are
    /// clamped.
    pub fn confidence(mut self, confidence: f32) -> Self {
        self.confidence = Some(confidence.clamp(0.0, 1.0));
        self
    }

    /// Lists the alternatives the agent considered but ranked lower.
    pub fn alternatives(
        mut self,
        alternatives: impl IntoIterator<Item = impl Into<SharedString>>,
    ) -> Self {
        self.alternatives = alternatives.into_iter().map(Into::into).collect();
        self
    }

    /// Adds an accept button. Default label is "Apply"; override with
    /// [`Self::accept_label`].
    pub fn on_event(
        mut self,
        handler: impl Fn(&RecommendationEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Box::new(handler));
        self
    }

    /// Overrides the accept button label.
    pub fn accept_label(mut self, label: impl Into<SharedString>) -> Self {
        self.accept_label = label.into();
        self
    }
}

impl Styled for RecommendationCard {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for RecommendationCard {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let event = RecommendationEvent::Accepted {
            id: self.id.clone(),
        };
        let accessibility_label = self.title.clone();
        let accessibility_description = self.description.clone();
        card(self.id.clone(), cx)
            .decoration_under(&mut decoration)
            .role(Role::Group)
            .aria_label(accessibility_label)
            .when_some(accessibility_description, |this, description| {
                this.aria_description(description)
            })
            .child(title(self.title, cx))
            .when_some(self.description, |this, text| {
                this.child(description(text, cx))
            })
            .when_some(self.confidence, |this, confidence| {
                let meter_color = if confidence < 0.4 {
                    cx.theme().danger
                } else if confidence < 0.7 {
                    cx.theme().warning
                } else {
                    cx.theme().success
                };
                this.child(
                    h_flex()
                        .id(format!("{}-confidence", self.id))
                        .role(Role::Meter)
                        .aria_label("Confidence")
                        .aria_min_numeric_value(0.)
                        .aria_max_numeric_value(100.)
                        .aria_numeric_value((confidence * 100.0) as f64)
                        .items_center()
                        .gap(tokens.spacing.sm)
                        .child(
                            div()
                                .flex_1()
                                .h_1p5()
                                .rounded(tokens.radius.full)
                                .bg(cx.theme().muted)
                                .child(
                                    // The fill retargets from its current
                                    // sample when the controlled confidence
                                    // changes; the first render adopts the
                                    // supplied value and reduced motion
                                    // snaps — the transition contract. The
                                    // meter's numeric value above is the
                                    // controlled figure, never the sample:
                                    // no animated count-up.
                                    div()
                                        .h_full()
                                        .w(relative(transition(
                                            (
                                                ElementId::from(SharedString::from(format!(
                                                    "{}-confidence-fill",
                                                    self.id
                                                ))),
                                                "value",
                                            ),
                                            confidence,
                                            Transition::new(crate::motion::retarget_duration(
                                                cx,
                                                MotionTokens::read(cx).standard(),
                                            ))
                                            .ease(crate::motion::ease_in_out_quart),
                                            window,
                                            cx,
                                        )))
                                        .rounded(tokens.radius.full)
                                        .bg(meter_color),
                                ),
                        )
                        .child(
                            div()
                                .flex_none()
                                .text_token(tokens.typography.xs)
                                .font_family(cx.theme().mono_font_family.clone())
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{:.0}%", confidence * 100.0)),
                        ),
                )
            })
            .when(!self.alternatives.is_empty(), |this| {
                this.child(
                    v_flex()
                        .id(format!("{}-alternatives", self.id))
                        .role(Role::List)
                        .aria_label("Also considered")
                        .gap(tokens.spacing.xs)
                        .child(eyebrow("Also considered", cx))
                        .children(self.alternatives.into_iter().enumerate().map(|(ix, alt)| {
                            let accessibility_label = alt.clone();
                            // The bullet rides a first-line slot so a
                            // wrapping alternative keeps it seated on its
                            // first line instead of floating mid-block.
                            h_flex()
                                .id(format!("{}-alternative-{ix}", self.id))
                                .role(Role::ListItem)
                                .aria_label(accessibility_label)
                                .items_start()
                                .gap(tokens.spacing.sm)
                                .text_token(tokens.typography.sm)
                                .text_color(cx.theme().muted_foreground)
                                .child(crate::surface::leading_glyph_slot(
                                    crate::sizing::slot_md(cx),
                                    div()
                                        .size_1()
                                        .rounded(tokens.radius.full)
                                        .bg(cx.theme().muted_foreground),
                                ))
                                .child(div().min_w_0().child(alt))
                        })),
                )
            })
            .when_some(self.on_event, |this, handler| {
                this.child(
                    h_flex().child(
                        Button::new(format!("{}-accept", self.id))
                            .primary()
                            .small()
                            .control_metrics(cx)
                            .accessibility_id(format!("{}-accept", self.id))
                            .text_label(self.accept_label)
                            .on_click(move |_: &ClickEvent, window, cx| {
                                handler(&event, window, cx)
                            }),
                    ),
                )
            })
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
    }
}
