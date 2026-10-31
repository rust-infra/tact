//! Collapsible tool-call cards and the group that folds a burst of them.
//!
//! A [`ToolCall`] shows one invocation: its name, a one-line summary, the
//! shared status vocabulary, and — when expanded — the input the agent sent,
//! the output it received, or the failure reason. Calls that need a human
//! decision carry Allow / Deny controls. A [`ToolGroup`] collapses several
//! consecutive calls behind one shimmering "Running N tools…" header so a
//! transcript stays readable while an agent works.
//!
//! Both are controlled: the application owns every invocation as a
//! [`Progressive<ToolInvocation>`] and decides what each typed event means.

use crate::ButtonLabelExt as _;
use crate::control::ControlMetricsExt as _;
use crate::control::QuietSurfaceExt as _;
use crate::cues::{self, Cue};
use crate::{
    code_block::CodeBlock,
    control::{PressReleaseExt as _, composed_button},
    handlers::{Handler, SharedHandler},
    motion::{Shimmer, disclosure_progress},
    sizing::SizeTokens,
    status::{StatusBadge, StatusTone},
    stream::{ProgressState, Progressive},
    surface::{eyebrow, inset, leading_glyph_slot, meta},
    theme::SemanticStyledExt as _,
};
use gpui::{
    AnyElement, App, ClickEvent, ElementId, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement, RenderOnce, Role, SharedString, StatefulInteractiveElement as _,
    StyleRefinement, Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, IconNamed, Sizable as _, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    text::TextView,
    v_flex,
};
use std::{rc::Rc, time::Duration};

/// Whether a person must allow a tool call before it runs, and what they decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolApproval {
    /// The call runs without a decision.
    #[default]
    NotRequired,
    /// The call is paused until someone allows or denies it.
    Requested,
    /// Someone allowed the call.
    Approved,
    /// Someone denied the call.
    Rejected,
}

/// Application-owned description of one tool invocation.
///
/// The lifecycle (pending, running, complete, failed) comes from the
/// enclosing [`Progressive`]; this type carries only what the call is and
/// what it produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolInvocation {
    id: SharedString,
    name: SharedString,
    summary: Option<SharedString>,
    input: Option<SharedString>,
    input_language: SharedString,
    output: Option<SharedString>,
    elapsed: Option<Duration>,
    approval: ToolApproval,
    icon: Option<SharedString>,
}

impl ToolInvocation {
    /// Creates an invocation with a stable application identifier and the
    /// tool name shown in the header.
    pub fn new(id: impl Into<SharedString>, name: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            summary: None,
            input: None,
            input_language: "json".into(),
            output: None,
            elapsed: None,
            approval: ToolApproval::NotRequired,
            icon: None,
        }
    }

    /// Adds a one-line summary of the arguments (shown in the header).
    pub fn summary(mut self, summary: impl Into<SharedString>) -> Self {
        self.summary = Some(summary.into());
        self
    }

    /// Adds the full input the agent sent, rendered as code.
    pub fn input(mut self, input: impl Into<SharedString>) -> Self {
        self.input = Some(input.into());
        self
    }

    /// Sets the language used to highlight the input (default `json`).
    pub fn input_language(mut self, language: impl Into<SharedString>) -> Self {
        self.input_language = language.into();
        self
    }

    /// Adds the tool's result, rendered as selectable Markdown.
    pub fn output(mut self, output: impl Into<SharedString>) -> Self {
        self.output = Some(output.into());
        self
    }

    /// Adds caller-measured elapsed time; the component never owns a clock.
    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }

    /// Sets the human approval state.
    pub fn approval(mut self, approval: ToolApproval) -> Self {
        self.approval = approval;
        self
    }

    /// Replaces the default terminal glyph with a tool-specific icon.
    pub fn icon(mut self, icon: impl IconNamed) -> Self {
        self.icon = Some(icon.path());
        self
    }

    /// Returns the stable invocation identifier.
    pub fn id(&self) -> &SharedString {
        &self.id
    }

    /// Returns the tool name.
    pub fn name(&self) -> &SharedString {
        &self.name
    }

    /// Returns the one-line summary, when present.
    pub fn summary_text(&self) -> Option<&SharedString> {
        self.summary.as_ref()
    }

    /// Returns the approval state.
    pub fn approval_state(&self) -> ToolApproval {
        self.approval
    }
}

