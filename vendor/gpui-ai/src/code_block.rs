//! Syntax-highlighted code blocks with streaming reveal.

use crate::decoration::{DecoratedExt as _, Decoration};
use crate::stream::{ProgressState, StreamedContent};
use crate::surface::CardFrameExt as _;
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, StyledExt as _, clipboard::Clipboard, h_flex, text::TextView, v_flex,
};

/// A code block with a language header, copy button, and syntax highlighting
/// (via gpui-component's markdown/highlighter pipeline).
///
/// For streaming code, pass a [`StreamedContent`] with [`Self::streamed`]; a
/// cursor glyph marks the insertion point while content arrives.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// CodeBlock::new("example", "fn main() {\n    println!(\"hi\");\n}")
///     .language("rust");
/// ```
#[derive(IntoElement)]
pub struct CodeBlock {
    id: ElementId,
    style: StyleRefinement,
    decoration: Decoration,
    code: SharedString,
    language: Option<SharedString>,
    streaming: bool,
    failed: Option<SharedString>,
    copyable: bool,
}

impl CodeBlock {
    /// Layers painted into this block's frame: one under the content, one
    /// over it, both clipped to its own shape and neither affecting layout.
    ///
    /// This crate ships no effects of its own — what goes in a decoration is
    /// the application's expression.
    pub fn decoration(mut self, decoration: Decoration) -> Self {
        self.decoration = decoration;
        self
    }

    /// Creates a block from complete code.
    pub fn new(id: impl Into<ElementId>, code: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            decoration: Decoration::default(),
            code: code.into(),
            language: None,
            streaming: false,
            failed: None,
            copyable: true,
        }
    }

    /// Creates a block from streamed content, deriving the streaming/failed
    /// presentation from its state.
    pub fn streamed(id: impl Into<ElementId>, content: &StreamedContent) -> Self {
        let mut this = Self::new(id, content.text().to_string());
        match content.state() {
            ProgressState::Running => this.streaming = true,
            ProgressState::Pending | ProgressState::Complete => {}
            ProgressState::Failed(reason) => this.failed = Some(reason.clone()),
        }
        this
    }

    /// Sets the language used for the header label and syntax highlighting.
    pub fn language(mut self, language: impl Into<SharedString>) -> Self {
        self.language = Some(language.into());
        self
    }

    /// Hides the copy button.
    pub fn not_copyable(mut self) -> Self {
        self.copyable = false;
        self
    }
}

impl Styled for CodeBlock {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for CodeBlock {
    fn render(mut self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // Taken once: each layer is placed at most once, and a component with
        // no decoration adds no elements at all.
        let mut decoration = std::mem::take(&mut self.decoration);
        let language = self.language.unwrap_or_else(|| "text".into());

        // Build a fenced markdown block, using a fence longer than any
        // backtick run inside the code so the content cannot break out.
        let longest_run = self
            .code
            .split(|c| c != '`')
            .map(str::len)
            .max()
            .unwrap_or(0);
        let fence = "`".repeat((longest_run + 1).max(3));
        let cursor = if self.streaming { "▌" } else { "" };
        let source = format!("{fence}{language}\n{}{cursor}\n{fence}", self.code);
        let accessibility_label: SharedString = format!("{language} code").into();
        let accessibility_description = self.failed.clone();

        v_flex()
            .id(self.id)
            .role(Role::Code)
            .aria_label(accessibility_label)
            .when_some(accessibility_description, |this, description| {
                this.aria_description(description)
            })
            .card_frame(cx)
            .overflow_hidden()
            .decoration_under(&mut decoration)
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.xs)
                    .bg(cx.theme().muted.opacity(0.35))
                    .border_b_1()
                    .border_color(cx.theme().border)
                    // The language name yields and the copy control holds:
                    // a long language should shorten its own label rather
                    // than push the only affordance in the header out of it.
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_token(tokens.typography.xs)
                            .font_family(cx.theme().mono_font_family.clone())
                            .text_color(cx.theme().muted_foreground)
                            .child(language.clone()),
                    )
                    .when(self.copyable, |this| {
                        this.child(
                            div()
                                .flex_none()
                                .child(Clipboard::new("copy").value(self.code.clone())),
                        )
                    }),
            )
            .child(
                div()
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.sm)
                    .text_token(tokens.typography.sm)
                    .child(TextView::markdown("code", source).selectable(true)),
            )
            .when_some(self.failed, |this, reason| {
                this.child(
                    div()
                        .px(tokens.spacing.md)
                        .py(tokens.spacing.xs)
                        .border_t_1()
                        .border_color(cx.theme().border)
                        .text_token(tokens.typography.xs)
                        .text_color(cx.theme().danger)
                        .child(reason),
                )
            })
            .decoration_over(&mut decoration)
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::Progressive;

    #[test]
    fn maps_all_shared_lifecycle_states() {
        assert!(
            !CodeBlock::streamed("pending", &Progressive::pending("code".to_owned())).streaming
        );
        assert!(CodeBlock::streamed("running", &Progressive::running("code".to_owned())).streaming);
        assert!(
            CodeBlock::streamed("complete", &Progressive::complete("code".to_owned()))
                .failed
                .is_none()
        );
        assert_eq!(
            CodeBlock::streamed("failed", &Progressive::failed("code".to_owned(), "stopped"))
                .failed,
            Some("stopped".into())
        );
    }
}
