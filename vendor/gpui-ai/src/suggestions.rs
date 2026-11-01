//! Suggestion chips: starter prompts and follow-ups as one quiet row.
//!
//! The chips ripple into place with a staggered reveal, sit on the surface
//! token as outlined pills, and report a stable ID when chosen. The
//! application decides whether a selection is sent immediately or merely
//! populates the composer.

use crate::cues::{self, Cue};
use crate::{
    control::{PressReleaseExt as _, composed_button},
    handlers::SharedHandler,
    motion::reveal_staggered,
    theme::SemanticStyledExt as _,
};
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, StyledExt as _, h_flex};
use std::rc::Rc;

/// One suggested prompt with stable identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Suggestion {
    id: SharedString,
    label: SharedString,
    description: Option<SharedString>,
}

impl Suggestion {
    /// Creates a suggestion with a stable identifier and the visible prompt.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            description: None,
        }
    }

    /// Adds an accessible description (what choosing this will do).
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Returns the stable suggestion identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the visible prompt text.
    pub fn label(&self) -> &SharedString {
        &self.label
    }
}

/// An interaction emitted by [`Suggestions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SuggestionsEvent {
    /// A suggestion was chosen.
    Selected {
        /// Stable suggestion identifier.
        id: SharedString,
    },
}

/// A wrapping row of suggestion chips.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// Suggestions::new("starters")
///     .items([
///         Suggestion::new("compare", "Compare supplier prices"),
///         Suggestion::new("risk", "Explain delivery risk"),
///     ])
///     .on_event(|event, _, _| { /* SuggestionsEvent::Selected { id } */ });
/// ```
#[derive(IntoElement)]
pub struct Suggestions {
    id: ElementId,
    style: StyleRefinement,
    items: Vec<Suggestion>,
    on_event: Option<SharedHandler<SuggestionsEvent>>,
    label: SharedString,
}

impl Suggestions {
    /// Creates an empty suggestion row.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            items: Vec::new(),
            on_event: None,
            label: "Suggestions".into(),
        }
    }

    /// Overrides the accessible name (default "Suggestions").
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Sets the suggestions, in display order.
    pub fn items(mut self, items: impl IntoIterator<Item = Suggestion>) -> Self {
        self.items = items.into_iter().collect();
        self
    }

    /// Handles typed selections. Without a handler the chips are static.
    pub fn on_event(
        mut self,
        handler: impl Fn(&SuggestionsEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl Styled for Suggestions {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for Suggestions {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let root_id = self.id.clone();
        let handler = self.on_event;
        h_flex()
            .id(self.id)
            .role(Role::Group)
            .aria_label(self.label)
            .flex_wrap()
            .gap(tokens.spacing.xs)
            .children(self.items.into_iter().enumerate().map(|(ix, item)| {
                let chip_id = ElementId::from((root_id.clone(), format!("chip-{}", item.id)));
                let debug_id = item.id.clone();
                let chip = match handler.clone() {
                    Some(handler) => {
                        let event = SuggestionsEvent::Selected {
                            id: item.id.clone(),
                        };
                        composed_button(chip_id.clone(), item.label.clone())
                            .debug_selector(move || format!("suggestion-{debug_id}"))
                            .when_some(item.description.clone(), |this, description| {
                                this.aria_description(description)
                            })
                            .flex()
                            .items_center()
                            // Chip geometry from the size policy: prompt
                            // pills stand a large control tall, which is
                            // the height the composer's own controls take
                            // and the proportion a pill wants — roughly
                            // twice the vertical room the medium tier left
                            // around a small-text line, against the same
                            // horizontal inset.
                            .h(crate::sizing::control_lg(cx))
                            .px(tokens.spacing.md)
                            .border_1()
                            .border_color(cx.theme().border)
                            .rounded(tokens.radius.full)
                            .bg(tokens.colors.surface)
                            .text_token(tokens.typography.sm)
                            .text_color(cx.theme().foreground)
                            // Hover fills; only keyboard focus recolors the
                            // border — the ring stays a focus signal.
                            .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                            .active(|style| style.bg(cx.theme().accent))
                            .focus_visible(|style| style.border_color(cx.theme().ring))
                            .press_release(
                                ElementId::from((chip_id, "press")),
                                tokens.radius.full,
                                window,
                                cx,
                            )
                            .child({
                                let label_debug = item.id.clone();
                                div()
                                    .debug_selector(move || {
                                        format!("suggestion-label-{label_debug}")
                                    })
                                    .child(item.label)
                            })
                            .on_click(move |_: &ClickEvent, window, cx| {
                                cues::emit(cx, Cue::SuggestionSelected);
                                handler(&event, window, cx)
                            })
                            .into_any_element()
                    }
                    None => div()
                        .id(chip_id)
                        .role(Role::ListItem)
                        .aria_label(item.label.clone())
                        .flex()
                        .items_center()
                        .h(crate::sizing::control_lg(cx))
                        .px(tokens.spacing.md)
                        .border_1()
                        .border_color(cx.theme().border)
                        .rounded(tokens.radius.full)
                        .bg(tokens.colors.surface)
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().muted_foreground)
                        .child(item.label)
                        .into_any_element(),
                };
                reveal_staggered(
                    div().child(chip),
                    (root_id.clone(), format!("reveal-{}", item.id)),
                    ix,
                    window,
                    cx,
                )
            }))
            .refine_style(&self.style)
    }
}
