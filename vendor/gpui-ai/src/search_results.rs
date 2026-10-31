//! Web-search tool output: a query and its results with sources.

use crate::control::{PressReleaseExt as _, composed_button};
use crate::decoration::{DecoratedExt as _, Decoration};
use crate::handlers::SharedHandler;
use crate::motion::{ArrivalRoster, MotionTokens};
use crate::surface::CardFrameExt as _;
use crate::surface::{initial_badge, initial_of};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, Role, SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled,
    Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, StyledExt as _, h_flex, spinner::Spinner,
    v_flex,
};

/// One search hit inside [`SearchResults`].
#[derive(Debug, Clone)]
pub struct SearchResult {
    id: SharedString,
    title: SharedString,
    domain: Option<SharedString>,
}

impl SearchResult {
    /// Creates a result with its display title.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            domain: None,
        }
    }

    /// Sets the source domain shown after the title (`example.com`).
    pub fn domain(mut self, domain: impl Into<SharedString>) -> Self {
        self.domain = Some(domain.into());
        self
    }

    fn opened_event(&self) -> SearchResultsEvent {
        SearchResultsEvent::Opened {
            id: self.id.clone(),
        }
    }
}

/// An interaction emitted by [`SearchResults`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchResultsEvent {
    /// A result was selected.
    Opened {
        /// Stable result identifier.
        id: SharedString,
    },
}

/// The output of a web-search tool call: the query, a live "searching"
/// state, and the results once they arrive.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// SearchResults::new("search-1", "gpui wasm support")
///     .results([
///         SearchResult::new("gallery", "GPUI Component — Web Gallery")
///             .domain("longbridge.github.io"),
///         SearchResult::new("gpui-web", "zed-industries/zed: crates/gpui_web")
///             .domain("github.com"),
///     ])
///     .on_event(|event, _, _| { /* open result */ });
/// ```
#[derive(IntoElement)]
pub struct SearchResults {
    id: ElementId,
    style: StyleRefinement,
    decoration: Decoration,
    query: SharedString,
    searching: bool,
    results: Vec<SearchResult>,
    on_event: Option<SharedHandler<SearchResultsEvent>>,
}

impl SearchResults {
    /// Layers painted into this card's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates the component for a search `query`.
    pub fn new(id: impl Into<ElementId>, query: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            query: query.into(),
            searching: false,
            results: Vec::new(),
            on_event: None,
        }
    }

    /// Marks the search as still running; the header shows a spinner.
    pub fn searching(mut self, searching: bool) -> Self {
        self.searching = searching;
        self
    }

    /// Sets the results.
    pub fn results(mut self, results: impl IntoIterator<Item = SearchResult>) -> Self {
        self.results = results.into_iter().collect();
        self
    }

    /// Handles typed result interactions.
    pub fn on_event(
        mut self,
        handler: impl Fn(&SearchResultsEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(std::rc::Rc::new(handler));
        self
    }
}

impl Styled for SearchResults {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for SearchResults {
    fn render(mut self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a
        // component with no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let header: SharedString = if self.searching {
            format!("Searching \u{201c}{}\u{201d}\u{2026}", self.query).into()
        } else {
            format!(
                "{} result{} for \u{201c}{}\u{201d}",
                self.results.len(),
                if self.results.len() == 1 { "" } else { "s" },
                self.query
            )
            .into()
        };
        let handler = self.on_event;
        let root_id = self.id.clone();

        // Results the list has already shown stay at rest; stable IDs
        // streamed into a mounted list settle in on the capped cascade, and
        // the initial set joins at rest.
        let motion = MotionTokens::read(cx).clone();
        let roster = window.use_keyed_state((root_id.clone(), "arrivals"), cx, |_, _| {
            ArrivalRoster::new()
        });
        roster.update(cx, |roster, cx| {
            roster.note(
                self.results.iter().map(|result| {
                    ElementId::Name(SharedString::from(format!("result-{}", result.id)))
                }),
                true,
                &motion,
                cx.background_executor().now(),
            );
        });

