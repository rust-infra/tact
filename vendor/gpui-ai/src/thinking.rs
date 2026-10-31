//! Expandable progressive reasoning traces.
//!
//! While reasoning runs the disclosure opens on its own, its title shimmers,
//! and a live preview follows the newest steps — until the reader scrolls
//! back through earlier ones, which holds their position until they return to
//! the tail. Once the trace settles it collapses to "Thought for Ns" and the
//! full trace stays one click (or Enter) away. Applications may override the
//! automatic policy with [`Thinking::open`].

use crate::control::QuietSurfaceExt as _;
use crate::{
    control::composed_button,
    handlers::Handler,
    motion::{ArrivalRoster, MotionTokens, Shimmer, disclosure_progress, reveal},
    stream::{ProgressState, Progressive},
    theme::SemanticStyledExt as _,
};
use gpui::{
    App, ClickEvent, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    RenderOnce, Role, ScrollHandle, SharedString, StatefulInteractiveElement as _, StyleRefinement,
    Styled, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, StyledExt as _, h_flex, spinner::Spinner, text::TextView,
    v_flex,
};
use std::{
    hash::{DefaultHasher, Hash as _, Hasher as _},
    mem::discriminant,
    time::Duration,
};

/// Status of one step inside a [`ThinkingTrace`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StepStatus {
    /// The step is in progress; it shows a spinner.
    #[default]
    Running,
    /// The step completed.
    Done,
}

/// One step of a reasoning trace: a title plus optional detail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThinkingStep {
    id: Option<SharedString>,
    title: SharedString,
    detail: Option<SharedString>,
    status: StepStatus,
}

impl ThinkingStep {
    /// Creates a step with a short title.
    pub fn new(title: impl Into<SharedString>) -> Self {
        Self {
            id: None,
            title: title.into(),
            detail: None,
            status: StepStatus::default(),
        }
    }

    /// Names the step's stable identity.
    ///
    /// Motion is keyed by this ID, so a step keeps its completion
    /// acknowledgment and its arrival across insertion and reordering.
    /// Without one the step falls back to its position, which is stable only
    /// while nothing is inserted above it — a legacy snapshot renders fine,
    /// but only identified steps may ever animate a reorder.
    pub fn id(mut self, id: impl Into<SharedString>) -> Self {
        self.id = Some(id.into());
        self
    }

    /// Adds detail rendered as muted markdown under the title.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Sets the step status.
    pub fn status(mut self, status: StepStatus) -> Self {
        self.status = status;
        self
    }

    /// The key this step's motion is stable under: its declared identity, or
    /// its position for legacy snapshots.
    fn motion_key(&self, trace_id: &SharedString, channel: &str, ix: usize) -> ElementId {
        match &self.id {
            Some(id) => ElementId::Name(format!("{trace_id}-step-{id}-{channel}").into()),
            None => ElementId::NamedInteger(format!("{trace_id}-step-{channel}").into(), ix as u64),
        }
    }
}

/// Typed content of a progressive thinking trace.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ThinkingTrace {
    prose: Option<SharedString>,
    steps: Vec<ThinkingStep>,
    thought_for: Option<Duration>,
}

impl ThinkingTrace {
    /// Creates an empty trace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets free-form reasoning rendered before structured steps.
    pub fn prose(mut self, prose: impl Into<SharedString>) -> Self {
        self.prose = Some(prose.into());
        self
    }

    /// Sets structured trace steps.
    pub fn steps(mut self, steps: impl IntoIterator<Item = ThinkingStep>) -> Self {
        self.steps = steps.into_iter().collect();
        self
    }

    /// Sets the caller-measured thinking duration.
    pub fn thought_for(mut self, duration: Duration) -> Self {
        self.thought_for = Some(duration);
        self
    }
}

/// An interaction emitted by [`Thinking`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ThinkingEvent {
    /// Requests a controlled expansion-state change.
    Toggled {
        /// Stable trace identifier.
        id: SharedString,
        /// Proposed expansion state.
        open: bool,
    },
}

/// An accessible reasoning disclosure with an automatic streaming policy.
#[derive(Clone, Debug, PartialEq, Eq)]
#[allow(missing_docs)]
pub struct ThinkingLabels {
    pub thinking: SharedString,
    pub stopped: SharedString,
    pub thought_for_prefix: SharedString,
    pub seconds_suffix: SharedString,
    pub thoughts: SharedString,
    pub step_running: SharedString,
    pub step_done: SharedString,
}

