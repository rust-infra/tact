//! An animated pixel-grid loading state with optional elapsed time.

use crate::motion::{MotionTokens, VisibleAnimationExt as _};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, StyledExt as _, h_flex};
use std::panic::Location;
use std::time::Duration;

/// Grid dimensions of the loader.
const ROWS: usize = 3;
const COLS: usize = 3;

/// A pixel-grid loader for "the agent is working" moments.
///
/// A small grid of squares pulses in a diagonal sweep, next to a label and an
/// optional elapsed-time readout. The component holds no timer: the caller
/// owns the clock and re-renders to tick the elapsed display (the sweep
/// itself animates independently via GPUI's animation system).
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// # use std::time::Duration;
/// LoadingState::new()
///     .label("Reasoning about your request")
///     .elapsed(Duration::from_secs(7));
/// ```
#[derive(IntoElement)]
pub struct LoadingState {
    id: ElementId,
    style: StyleRefinement,
    label: SharedString,
    elapsed: Option<Duration>,
}

impl LoadingState {
    /// Creates a loader with the default label.
    #[track_caller]
    pub fn new() -> Self {
        Self {
            id: ElementId::CodeLocation(*Location::caller()),
            style: StyleRefinement::default(),
            label: "Working…".into(),
            elapsed: None,
        }
    }

    /// Sets the label describing what is happening.
    pub fn label(mut self, label: impl Into<SharedString>) -> Self {
        self.label = label.into();
        self
    }

    /// Shows an elapsed-time readout after the label.
    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }
}

impl Default for LoadingState {
    #[track_caller]
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for LoadingState {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for LoadingState {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        let color = cx.theme().primary;
        let sweep = MotionTokens::read(cx).grid_sweep();
        let cell_gap = tokens.spacing.xxs;
        let cell_size = tokens.spacing.xs;
        let full = crate::motion::motion_is_full(cx);
        let cell_radius = tokens.spacing.xxs;

        // One clock for the whole grid: the root animation samples a single
        // phase and every cell derives its pulse from it, so nine squares
        // cost one scheduled animation, not nine. Phase-locked to the shared
        // epoch, so loaders mounted at different moments sweep together.
        let grid = div()
            .flex()
            .flex_col()
            .gap(cell_gap)
            .with_visible_animation(
                "loading-grid",
                // Frame demand: active while the caller keeps the loader mounted
                // — the loader *is* the "work is running" state and owns no
                // clock, so it settles by being unmounted. Reduced motion holds
                // delta at 0, leaving a static diagonal gradient across the grid.
                sweep.looping_synced(),
                move |grid, delta| {
                    let delta = if full { delta } else { 0.0 };
                    grid.children((0..ROWS).map(|row| {
                        h_flex().gap(cell_gap).children((0..COLS).map(move |col| {
                            // Each cell's pulse is phase-shifted along the
                            // diagonal, producing a sweep from the top-left
                            // corner.
                            let phase = (row + col) as f32 / ((ROWS + COLS) as f32);
                            let wave = ((delta - phase).rem_euclid(1.0) * 2.0 - 1.0).abs();
                            div()
                                .size(cell_size)
                                .rounded(cell_radius)
                                .bg(color)
                                .opacity(0.15 + 0.85 * wave)
                        }))
                    }))
                },
            );

        h_flex()
            .id(self.id)
            .role(Role::ProgressIndicator)
            .aria_label(self.label.clone())
            .items_center()
            .gap(tokens.spacing.md)
            .child(grid)
            .child(
                div()
                    .text_token(tokens.typography.sm)
                    .text_color(cx.theme().muted_foreground)
                    .child(self.label),
            )
            .when_some(self.elapsed, |this, elapsed| {
                this.child(
                    div()
                        .text_token(tokens.typography.xs)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("{:.0}s", elapsed.as_secs_f64())),
                )
            })
            .refine_style(&self.style)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Context, Render, TestAppContext, WindowHandle, px, size};

    struct LoaderProbe {
        elapsed: Option<Duration>,
    }

    impl Render for LoaderProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let mut loader = LoadingState::new().label("Working");
            if let Some(elapsed) = self.elapsed {
                loader = loader.elapsed(elapsed);
            }
            div().child(loader)
        }
    }

    fn open(elapsed: Option<Duration>, cx: &mut TestAppContext) -> WindowHandle<LoaderProbe> {
        cx.update(crate::init);
        let window = cx.open_window(size(px(240.), px(80.)), |_, _| LoaderProbe { elapsed });
        cx.run_until_parked();
        // Resolve the initial visibility observation before measuring the
        // steady region clock (the visibility transition is a one-off).
        next_frame(&window, cx);
        window
    }

    fn next_frame(window: &WindowHandle<LoaderProbe>, cx: &mut TestAppContext) -> usize {
        let callbacks = window
            .update(cx, |_, window, cx| window.simulate_next_frame(cx))
            .expect("the loader window should remain open");
        cx.run_until_parked();
        callbacks
    }

    #[gpui::test]
    fn the_whole_grid_ticks_on_one_clock(cx: &mut TestAppContext) {
        // The count is the architecture: nine cells deriving their pulse from
        // one sampled phase schedule one animation, not nine.
        let window = open(None, cx);
        assert_eq!(next_frame(&window, cx), 1, "one region, one clock");
    }

    #[gpui::test]
    fn an_elapsed_update_does_not_disturb_the_clock(cx: &mut TestAppContext) {
        let window = open(Some(Duration::from_secs(3)), cx);
        assert_eq!(next_frame(&window, cx), 1);

        // Mid-cycle, the caller ticks the readout — the reason this component
        // re-renders in practice, once a second for as long as it is shown.
        cx.executor().advance_clock(Duration::from_millis(700));
        window
            .update(cx, |probe, _, cx| {
                probe.elapsed = Some(Duration::from_secs(4));
                cx.notify();
            })
            .expect("the loader window should remain open");
        cx.run_until_parked();

        // The update re-renders while the previous frame's callback is still
        // queued, so the frame right after it can transiently report both.
        // Drain it; the steady state is the claim.
        next_frame(&window, cx);

        // Still exactly one scheduled animation: the update neither tore the
        // clock down (0) nor left a second one running beside it (2).
        assert_eq!(next_frame(&window, cx), 1);
    }

    #[gpui::test]
    fn reduced_motion_holds_the_choreographed_still_frame(cx: &mut TestAppContext) {
        cx.update(|cx| cx.set_reduce_motion(true));
        let window = open(None, cx);
        assert_eq!(
            next_frame(&window, cx),
            0,
            "a held sweep schedules nothing; the static diagonal gradient is the frame"
        );
    }
}