        v_flex()
            .id(self.id)
            .role(Role::Search)
            .aria_label(header.clone())
            .card_frame(cx)
            .decoration_under(&mut decoration)
            .overflow_hidden()
            .child(
                h_flex()
                    .items_center()
                    .gap(tokens.spacing.sm)
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.xs)
                    .text_token(tokens.typography.xs)
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.searching {
                        Spinner::new()
                            .xsmall()
                            .color(cx.theme().muted_foreground)
                            .into_any_element()
                    } else {
                        Icon::new(IconName::Search)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground)
                            .into_any_element()
                    })
                    .child(header),
            )
            .when(!self.results.is_empty(), |this| {
                this.child(
                    v_flex()
                        .border_t_1()
                        .border_color(crate::surface::hairline(cx))
                        // Rows are inset from the card that frames them, so
                        // a hover fill sits inside the card's rounded edge
                        // instead of running out to it.
                        .p(tokens.spacing.xs)
                        .gap(tokens.spacing.xxs)
                        .children(self.results.into_iter().map(|result| {
                            let event = result.opened_event();
                            let result_id = result.id.clone();
                            let arrival = roster.update(cx, |roster, cx| {
                                roster.progress(
                                    &ElementId::Name(SharedString::from(format!(
                                        "result-{}",
                                        result.id
                                    ))),
                                    window,
                                    cx,
                                )
                            });
                            let accessibility_label = result.title.clone();
                            let accessibility_description = result.domain.clone();
                            let row = h_flex()
                                .w_full()
                                .items_center()
                                .gap(tokens.spacing.sm)
                                .child(match &result.domain {
                                    Some(domain) => {
                                        initial_badge(initial_of(domain), cx).into_any_element()
                                    }
                                    None => Icon::new(IconName::Globe)
                                        .xsmall()
                                        .text_color(cx.theme().muted_foreground)
                                        .into_any_element(),
                                })
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .text_token(tokens.typography.sm)
                                        .text_color(cx.theme().foreground)
                                        .child(result.title),
                                )
                                .when_some(result.domain, |this, domain| {
                                    this.child(
                                        crate::surface::hint(domain, cx)
                                            .flex_none()
                                            .font_family(cx.theme().mono_font_family.clone()),
                                    )
                                });

                            let row = match arrival {
                                Some(progress) => row.opacity(progress).top(
                                    tokens.spacing.xxs
                                        * (1.0 - progress)
                                        * crate::motion::travel(cx),
                                ),
                                None => row,
                            };

                            match handler.clone() {
                                Some(handler) => {
                                    composed_button(result.id.clone(), accessibility_label)
                                        .w_full()
                                        .justify_start()
                                        .px(tokens.spacing.sm)
                                        .py(tokens.spacing.xs)
                                        .border_1()
                                        .border_color(cx.theme().transparent)
                                        .rounded(crate::surface::nested_radius(
                                            tokens.radius.lg,
                                            tokens.spacing.xs,
                                            tokens.radius.sm,
                                        ))
                                        .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                                        .active(|style| style.bg(cx.theme().accent))
                                        .focus_visible(|style| style.border_color(cx.theme().ring))
                                        .press_release(
                                            ElementId::from((
                                                ElementId::from(result.id.clone()),
                                                "press",
                                            )),
                                            tokens.radius.sm,
                                            window,
                                            cx,
                                        )
                                        .when_some(
                                            accessibility_description,
                                            |this, description| this.aria_description(description),
                                        )
                                        .child(row)
                                        .on_click(move |_: &ClickEvent, window, cx| {
                                            handler(&event, window, cx)
                                        })
                                        .into_any_element()
                                }
                                None => div()
                                    .id(result_id)
                                    .px(tokens.spacing.sm)
                                    .py(tokens.spacing.xs)
                                    .role(Role::ListItem)
                                    .aria_label(accessibility_label)
                                    .when_some(accessibility_description, |this, description| {
                                        this.aria_description(description)
                                    })
                                    .w_full()
                                    .px(tokens.spacing.md)
                                    .py(tokens.spacing.xs)
                                    .child(row)
                                    .into_any_element(),
                            }
                        })),
                )
            })
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_titles_emit_distinct_stable_ids() {
        let first = SearchResult::new("first", "Same title");
        let second = SearchResult::new("second", "Same title");
        assert_eq!(
            first.opened_event(),
            SearchResultsEvent::Opened { id: "first".into() }
        );
        assert_eq!(
            second.opened_event(),
            SearchResultsEvent::Opened {
                id: "second".into()
            }
        );
    }
}