impl Default for ThinkingLabels {
    fn default() -> Self {
        Self {
            thinking: "Thinking…".into(),
            stopped: "Thinking stopped".into(),
            thought_for_prefix: "Thought for ".into(),
            seconds_suffix: "s".into(),
            thoughts: "Thoughts".into(),
            step_running: "in progress".into(),
            step_done: "complete".into(),
        }
    }
}

/// An accessible reasoning disclosure with an automatic streaming policy.
#[derive(IntoElement)]
pub struct Thinking {
    id: SharedString,
    style: StyleRefinement,
    open: Option<bool>,
    state: ProgressState,
    trace: ThinkingTrace,
    revision: u64,
    on_event: Option<Handler<ThinkingEvent>>,
    labels: ThinkingLabels,
}

impl Thinking {
    /// Creates a trace from a progressive snapshot.
    ///
    /// Without an explicit [`Self::open`] the trace is expanded while
    /// reasoning runs and collapsed once it settles.
    pub fn new(id: impl Into<SharedString>, trace: &Progressive<ThinkingTrace>) -> Self {
        Self {
            id: id.into(),
            style: StyleRefinement::default(),
            open: None,
            state: trace.state().clone(),
            trace: trace.content().clone(),
            revision: trace.revision(),
            on_event: None,
            labels: ThinkingLabels::default(),
        }
    }

    /// Replaces the disclosure's user-visible labels.
    pub fn labels(mut self, labels: ThinkingLabels) -> Self {
        self.labels = labels;
        self
    }

    /// Sets the expansion state explicitly, replacing the automatic policy.
    pub fn open(mut self, open: bool) -> Self {
        self.open = Some(open);
        self
    }

    /// Handles typed trace interactions.
    pub fn on_event(
        mut self,
        handler: impl Fn(&ThinkingEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Box::new(handler));
        self
    }

    /// Whether the body is shown: the explicit value, or "open while running".
    pub fn is_open(&self) -> bool {
        self.open.unwrap_or(self.state == ProgressState::Running)
    }
}

impl Styled for Thinking {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

/// Live-preview state for one trace, held in keyed window state.
///
/// A [`RenderOnce`] trace remembers nothing across frames, so without this it
/// cannot tell newly streamed reasoning from a re-render and drags the
/// preview back to the tail on every frame. The key carries the trace's
/// stable ID, so a different trace starts with a fresh scroll position and a
/// fresh follow decision.
struct LivePreview {
    scroll: ScrollHandle,
    /// Digest of the content the preview last showed.
    revision: Option<u64>,
    /// Whether the user was at the tail when the preview last rendered.
    follow: bool,
    /// Which step identities the preview has shown, and what arrival delay
    /// each fresh one carries. Keyed by the step's motion key, so an
    /// identified step stays seen wherever it moves, while an unidentified
    /// one is only as stable as its position — the documented fallback
    /// limit.
    arrivals: ArrivalRoster,
}

impl LivePreview {
    fn new() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            revision: None,
            follow: true,
            arrivals: ArrivalRoster::new(),
        }
    }

    /// Records `revision` and the position the user left the preview in, and
    /// answers whether this render should land on the tail. Content the
    /// preview has already shown never scrolls, and neither does new content
    /// once the user has scrolled away to read earlier reasoning.
    fn observe(&mut self, revision: u64, slack: Pixels) -> bool {
        self.follow = follows_tail(self.scroll.offset().y, self.scroll.max_offset().y, slack);
        let arrived = self.revision != Some(revision);
        self.revision = Some(revision);
        arrived && self.follow
    }

    /// Takes the roll call of this render's step identities and assigns one
    /// decelerating cascade to the identities not seen before.
    ///
    /// The first roll call is history and joins at rest. Later batches
    /// cascade only when `assign` and the reader is following: the caller
    /// passes `assign` only while the disclosure rests fully open, because
    /// during a body fade-in the fade owns the acknowledgment — one dominant
    /// signal, never two. Reasoning that streams in behind a reader who
    /// scrolled away likewise appears at rest when they return. Either way
    /// the identity is marked seen, so nothing retro-animates.
    fn note_steps(
        &mut self,
        keys: impl Iterator<Item = ElementId>,
        assign_arrivals: bool,
        tokens: &MotionTokens,
        now: crate::motion::Instant,
    ) {
        let follow = self.follow;
        self.arrivals
            .note(keys, assign_arrivals && follow, tokens, now);
    }

    /// The arrival delay this step identity was assigned, or `None` for one
    /// that appears at rest.
    #[cfg(test)]
    fn arrival_delay(&self, key: &ElementId) -> Option<Duration> {
        self.arrivals.delay(key)
    }
}