/// An interaction emitted by [`ToolCall`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCallEvent {
    /// Requests a controlled expansion-state change.
    Toggled {
        /// Stable invocation identifier.
        id: SharedString,
        /// Proposed expansion state.
        open: bool,
    },
    /// The user allowed a call that was awaiting approval.
    Approved {
        /// Stable invocation identifier.
        id: SharedString,
    },
    /// The user denied a call that was awaiting approval.
    Rejected {
        /// Stable invocation identifier.
        id: SharedString,
    },
}

/// A collapsible card for one tool invocation.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// let call = Progressive::complete(
///     ToolInvocation::new("read-1", "read_file")
///         .summary("pricing.md")
///         .input("{ \"path\": \"pricing.md\" }")
///         .output("Read **214** lines."),
/// );
/// ToolCall::new(&call).on_event(|event, _, _| match event {
///     ToolCallEvent::Approved { id } => { /* run it */ }
///     ToolCallEvent::Rejected { id } => { /* skip it */ }
///     ToolCallEvent::Toggled { id, open } => { /* persist `open` */ }
/// });
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct ToolCallLabels {
    pub input: SharedString,
    pub output: SharedString,
    pub approval: SharedString,
    pub waiting: SharedString,
    pub allow: SharedString,
    pub deny: SharedString,
    pub needs_approval: SharedString,
    pub denied: SharedString,
    pub awaiting_approval: SharedString,
    pub pending: SharedString,
    pub running: SharedString,
    pub completed: SharedString,
    pub failed: SharedString,
}

impl Default for ToolCallLabels {
    fn default() -> Self {
        Self {
            input: "Input".into(),
            output: "Output".into(),
            approval: "Approval".into(),
            waiting: "This call is waiting for your decision".into(),
            allow: "Allow".into(),
            deny: "Deny".into(),
            needs_approval: "Needs approval".into(),
            denied: "Denied".into(),
            awaiting_approval: "awaiting approval".into(),
            pending: "Pending".into(),
            running: "Running".into(),
            completed: "Completed".into(),
            failed: "Failed".into(),
        }
    }
}

/// A collapsible tool invocation card.
#[derive(IntoElement)]
pub struct ToolCall {
    style: StyleRefinement,
    state: ProgressState,
    invocation: ToolInvocation,
    open: Option<bool>,
    on_event: Option<SharedHandler<ToolCallEvent>>,
    labels: ToolCallLabels,
}

impl ToolCall {
    /// Creates a card from a progressive invocation snapshot.
    ///
    /// Without an explicit [`Self::open`], the card expands when the call
    /// failed or awaits approval and stays collapsed otherwise.
    pub fn new(call: &Progressive<ToolInvocation>) -> Self {
        Self {
            style: StyleRefinement::default(),
            state: call.state().clone(),
            invocation: call.content().clone(),
            open: None,
            on_event: None,
            labels: ToolCallLabels::default(),
        }
    }

