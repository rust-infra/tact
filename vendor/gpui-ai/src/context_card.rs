//! Cards for retrieved knowledge chunks with source attribution.

use crate::control::composed_button;
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::handlers::SharedHandler;
use crate::surface::CardFrameExt as _;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ClickEvent, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, v_flex,
};
use std::rc::Rc;

/// A retrieved knowledge chunk: source, snippet, and optional relevance.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// ContextCard::new("ctx-1", "pricing.md")
///     .snippet("Enterprise plans include SSO and a dedicated…")
///     .relevance(0.92)
///     .on_event(|event, _, _| { /* open the stable event id */ });
/// ```
#[derive(IntoElement)]
pub struct ContextCard {
    id: SharedString,
    style: StyleRefinement,
    decoration: Decoration,
    source: SharedString,
    snippet: Option<SharedString>,
    relevance: Option<f32>,
    on_event: Option<SharedHandler<ContextCardEvent>>,
}

impl ContextCard {
    /// Layers painted into this card's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a card for a chunk retrieved from `source` (a document name,
    /// URL, or collection).
    pub fn new(id: impl Into<SharedString>, source: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            source: source.into(),
            snippet: None,
            relevance: None,
            on_event: None,
        }
    }

    /// Sets the retrieved text snippet.
    pub fn snippet(mut self, snippet: impl Into<SharedString>) -> Self {
        self.snippet = Some(snippet.into());
        self
    }

    /// Sets a relevance score in `0.0..=1.0`, shown as a percentage.
    /// Values are clamped.
    pub fn relevance(mut self, relevance: f32) -> Self {
        self.relevance = Some(relevance.clamp(0.0, 1.0));
        self
    }

    /// Makes the card clickable to open the underlying source.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ContextCardEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    fn accessibility_label(&self) -> SharedString {
        match self.relevance {
            Some(relevance) => {
                format!("{}, {:.0}% relevance", self.source, relevance * 100.0).into()
            }
            None => self.source.clone(),
        }
    }
}

impl Styled for ContextCard {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ContextCard {
    fn render(mut self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let clickable = self.on_event.is_some();
        let event = ContextCardEvent::Opened {
            id: self.id.clone(),
        };
        let accessibility_label = self.accessibility_label();
        let accessibility_description = self.snippet.clone();

        // The source line is a header over the snippet, not a label glued
        // to it: the card takes panel padding and the two sit a step apart.
        let content = v_flex()
            .gap(tokens.spacing.sm)
            .child(
                h_flex()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .child(
                        Icon::new(IconName::File)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        crate::surface::hint(self.source, cx)
                            .flex_1()
                            .truncate()
                            .font_family(cx.theme().mono_font_family.clone()),
                    )
                    .when_some(self.relevance, |this, relevance| {
                        this.child(
                            div()
                                .flex_none()
                                .text_token(tokens.typography.xs)
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{:.0}%", relevance * 100.0)),
                        )
                    })
                    .when(clickable, |this| {
                        this.child(
                            Icon::new(IconName::ExternalLink)
                                .xsmall()
                                .text_color(cx.theme().muted_foreground),
                        )
                    }),
            )
            .when_some(self.snippet, |this, snippet| {
                this.child(
                    div()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().foreground)
                        .line_clamp(3)
                        .child(snippet),
                )
            });

        // One frame, both ways. The interactive card used to be an upstream
        // button, whose own content layout centred and padded differently
        // from the plain div the static card used — two cards in a column
        // sat on different edges. The button now contributes behavior; the
        // frame is the same frame either way.
        if let Some(handler) = self.on_event {
            composed_button(self.id.clone(), accessibility_label)
                .w_full()
                .flex()
                .flex_col()
                .items_start()
                .justify_start()
                .p(tokens.spacing.lg)
                .card_frame(cx)
                .decoration_under(&mut decoration)
                .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                .active(|style| style.bg(cx.theme().accent))
                .focus_visible(|style| style.border_color(cx.theme().ring))
                .when_some(accessibility_description, |this, description| {
                    this.aria_description(description)
                })
                .child(content.w_full())
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                .decoration_over(&mut decoration)
                .refine_style(&self.style)
                .into_any_element()
        } else {
            content
                .id(self.id)
                .role(Role::Group)
                .aria_label(accessibility_label)
                .when_some(accessibility_description, |this, description| {
                    this.aria_description(description)
                })
                .w_full()
                .p(tokens.spacing.lg)
                .card_frame(cx)
                .decoration_under(&mut decoration)
                .decoration_over(&mut decoration)
                .refine_style(&self.style)
                .into_any_element()
        }
    }
}
/// An interaction emitted by [`ContextCard`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextCardEvent {
    /// The underlying source was selected.
    Opened {
        /// Stable context identifier.
        id: SharedString,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accessibility_name_includes_source_and_relevance() {
        assert_eq!(
            ContextCard::new("ctx", "pricing.md")
                .relevance(0.923)
                .accessibility_label(),
            "pricing.md, 92% relevance"
        );
        assert_eq!(
            ContextCard::new("ctx", "pricing.md").accessibility_label(),
            "pricing.md"
        );
    }
}