/// Whether a preview resting at `offset_y` still counts as following the tail.
///
/// GPUI scroll offsets are zero at the top of the content and `-max_offset` at
/// the bottom, so `offset_y + max_offset_y` is the distance left to the tail.
/// `slack` keeps a preview that stopped a fraction short of the edge — wheel
/// and trackpad gestures rarely land exactly on it — still following.
fn follows_tail(offset_y: Pixels, max_offset_y: Pixels, slack: Pixels) -> bool {
    max_offset_y <= Pixels::ZERO || offset_y + max_offset_y <= slack
}

/// Digest of everything the live preview shows.
///
/// Applications rebuild their [`Progressive`] snapshot every frame, so a trace
/// that grew and one that merely re-rendered both report revision zero. The
/// digest folds the trace itself in, which is what makes appended reasoning
/// distinguishable from a repaint.
fn content_revision(revision: u64, trace: &ThinkingTrace) -> u64 {
    let mut hasher = DefaultHasher::new();
    revision.hash(&mut hasher);
    trace.prose.hash(&mut hasher);
    trace.thought_for.hash(&mut hasher);
    for step in &trace.steps {
        step.id.hash(&mut hasher);
        step.title.hash(&mut hasher);
        step.detail.hash(&mut hasher);
        discriminant(&step.status).hash(&mut hasher);
    }
    hasher.finish()
}

impl RenderOnce for Thinking {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let live = self.state == ProgressState::Running;
        let open = self.is_open();
        let labels = self.labels.clone();
        let title: SharedString = match (&self.state, self.trace.thought_for) {
            (ProgressState::Running, _) => self.labels.thinking.clone(),
            (ProgressState::Failed(_), _) => self.labels.stopped.clone(),
            (_, Some(duration)) => format!(
                "{}{:.0}{}",
                self.labels.thought_for_prefix,
                duration.as_secs_f64(),
                self.labels.seconds_suffix
            )
            .into(),
            _ => self.labels.thoughts.clone(),
        };
        let event = ThinkingEvent::Toggled {
            id: self.id.clone(),
            open: !open,
        };
        let failed = match &self.state {
            ProgressState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let trace_id = self.id.clone();
        let root_id = ElementId::from(self.id.clone());
        let motion = MotionTokens::read(cx).clone();

        let disclosure = disclosure_progress((root_id.clone(), "disclosure"), open, window, cx);
        let disclosure_opacity =
            crate::motion::disclosure_fade((root_id.clone(), "disclosure"), open, window, cx);
        let showing = open;

        // Observed before the steps render, so a batch that arrived this
        // frame has its cascade assigned by the time each row asks for its
        // delay. A collapsed trace has no preview to follow, so it keeps no
        // follow state either: reopening starts again on the newest
        // reasoning.
        let preview = (live && showing).then(|| {
            let revision = content_revision(self.revision, &self.trace);
            let preview = window.use_keyed_state((root_id.clone(), "live-follow"), cx, |_, _| {
                LivePreview::new()
            });
            let (scroll, follow) = preview.update(cx, |state, cx| {
                let follow = state.observe(revision, tokens.typography.sm.line_height);
                state.note_steps(
                    self.trace
                        .steps
                        .iter()
                        .enumerate()
                        .map(|(ix, step)| step.motion_key(&trace_id, "arrive", ix)),
                    disclosure >= 1.0,
                    &motion,
                    cx.background_executor().now(),
                );
                (state.scroll.clone(), follow)
            });
            (preview, scroll, follow)
        });

        let interactive = self.on_event.is_some();
        let header = h_flex()
            .items_center()
            .gap(tokens.spacing.sm)
            .text_token(tokens.typography.sm)
            .text_color(cx.theme().muted_foreground)
            .when(interactive, |this| {
                this.child(crate::surface::disclosure_chevron(disclosure))
            })
            .child(
                Shimmer::new((root_id.clone(), "title"), title.clone())
                    .active(live)
                    .text_token(tokens.typography.sm),
            );
        let toggle = match self.on_event {
            Some(handler) => composed_button(format!("{}-toggle", self.id), title.clone())
                .aria_expanded(open)
                .px(tokens.spacing.xs)
                .py(tokens.spacing.xxs)
                .quiet_press_surface(
                    ElementId::from((root_id.clone(), "toggle-press")),
                    tokens.radius.sm,
                    window,
                    cx,
                )
                .child(header)
                .on_click(move |_: &ClickEvent, window, cx| handler(&event, window, cx))
                .into_any_element(),
            None => header.into_any_element(),
        };

        let body = v_flex()
            .gap(tokens.spacing.sm)
            .pl(tokens.spacing.md)
            .ml(tokens.spacing.xs)
            .border_l_1()
            .border_color(cx.theme().border)
            .when_some(self.trace.prose, |this, prose| {
                this.child(
                    div()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().muted_foreground)
                        .child(TextView::markdown("prose", prose).selectable(true)),
                )
            })
            .children(self.trace.steps.into_iter().enumerate().map(|(ix, step)| {
                let accessibility_label: SharedString = format!(
                    "{}, {}",
                    step.title,
                    match step.status {
                        StepStatus::Running => labels.step_running.as_ref(),
                        StepStatus::Done => labels.step_done.as_ref(),
                    }
                )
                .into();
                let indicator = match step.status {
                    StepStatus::Running => Spinner::new()
                        .xsmall()
                        .color(cx.theme().info)
                        .into_any_element(),
                    // Completion settles into the fixed indicator slot with a
                    // one-shot reveal, keyed by the step's declared identity
                    // where it has one. The positional fallback is stable
                    // only while nothing is inserted above the step — an
                    // insertion shifts every later index and would replay
                    // acknowledgments on the wrong rows — which is why
                    // unidentified steps never animate reordering.
                    StepStatus::Done => reveal(
                        div()
                            .size_1p5()
                            .rounded(tokens.radius.full)
                            .bg(cx.theme().success),
                        step.motion_key(&trace_id, "done", ix),
                        window,
                        cx,
                    )
                    .into_any_element(),
                };
                // Freshly streamed steps settle in on the batch's assigned
                // cascade; history and steps that arrived behind a scrolled
                // reader carry no delay entry and render at rest.
                let arrival_key = step.motion_key(&trace_id, "arrive", ix);
                let arrival = preview.as_ref().and_then(|(state, ..)| {
                    state.update(cx, |state, cx| {
                        state.arrivals.progress(&arrival_key, window, cx)
                    })
                });
                v_flex()
                    .id((trace_id.clone(), ix))
                    .role(Role::ListItem)
                    .aria_label(accessibility_label)
                    .gap(tokens.spacing.xxs)
                    .child(
                        h_flex()
                            .items_center()
                            .gap(tokens.spacing.sm)
                            .text_token(tokens.typography.sm)
                            .text_color(cx.theme().foreground)
                            .child(
                                div()
                                    .flex_none()
                                    .w(tokens.spacing.sm)
                                    .flex()
                                    .justify_center()
                                    .child(indicator),
                            )
                            .child(step.title),
                    )
                    .when_some(step.detail, |this, detail| {
                        this.child(
                            div()
                                .pl(tokens.spacing.md)
                                .text_token(tokens.typography.sm)
                                .text_color(cx.theme().muted_foreground)
                                .child(
                                    TextView::markdown(("step-detail", ix), detail)
                                        .selectable(true),
                                ),
                        )
                    })
                    .when_some(arrival, |this, progress| {
                        this.opacity(progress)
                            .top(tokens.spacing.xxs * (1.0 - progress) * crate::motion::travel(cx))
                    })
            }))
            .when_some(failed.clone(), |this, reason| {
                this.child(
                    div()
                        .text_token(tokens.typography.sm)
                        .text_color(cx.theme().danger)
                        .child(reason),
                )
            })
            .debug_selector(|| format!("thinking-body-{trace_id}"));