    /// Replaces the card's user-visible labels.
    pub fn labels(mut self, labels: ToolCallLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Sets the expansion state explicitly, replacing the automatic policy.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Handles typed card interactions. Without a handler the card is a
    /// static, non-interactive summary.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ToolCallEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }

    /// Whether the body is shown: the explicit value, or "open when it needs
    /// attention" (failed, or awaiting approval).
    pub fn is_open(&self) -> bool {
        self.open.unwrap_or(
            matches!(self.state, ProgressState::Failed(_))
                || self.invocation.approval == ToolApproval::Requested,
        )
    }

    fn accessibility_label(&self) -> SharedString {
        let status = match self.invocation.approval {
            ToolApproval::Requested => self.labels.awaiting_approval.clone(),
            ToolApproval::Rejected => self.labels.denied.clone(),
            ToolApproval::NotRequired | ToolApproval::Approved => match self.state {
                ProgressState::Pending => self.labels.pending.clone(),
                ProgressState::Running => self.labels.running.clone(),
                ProgressState::Complete => self.labels.completed.clone(),
                ProgressState::Failed(_) => self.labels.failed.clone(),
            },
        };
        match &self.invocation.summary {
            Some(summary) => format!("{} {}, {}", self.invocation.name, summary, status).into(),
            None => format!("{}, {}", self.invocation.name, status).into(),
        }
    }

    fn status_glyph(&self, window: &mut Window, cx: &mut App) -> AnyElement {
        // The terminal glyphs settle in rather than popping, once per state
        // and never on a re-render; the state a card mounts with is exempt.
        crate::status::progress_glyph(
            &self.state,
            ElementId::from((ElementId::from(self.invocation.id.clone()), "glyph")),
            crate::sizing::slot_sm(cx),
            window,
            cx,
        )
        .into_any_element()
    }
}

impl Styled for ToolCall {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ToolCall {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let labels = self.labels.clone();
        let tokens = cx.theme().semantic_tokens();
        let open = self.is_open();
        let id = self.invocation.id.clone();
        let root_id = ElementId::from(id.clone());
        let disclosure = disclosure_progress((root_id.clone(), "disclosure"), open, window, cx);
        let disclosure_opacity =
            crate::motion::disclosure_fade((root_id.clone(), "disclosure"), open, window, cx);
        // Opacity does not make descendants inert. Close the interaction tree
        // immediately; the header alone retains the closing transition.
        let showing = open;
        // The capsule morph: a closed header-only card sits one step
        // rounder — twice the card radius reads as a capsule on a
        // single-line header — and relaxes to the card radius as the body
        // opens. It rides the disclosure sample the body already animates
        // on, so it costs no clock and snaps whenever the disclosure
        // snaps. The header's own states are drawn inside it.
        let card_radius = tokens.radius.lg + tokens.radius.lg * (1.0 - disclosure);
        let accessibility_label = self.accessibility_label();
        let interactive = self.on_event.is_some();
        let handler = self.on_event.clone();
        let approval = self.invocation.approval;
        let failure = match &self.state {
            ProgressState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let badge = match approval {
            ToolApproval::Requested => {
                StatusBadge::new((root_id.clone(), "status"), labels.needs_approval.clone())
                    .tone(StatusTone::Warning)
            }
            ToolApproval::Rejected => {
                StatusBadge::new((root_id.clone(), "status"), labels.denied.clone())
                    .tone(StatusTone::Neutral)
            }
            ToolApproval::NotRequired | ToolApproval::Approved => {
                StatusBadge::for_progress((root_id.clone(), "status"), &self.state)
            }
        };
        let tool_icon = match &self.invocation.icon {
            Some(path) => Icon::default().path(path.clone()),
            None => Icon::new(IconName::SquareTerminal),
        };
        let header = h_flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(tokens.spacing.sm)
            .child(
                // A fixed square slot: spinner, check, cross, and the
                // pending dot all centre in the same box, so a lifecycle
                // change never nudges the name sideways.
                div()
                    .flex_none()
                    .size(crate::sizing::slot_xs(cx))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(self.status_glyph(window, cx)),
            )
            .child(tool_icon.xsmall().text_color(cx.theme().muted_foreground))
            .child(
                div()
                    .flex_none()
                    .text_token(tokens.typography.sm)
                    .font_weight(FontWeight::MEDIUM)
                    .font_family(cx.theme().mono_font_family.clone())
                    .text_color(cx.theme().foreground)
                    .child(self.invocation.name.clone()),
            )
            .when_some(self.invocation.summary.clone(), |this, summary| {
                this.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().muted_foreground)
                        .child(summary),
                )
            })
            .when(self.invocation.summary.is_none(), |this| {
                this.child(div().flex_1())
            })
            .when_some(self.invocation.elapsed, |this, elapsed| {
                this.child(meta(format_elapsed(elapsed), cx).flex_none())
            })
            .child(badge)
            .when(interactive, |this| {
                this.child(
                    crate::surface::disclosure_chevron(disclosure)
                        .text_color(cx.theme().muted_foreground),
                )
            });
        let header = match handler.clone() {
            Some(handler) => {
                let event = ToolCallEvent::Toggled {
                    id: id.clone(),
                    open: !open,
                };
                let toggle_debug_id = id.clone();
                composed_button((root_id.clone(), "toggle"), accessibility_label.clone())
                    .debug_selector(move || format!("tool-call-toggle-{toggle_debug_id}"))
                    .aria_expanded(open)
                    .w_full()
                    .px(tokens.spacing.md)
                    .py(tokens.spacing.sm)
                    .border_1()
                    .border_color(cx.theme().transparent)
                    // The header's states are drawn inside the card's own
                    // corners: a closed card is header-only and takes the
                    // whole capsule, an open one rounds its top alone. A
                    // square fill under a rounded card was the corner the
                    // 0.4.0 feel review saw poking out.
                    .rounded_t(card_radius)
                    .when(!showing, |header| header.rounded_b(card_radius))
                    .hover(|style| style.bg(cx.theme().accent.opacity(0.6)))
                    .active(|style| style.bg(cx.theme().accent))
                    .focus_visible(|style| style.border_color(cx.theme().ring))
                    .press_release(
                        ElementId::from((root_id.clone(), "toggle")),
                        card_radius,
                        window,
                        cx,
                    )
                    .child(header)
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                    .into_any_element()
            }
            None => header
                .px(tokens.spacing.md)
                .py(tokens.spacing.sm)
                .into_any_element(),
        };

        let input = self.invocation.input.clone();
        let input_language = self.invocation.input_language.clone();
        let output = self.invocation.output.clone();
        let body_debug_id = id.clone();
        let body = v_flex()
            .debug_selector(move || format!("tool-call-body-{body_debug_id}"))
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.sm)
            .px(tokens.spacing.md)
            .pb(tokens.spacing.md)
            .border_t_1()
            .border_color(crate::surface::hairline(cx))
            .pt(tokens.spacing.sm)
            .when(approval == ToolApproval::Requested, |this| {
                let approve = handler.clone();
                let reject = handler.clone();
                let approve_id = id.clone();
                let reject_id = id.clone();
                this.child(
                    h_flex()
                        .id((root_id.clone(), "approval"))
                        .role(Role::Group)
                        .aria_label(labels.approval.clone())
                        .w_full()
                        .items_center()
                        .justify_between()
                        .gap(tokens.spacing.sm)
                        .child(
                            div()
                                .text_token(tokens.typography.sm)
                                .text_color(cx.theme().foreground)
                                .child(labels.waiting.clone()),
                        )
                        .child(
                            h_flex()
                                .gap(tokens.spacing.xs)
                                .when_some(approve, |this, handler| {
                                    let allow_debug_id = approve_id.clone();
                                    this.child(
                                        div()
                                            .debug_selector(move || {
                                                format!("tool-call-allow-{allow_debug_id}")
                                            })
                                            .child(
                                                Button::new((root_id.clone(), "allow"))
                                                    .primary()
                                                    .small()
                                                    .control_metrics(cx)
                                                    .accessibility_id(format!("{id}-allow"))
                                                    .text_label(labels.allow.clone())
                                                    .on_click(move |_: &ClickEvent, window, cx| {
                                                        cues::emit(
                                                            cx,
                                                            Cue::Decided { approved: true },
                                                        );
                                                        handler(
                                                            &ToolCallEvent::Approved {
                                                                id: approve_id.clone(),
                                                            },
                                                            window,
                                                            cx,
                                                        )
                                                    }),
                                            ),
                                    )
                                })
                                .when_some(reject, |this, handler| {
                                    let deny_debug_id = reject_id.clone();
                                    this.child(
                                        div()
                                            .debug_selector(move || {
                                                format!("tool-call-deny-{deny_debug_id}")
                                            })
                                            .child(
                                                Button::new((root_id.clone(), "deny"))
                                                    .outline()
                                                    .small()
                                                    .control_metrics(cx)
                                                    .accessibility_id(format!("{id}-deny"))
                                                    .text_label(labels.deny.clone())
                                                    .on_click(move |_: &ClickEvent, window, cx| {
                                                        cues::emit(
                                                            cx,
                                                            Cue::Decided { approved: false },
                                                        );
                                                        handler(
                                                            &ToolCallEvent::Rejected {
                                                                id: reject_id.clone(),
                                                            },
                                                            window,
                                                            cx,
                                                        )
                                                    }),
                                            ),
                                    )
                                }),
                        ),
                )
            })
            .when_some(input, |this, input| {
                this.child(eyebrow(labels.input.clone(), cx)).child(
                    CodeBlock::new((root_id.clone(), "input"), input).language(input_language),
                )
            })
            .when_some(output, |this, output| {
                this.child(eyebrow(labels.output.clone(), cx)).child(
                    inset(cx)
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().foreground)
                        .child(
                            TextView::markdown((root_id.clone(), "output"), output)
                                .selectable(true),
                        ),
                )
            })
            .when_some(failure.clone(), |this, reason| {
                // The glyph rides a first-line slot: the row stays
                // items_start, so however far the reason wraps, the
                // triangle holds centered on the first line.
                this.child(
                    h_flex()
                        .items_start()
                        .gap(tokens.spacing.sm)
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().danger)
                        .child(
                            leading_glyph_slot(
                                SizeTokens::read(cx).slot_md(),
                                Icon::new(IconName::TriangleAlert).small(),
                            )
                            .debug_selector(|| "tool-call-failure-glyph".into()),
                        )
                        .child(
                            div()
                                .min_w_0()
                                .debug_selector(|| "tool-call-failure-reason".into())
                                .child(reason),
                        ),
                )
            });

        let card_debug_id = id.clone();
        v_flex()
            .id((root_id.clone(), "card"))
            .debug_selector(move || format!("tool-call-card-{card_debug_id}"))
            .role(Role::Group)
            .aria_label(accessibility_label)
            .when_some(failure, |this, reason| this.aria_description(reason))
            .w_full()
            .min_w_0()
            .bg(tokens.colors.surface)
            .border_1()
            .border_color(match approval {
                ToolApproval::Requested => cx.theme().warning,
                _ => cx.theme().border,
            })
            .rounded(card_radius)
            .overflow_hidden()
            .child(header)
            .when(showing, |this| {
                // Opening content grows into its own height and fades in;
                // closing removes descendants immediately so input and
                // semantics match aria_expanded.
                let clip_debug_id = id.clone();
                this.child(
                    crate::motion::disclosure_clip(
                        ElementId::from((root_id.clone(), "disclosure-clip")),
                        disclosure,
                        div().opacity(disclosure_opacity).child(body),
                        window,
                        cx,
                    )
                    .debug_selector(move || format!("tool-call-disclosure-clip-{clip_debug_id}")),
                )
            })
            .refine_style(&self.style)
    }
}