        // While reasoning streams, the body is a bounded live preview that
        // follows its newest content; it still scrolls, and the full trace
        // renders unbounded once the state settles.
        let body = match &preview {
            Some((_, scroll, follow)) => {
                // GPUI applies the request during this preview's own prepaint,
                // so it has to be made while the tree is built — a prepaint or
                // next-frame hook lands a frame late. It is never
                // unconditional: it takes content this preview has not shown
                // yet plus a user who has not scrolled away from the tail.
                if *follow {
                    scroll.scroll_to_bottom();
                }
                div()
                    .id((root_id.clone(), "live-preview"))
                    .debug_selector(|| format!("thinking-live-preview-{trace_id}"))
                    .max_h(tokens.spacing.xxl * 4.0)
                    .overflow_y_scroll()
                    .track_scroll(scroll)
                    .child(body)
                    .into_any_element()
            }
            None => body.into_any_element(),
        };

        v_flex()
            .id(self.id.clone())
            .role(Role::Group)
            .aria_label(title)
            .when_some(failed, |this, reason| this.aria_description(reason))
            .gap(tokens.spacing.xs)
            .child(toggle)
            .when(showing, |this| {
                // Opening content fades in; closed content leaves the tree
                // immediately, including its selectable text and controls.
                this.child(
                    div()
                        .opacity(disclosure_opacity)
                        .top(tokens.spacing.xxs * (1.0 - disclosure) * crate::motion::travel(cx))
                        .child(body),
                )
            })
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        Context, Entity, Render, ScrollDelta, ScrollWheelEvent, TestAppContext, VisualTestContext,
        point, px,
    };

    /// Stable trace ID for the window tests. `debug_bounds` takes a literal,
    /// so the selectors below have to spell the ID out.
    const TRACE_ID: &str = "trace";
    const PREVIEW: &str = "thinking-live-preview-trace";
    const BODY: &str = "thinking-body-trace";
    /// Steps enough to overflow the preview's four-`xxl` cap several times.
    const STEPS: usize = 16;

    /// A trace mounted in a host taller than the live preview, so overflow is
    /// the preview's own bound rather than the window's.
    struct TraceProbe {
        state: ProgressState,
        steps: usize,
        open: bool,
    }

    impl TraceProbe {
        fn running() -> Self {
            Self {
                state: ProgressState::Running,
                steps: STEPS,
                open: true,
            }
        }

        /// Rebuilt per frame, exactly as applications rebuild theirs — every
        /// snapshot therefore reports revision zero.
        fn progress(&self) -> Progressive<ThinkingTrace> {
            let trace = ThinkingTrace::new()
                .steps((0..self.steps).map(|ix| ThinkingStep::new(format!("Reasoning step {ix}"))));
            match &self.state {
                ProgressState::Pending => Progressive::pending(trace),
                ProgressState::Running => Progressive::running(trace),
                ProgressState::Complete => Progressive::complete(trace),
                ProgressState::Failed(reason) => Progressive::failed(trace, reason.clone()),
            }
        }
    }

    impl Render for TraceProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(px(320.))
                .h(px(480.))
                .child(Thinking::new(TRACE_ID, &self.progress()).open(self.open))
        }
    }

    fn draw(cx: &mut VisualTestContext) {
        cx.update(|window, cx| window.draw(cx).clear(cx));
    }

    /// Where the trace body sits inside its live preview: zero at the top of
    /// the content, negative once the preview has scrolled down.
    fn scroll_offset(cx: &mut VisualTestContext) -> Pixels {
        let preview = cx.debug_bounds(PREVIEW).expect("the preview should render");
        let body = cx.debug_bounds(BODY).expect("the trace body should render");
        body.top() - preview.top()
    }

    /// How far the end of the content sits below the preview: zero while the
    /// preview is pinned to the tail.
    fn distance_past_tail(cx: &mut VisualTestContext) -> f32 {
        let preview = cx.debug_bounds(PREVIEW).expect("the preview should render");
        let body = cx.debug_bounds(BODY).expect("the trace body should render");
        (body.bottom() - preview.bottom()).as_f32()
    }

    fn assert_pinned_to_tail(cx: &mut VisualTestContext, expectation: &str) {
        let overflow = {
            let preview = cx.debug_bounds(PREVIEW).expect("the preview should render");
            let body = cx.debug_bounds(BODY).expect("the trace body should render");
            (body.size.height - preview.size.height).as_f32()
        };
        assert!(
            overflow > 0.0,
            "the trace must outgrow the preview for this to mean anything"
        );
        assert!(
            distance_past_tail(cx).abs() < 1.0,
            "{expectation} (content ends {} past the preview)",
            distance_past_tail(cx)
        );
    }

    fn append_step(probe: &Entity<TraceProbe>, cx: &mut VisualTestContext) {
        probe.update(cx, |probe, cx| {
            probe.steps += 1;
            cx.notify();
        });
        draw(cx);
    }

    fn set_open(probe: &Entity<TraceProbe>, cx: &mut VisualTestContext, open: bool) {
        probe.update(cx, |probe, cx| {
            probe.open = open;
            cx.notify();
        });
        draw(cx);
    }

    /// Advances past the disclosure cross-fade and draws the settled frame.
    fn settle_disclosure(cx: &mut VisualTestContext) {
        cx.executor()
            .advance_clock(MotionTokens::DEFAULT.standard() * 2);
        draw(cx);
        draw(cx);
    }

    /// Wheels the preview back toward earlier reasoning; positive GPUI offsets
    /// move toward the top of the content.
    fn scroll_up(cx: &mut VisualTestContext, distance: f32) {
        let preview = cx.debug_bounds(PREVIEW).expect("the preview should render");
        cx.simulate_event(ScrollWheelEvent {
            position: preview.center(),
            delta: ScrollDelta::Pixels(point(px(0.), px(distance))),
            ..Default::default()
        });
        draw(cx);
    }

    #[gpui::test]
    fn streamed_reasoning_stays_pinned_to_the_tail_while_following(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        assert_pinned_to_tail(cx, "a live preview opens on the newest reasoning");
        append_step(&probe, cx);
        assert_pinned_to_tail(
            cx,
            "streamed reasoning keeps a followed preview at the tail",
        );
    }

    #[gpui::test]
    fn streamed_reasoning_after_a_scroll_up_keeps_the_users_position(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        let tail = scroll_offset(cx);

        scroll_up(cx, 80.);
        let inspecting = scroll_offset(cx);
        assert!(
            inspecting > tail,
            "the wheel should have moved the preview off the tail"
        );

        append_step(&probe, cx);
        assert_eq!(
            scroll_offset(cx),
            inspecting,
            "streamed reasoning must not steal the position the user chose"
        );
        append_step(&probe, cx);
        assert_eq!(
            scroll_offset(cx),
            inspecting,
            "and it must not creep back over repeated updates"
        );
    }

    #[gpui::test]
    fn collapsing_and_reopening_returns_to_the_newest_reasoning(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        scroll_up(cx, 80.);
        assert!(distance_past_tail(cx) > 1.0, "the preview is off the tail");

        set_open(&probe, cx, false);
        assert!(
            cx.debug_bounds(PREVIEW).is_none(),
            "a collapsed trace immediately removes its live preview"
        );

        // Reasoning keeps streaming behind the collapsed disclosure.
        probe.update(cx, |probe, cx| {
            probe.steps += 1;
            cx.notify();
        });
        set_open(&probe, cx, true);
        assert_pinned_to_tail(cx, "reopening a live trace lands on the newest reasoning");
    }

    #[gpui::test]
    fn ten_rapid_toggles_keep_the_body_consistent_with_expansion(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        // Faster than the header transition can finish: closed content must
        // still leave the semantic and interaction tree on every toggle.
        for toggle in 0..10 {
            probe.update(cx, |probe, cx| {
                probe.open = !probe.open;
                cx.notify();
            });
            cx.executor().advance_clock(Duration::from_millis(70));
            draw(cx);
            assert_eq!(
                cx.debug_bounds(BODY).is_some(),
                toggle % 2 == 1,
                "toggle {toggle} must match the expanded state"
            );
        }

        // Ten flips from open land back on open; close deliberately, and
        // once the channel settles the body is gone for real.
        set_open(&probe, cx, false);
        settle_disclosure(cx);
        assert!(
            cx.debug_bounds(BODY).is_none(),
            "a settled closed disclosure unmounts its body"
        );
    }

    #[gpui::test]
    fn streamed_reasoning_does_not_replay_the_open_transition(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        settle_disclosure(cx);
        // The preview box, not the body: the body scrolls inside it as the
        // tail is followed, while the box only moves if the disclosure lift
        // re-runs — which is exactly what must not happen.
        let settled_top = cx
            .debug_bounds(PREVIEW)
            .expect("the settled preview should render")
            .top();

        append_step(&probe, cx);
        draw(cx);
        assert_eq!(
            cx.debug_bounds(PREVIEW)
                .expect("the streaming preview should render")
                .top(),
            settled_top,
            "appended reasoning must not re-run the disclosure lift"
        );
    }

    #[gpui::test]
    fn fresh_steps_animate_and_history_does_not(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        settle_disclosure(cx);
        crate::motion::take_reveal_frame_requests();

        // History shown at mount stays at rest: no reveal demand.
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "steps the preview opened with must not animate arrival"
        );

        // A step streamed while following the tail settles in.
        append_step(&probe, cx);
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "a freshly streamed step must acknowledge its arrival"
        );

        // Let it finish, then stream one in behind a scrolled-away reader:
        // choreography is bounded to the live tail.
        settle_disclosure(cx);
        scroll_up(cx, 80.);
        crate::motion::take_reveal_frame_requests();
        append_step(&probe, cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "reasoning that streams in behind the reader appears at rest"
        );
    }

    #[gpui::test]
    fn an_identified_step_keeps_its_acknowledgment_across_insertion(cx: &mut TestAppContext) {
        struct IdentityProbe {
            leading: usize,
        }

        impl Render for IdentityProbe {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                let steps = (0..self.leading)
                    .map(|ix| ThinkingStep::new(format!("Inserted {ix}")))
                    .chain([ThinkingStep::new("Anchored")
                        .id("anchored")
                        .status(StepStatus::Done)]);
                // Settled, so the arrival machinery is out of the picture and
                // the only reveal in play is the completion acknowledgment
                // whose keying is under test.
                let trace = Progressive::complete(ThinkingTrace::new().steps(steps));
                div()
                    .w(px(320.))
                    .h(px(480.))
                    .child(Thinking::new(TRACE_ID, &trace).open(true))
            }
        }

        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| IdentityProbe { leading: 0 });
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        settle_disclosure(cx);
        // Let the completion reveal play out entirely.
        cx.executor().advance_clock(Duration::from_secs(2));
        draw(cx);
        crate::motion::take_reveal_frame_requests();

        // Insert a step above: the identified step's index shifts, but its
        // declared identity keys the acknowledgment, so nothing replays.
        // (The positional fallback would shift keys here and replay the
        // reveal on the wrong row — the failure the ID exists to prevent.)
        probe.update(cx, |probe, cx| {
            probe.leading = 1;
            cx.notify();
        });
        draw(cx);
        draw(cx);
        assert_eq!(
            crate::motion::take_reveal_frame_requests(),
            0,
            "an identified step must not replay completion when its index shifts"
        );
    }

    #[gpui::test]
    fn reduced_motion_opens_and_closes_without_travel(cx: &mut TestAppContext) {
        cx.update(|cx| {
            crate::init(cx);
            cx.set_reduce_motion(true);
        });
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        assert!(
            cx.debug_bounds(BODY).is_some(),
            "reduced motion still opens the body — immediately"
        );

        set_open(&probe, cx, false);
        assert!(
            cx.debug_bounds(BODY).is_none(),
            "reduced motion closes without a fade to wait out"
        );
    }

    #[gpui::test]
    fn a_settled_trace_stops_adjusting_the_scroll_position(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);
        scroll_up(cx, 80.);

        probe.update(cx, |probe, cx| {
            probe.state = ProgressState::Failed("reasoning interrupted".into());
            cx.notify();
        });
        draw(cx);
        assert!(
            cx.debug_bounds(PREVIEW).is_none(),
            "a settled trace renders the full trace, not a bounded preview"
        );
        let settled = cx.debug_bounds(BODY).expect("the trace body should render");

        append_step(&probe, cx);
        let after = cx.debug_bounds(BODY).expect("the trace body should render");
        assert_eq!(
            after.top(),
            settled.top(),
            "a settled trace never moves its content"
        );
        assert!(
            after.size.height > settled.size.height,
            "the settled trace grows in place instead of scrolling"
        );
    }

    #[gpui::test]
    fn reduced_motion_still_lands_on_the_tail(cx: &mut TestAppContext) {
        cx.update(crate::init);
        cx.update(|cx| cx.set_reduce_motion(true));
        let (probe, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        assert_pinned_to_tail(cx, "a reduced-motion preview opens at its resting frame");
        append_step(&probe, cx);
        assert_pinned_to_tail(cx, "reduced motion settles on the tail without animating");
    }

    #[gpui::test]
    fn a_constrained_live_preview_keeps_every_step_reachable(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (_, cx) = cx.add_window_view(|_, _| TraceProbe::running());
        let cx: &mut VisualTestContext = cx;
        draw(cx);

        assert_pinned_to_tail(cx, "the end of the trace is on screen while following");
        scroll_up(cx, 4000.);
        assert!(
            scroll_offset(cx).as_f32().abs() < 1.0,
            "scrolling back must reach the first step"
        );
    }

    #[test]
    fn a_preview_with_nothing_to_scroll_follows_the_tail() {
        assert!(follows_tail(Pixels::ZERO, Pixels::ZERO, px(20.)));
    }

    #[test]
    fn following_tolerates_stopping_just_short_of_the_bottom() {
        assert!(follows_tail(px(-200.), px(200.), px(20.)), "at the tail");
        assert!(
            follows_tail(px(-190.), px(200.), px(20.)),
            "ten short of it"
        );
        assert!(
            !follows_tail(px(-160.), px(200.), px(20.)),
            "forty short of it is deliberate"
        );
        assert!(
            !follows_tail(Pixels::ZERO, px(200.), px(20.)),
            "the top of the trace is not the tail"
        );
    }

    #[test]
    fn a_live_preview_follows_only_content_it_has_not_shown() {
        let slack = px(20.);
        let mut preview = LivePreview::new();
        assert!(
            preview.observe(7, slack),
            "the first content a preview shows lands on the tail"
        );
        assert!(
            !preview.observe(7, slack),
            "a re-render of the same content never scrolls"
        );
        assert!(preview.observe(8, slack), "appended reasoning follows");
    }

    #[test]
    fn arrival_delays_belong_to_fresh_followed_identities_only() {
        let tokens = MotionTokens::DEFAULT;
        let now = crate::motion::Instant::now();
        let key = |name: &str| ElementId::Name(format!("step-{name}").into());
        let keys = |names: &[&str]| names.iter().map(|name| key(name)).collect::<Vec<_>>();
        let mut preview = LivePreview::new();

        // The first roll call is history: seen, at rest.
        preview.note_steps(keys(&["a", "b"]).into_iter(), true, &tokens, now);
        assert_eq!(preview.arrival_delay(&key("a")), None);
        assert_eq!(preview.arrival_delay(&key("b")), None);

        // One appended identity is a batch of one — acknowledged, no cascade.
        preview.note_steps(keys(&["a", "b", "c"]).into_iter(), true, &tokens, now);
        assert_eq!(preview.arrival_delay(&key("c")), Some(Duration::ZERO));

        // A three-identity batch cascades, decelerating, and the assignment
        // is frozen: a later roll call must not re-space it.
        preview.note_steps(
            keys(&["a", "b", "c", "d", "e", "f"]).into_iter(),
            true,
            &tokens,
            now,
        );
        let batch: Vec<_> = ["d", "e", "f"]
            .iter()
            .map(|name| preview.arrival_delay(&key(name)))
            .collect();
        assert_eq!(batch[0], Some(Duration::ZERO));
        assert!(batch[1] < batch[2], "the cascade is ordered");
        preview.note_steps(
            keys(&["a", "b", "c", "d", "e", "f", "g"]).into_iter(),
            true,
            &tokens,
            now,
        );
        assert_eq!(
            ["d", "e", "f"]
                .iter()
                .map(|name| preview.arrival_delay(&key(name)))
                .collect::<Vec<_>>(),
            batch,
            "an earlier batch keeps the delays it was assigned"
        );

        // An identity that arrives while the reader is away from the tail
        // appears at rest when they return.
        preview.follow = false;
        preview.note_steps(
            keys(&["a", "b", "c", "d", "e", "f", "g", "h"]).into_iter(),
            true,
            &tokens,
            now,
        );
        assert_eq!(preview.arrival_delay(&key("h")), None);

        // So does one that lands under the body's own fade-in, which owns
        // the acknowledgment.
        preview.follow = true;
        preview.note_steps(
            keys(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]).into_iter(),
            false,
            &tokens,
            now,
        );
        assert_eq!(preview.arrival_delay(&key("i")), None);
    }

    #[test]
    fn the_content_digest_sees_growth_that_the_snapshot_revision_cannot() {
        let trace = ThinkingTrace::new().steps([ThinkingStep::new("Reading the schema")]);
        let appended = ThinkingTrace::new().steps([
            ThinkingStep::new("Reading the schema"),
            ThinkingStep::new("Comparing unit prices"),
        ]);
        let finished = ThinkingTrace::new()
            .steps([ThinkingStep::new("Reading the schema").status(StepStatus::Done)]);

        assert_eq!(
            content_revision(0, &trace),
            content_revision(0, &trace.clone())
        );
        assert_ne!(content_revision(0, &trace), content_revision(0, &appended));
        assert_ne!(content_revision(0, &trace), content_revision(0, &finished));
        assert_ne!(
            content_revision(0, &trace),
            content_revision(0, &trace.clone().prose("Considering the schema"))
        );
    }

    #[test]
    fn trace_maps_the_shared_lifecycle() {
        let trace = ThinkingTrace::new().prose("Considering the schema");
        for (progress, state) in [
            (Progressive::pending(trace.clone()), ProgressState::Pending),
            (Progressive::running(trace.clone()), ProgressState::Running),
            (
                Progressive::complete(trace.clone()),
                ProgressState::Complete,
            ),
            (
                Progressive::failed(trace.clone(), "reasoning interrupted"),
                ProgressState::Failed("reasoning interrupted".into()),
            ),
        ] {
            let thinking = Thinking::new("trace", &progress);
            assert_eq!(thinking.state, state);
            assert_eq!(
                thinking.trace.prose.as_deref(),
                Some("Considering the schema")
            );
        }
    }

    #[test]
    fn automatic_policy_opens_while_running_and_collapses_when_settled() {
        let trace = ThinkingTrace::new();
        assert!(Thinking::new("t", &Progressive::running(trace.clone())).is_open());
        assert!(!Thinking::new("t", &Progressive::complete(trace.clone())).is_open());
        assert!(!Thinking::new("t", &Progressive::pending(trace.clone())).is_open());
        assert!(!Thinking::new("t", &Progressive::failed(trace.clone(), "stopped")).is_open());
    }

    #[test]
    fn explicit_open_overrides_the_automatic_policy() {
        let trace = ThinkingTrace::new();
        assert!(
            !Thinking::new("t", &Progressive::running(trace.clone()))
                .open(false)
                .is_open()
        );
        assert!(
            Thinking::new("t", &Progressive::complete(trace))
                .open(true)
                .is_open()
        );
    }
}