/// An interaction emitted by [`ToolGroup`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolGroupEvent {
    /// Requests a controlled expansion-state change.
    Toggled {
        /// Stable group identifier.
        id: SharedString,
        /// Proposed expansion state.
        open: bool,
    },
}

/// Folds consecutive tool calls behind one header.
///
/// The header shimmers while the group is active and reads "N tool calls"
/// once it settles. Children are any elements — typically [`ToolCall`]s.
///
/// # Example
///
/// ```no_run
/// # use gpui_ai::prelude::*;
/// # use gpui::ParentElement;
/// # fn example(calls: Vec<Progressive<ToolInvocation>>) {
/// ToolGroup::new("burst-1")
///     .count(3)
///     .active(true)
///     .children(calls.iter().map(ToolCall::new));
/// # }
/// ```
#[derive(IntoElement)]
pub struct ToolGroup {
    id: SharedString,
    style: StyleRefinement,
    title: Option<SharedString>,
    count: usize,
    active: bool,
    open: Option<bool>,
    children: Vec<AnyElement>,
    on_event: Option<Handler<ToolGroupEvent>>,
}

impl ToolGroup {
    /// Creates a group with a stable identifier.
    pub fn new(id: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            title: None,
            count: 0,
            active: false,
            open: None,
            children: Vec::new(),
            on_event: None,
        }
    }

    /// Overrides the generated "N tool calls" title.
    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    /// Sets the number of calls the group represents.
    pub fn count(mut self, count: usize) -> Self {
        self.count = count;
        self
    }

    /// Marks the group as still running: the title shimmers and the group
    /// opens unless told otherwise.
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }

    /// Sets the expansion state explicitly, replacing "open while active".
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Handles typed group interactions.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ToolGroupEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Box::new(handler));
        self
    }

    /// Whether the calls are shown: the explicit value, or "open while active".
    pub fn is_open(&self) -> bool {
        self.open.unwrap_or(self.active)
    }

    fn resolved_title(&self) -> SharedString {
        match &self.title {
            Some(title) => title.clone(),
            None if self.active => format!(
                "Running {} tool{}…",
                self.count,
                if self.count == 1 { "" } else { "s" }
            )
            .into(),
            None => format!(
                "{} tool call{}",
                self.count,
                if self.count == 1 { "" } else { "s" }
            )
            .into(),
        }
    }
}

impl ParentElement for ToolGroup {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl Styled for ToolGroup {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for ToolGroup {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let open = self.is_open();
        let title = self.resolved_title();
        let root_id = ElementId::from(self.id.clone());
        let disclosure = disclosure_progress((root_id.clone(), "disclosure"), open, window, cx);
        let disclosure_opacity =
            crate::motion::disclosure_fade((root_id.clone(), "disclosure"), open, window, cx);
        let showing = open;

        // ParentElement accepts opaque children without domain identity.
        // Keep their content at rest rather than assigning mutable positions
        // to arrival clocks; the group's disclosure owns its transition.
        let call_count = self.children.len();
        let mut calls = Vec::with_capacity(if showing { call_count } else { 0 });
        if showing {
            for child in self.children {
                calls.push(div().w_full().min_w_0().child(child));
            }
        }
        let interactive = self.on_event.is_some();
        let header = h_flex()
            .items_center()
            .gap(tokens.spacing.sm)
            .text_token(tokens.typography.sm)
            .text_color(cx.theme().muted_foreground)
            .when(interactive, |this| {
                this.child(crate::surface::disclosure_chevron(disclosure))
            })
            .child(Icon::new(IconName::SquareTerminal).xsmall())
            .child(
                // The shimmer speaks for work the reader cannot see. Open,
                // the calls report their own status, so the title rests —
                // one dominant active-work signal, never two. The title's
                // text keeps saying "Running…" either way: semantics are
                // carried by words, not by the highlight.
                Shimmer::new((root_id.clone(), "title"), title.clone())
                    .active(self.active && !open)
                    .text_token(tokens.typography.sm),
            )
            .when(self.count > 0, |this| {
                this.child(
                    div()
                        .px(tokens.spacing.xs)
                        .rounded(tokens.radius.full)
                        .bg(cx.theme().muted)
                        .text_token(tokens.typography.xs)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(self.count.to_string()),
                )
            });
        let toggle = match self.on_event {
            Some(handler) => {
                let event = ToolGroupEvent::Toggled {
                    id: self.id.clone(),
                    open: !open,
                };
                let group_debug_id = self.id.clone();
                composed_button((root_id.clone(), "toggle"), title.clone())
                    .debug_selector(move || format!("tool-group-toggle-{group_debug_id}"))
                    .aria_expanded(open)
                    .px(tokens.spacing.xs)
                    .py(tokens.spacing.xxs)
                    .quiet_press_surface(
                        ElementId::from((root_id.clone(), "toggle")),
                        tokens.radius.md,
                        window,
                        cx,
                    )
                    .child(header)
                    .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                    .into_any_element()
            }
            None => header.into_any_element(),
        };

        v_flex()
            .id(root_id.clone())
            .role(Role::Group)
            .aria_label(title.clone())
            .w_full()
            .min_w_0()
            .gap(tokens.spacing.xs)
            .child(toggle)
            .when(showing, |this| {
                // Only expanded groups expose their child controls.
                this.child(
                    v_flex()
                        .id((root_id, "calls"))
                        .debug_selector({
                            let calls_debug_id = self.id.clone();
                            move || format!("tool-group-calls-{calls_debug_id}")
                        })
                        .role(Role::List)
                        .aria_label(title.clone())
                        .w_full()
                        .min_w_0()
                        .gap(tokens.spacing.xs)
                        .pl(tokens.spacing.md)
                        .ml(tokens.spacing.xs)
                        .border_l_1()
                        .border_color(cx.theme().border)
                        .opacity(disclosure_opacity)
                        .top(tokens.spacing.xxs * (1.0 - disclosure) * crate::motion::travel(cx))
                        .children(calls),
                )
            })
            .refine_style(&self.style)
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 1.0 {
        format!("{:.0}ms", secs * 1000.0)
    } else if secs < 10.0 {
        format!("{secs:.1}s")
    } else if secs < 60.0 {
        format!("{secs:.0}s")
    } else {
        let minutes = (secs / 60.0).floor() as u64;
        let rest = secs as u64 % 60;
        format!("{minutes}m {rest:02}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Entity, Render, TestAppContext, VisualTestContext, px};

    struct GroupProbe {
        open: bool,
        count: usize,
    }

    impl Render for GroupProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div().w(px(360.)).h(px(480.)).child(
                ToolGroup::new("burst")
                    .count(self.count)
                    .open(self.open)
                    .children((0..self.count).map(|index| {
                        div()
                            .h(px(20.))
                            .child(format!("call {index}"))
                            .into_any_element()
                    })),
            )
        }
    }

    fn open_group(cx: &mut TestAppContext) -> (Entity<GroupProbe>, &mut VisualTestContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| GroupProbe {
            open: true,
            count: 3,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (probe, cx)
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Advances past the disclosure fade and any arrival reveals.
    fn settle(cx: &mut VisualTestContext) {
        cx.executor().advance_clock(Duration::from_secs(2));
        draw(cx);
        draw(cx);
    }

    #[gpui::test]
    fn a_groups_first_open_and_every_reopen_join_at_rest(cx: &mut TestAppContext) {
        let (probe, cx) = open_group(cx);
        settle(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "the opening fade owns the first reveal; calls join at rest"
        );

        probe.update(cx, |probe, cx| {
            probe.open = false;
            cx.notify();
        });
        settle(cx);
        probe.update(cx, |probe, cx| {
            probe.open = true;
            cx.notify();
        });
        settle(cx);
        crate::motion::take_reveal_frame_requests();
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "an acknowledged call must never cascade again on reopen"
        );
    }

    #[gpui::test]
    fn unkeyed_group_children_do_not_acquire_positional_arrival_motion(cx: &mut TestAppContext) {
        let (probe, cx) = open_group(cx);
        settle(cx);
        crate::motion::take_reveal_frame_requests();

        probe.update(cx, |probe, cx| {
            probe.count = 4;
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "opaque children cannot acquire arrival state by mutable position"
        );

        settle(cx);
        crate::motion::take_reveal_frame_requests();
        probe.update(cx, |_, cx| cx.notify());
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "a re-render must not replay an acknowledged arrival"
        );
    }

    #[gpui::test]
    fn reduced_motion_appends_at_rest(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let (probe, cx) = open_group(cx);
        settle(cx);
        crate::motion::take_reveal_frame_requests();

        probe.update(cx, |probe, cx| {
            probe.count = 4;
            cx.notify();
        });
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "reduced motion appends without frames"
        );
    }

    fn invocation() -> ToolInvocation {
        ToolInvocation::new("read-1", "read_file").summary("pricing.md")
    }

    #[test]
    fn automatic_policy_opens_only_when_attention_is_needed() {
        assert!(!ToolCall::new(&Progressive::pending(invocation())).is_open());
        assert!(!ToolCall::new(&Progressive::running(invocation())).is_open());
        assert!(!ToolCall::new(&Progressive::complete(invocation())).is_open());
        assert!(ToolCall::new(&Progressive::failed(invocation(), "timeout")).is_open());
        assert!(
            ToolCall::new(&Progressive::pending(
                invocation().approval(ToolApproval::Requested)
            ))
            .is_open()
        );
        assert!(
            !ToolCall::new(&Progressive::failed(invocation(), "timeout"))
                .open(false)
                .is_open()
        );
    }

    #[test]
    fn accessibility_label_carries_name_summary_and_decision_state() {
        let running = ToolCall::new(&Progressive::running(invocation()));
        assert_eq!(
            running.accessibility_label(),
            "read_file pricing.md, Running"
        );
        let awaiting = ToolCall::new(&Progressive::pending(
            ToolInvocation::new("send", "send_email").approval(ToolApproval::Requested),
        ));
        assert_eq!(
            awaiting.accessibility_label(),
            "send_email, awaiting approval"
        );
        let denied = ToolCall::new(&Progressive::complete(
            ToolInvocation::new("send", "send_email").approval(ToolApproval::Rejected),
        ));
        assert_eq!(denied.accessibility_label(), "send_email, Denied");
    }

    #[test]
    fn group_title_reflects_activity_and_count() {
        assert_eq!(
            ToolGroup::new("g").count(3).active(true).resolved_title(),
            "Running 3 tools…"
        );
        assert_eq!(
            ToolGroup::new("g").count(1).active(true).resolved_title(),
            "Running 1 tool…"
        );
        assert_eq!(
            ToolGroup::new("g").count(2).resolved_title(),
            "2 tool calls"
        );
        assert_eq!(
            ToolGroup::new("g")
                .count(2)
                .title("Research")
                .resolved_title(),
            "Research"
        );
        assert!(ToolGroup::new("g").active(true).is_open());
        assert!(!ToolGroup::new("g").is_open());
        assert!(ToolGroup::new("g").open(true).is_open());
    }

    #[test]
    fn elapsed_formatting_covers_sub_second_to_minutes() {
        assert_eq!(format_elapsed(Duration::from_millis(340)), "340ms");
        assert_eq!(format_elapsed(Duration::from_millis(1400)), "1.4s");
        assert_eq!(format_elapsed(Duration::from_secs(12)), "12s");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 05s");
    }
}
