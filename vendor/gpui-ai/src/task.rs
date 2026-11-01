//! Live status rows for progressive agent tasks.

use crate::stream::{ProgressState, Progressive};
use crate::theme::SemanticStyledExt as _;
use gpui::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce, Role,
    SharedString, StatefulInteractiveElement as _, StyleRefinement, Styled, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{ActiveTheme as _, StyledExt as _, h_flex};
use std::time::Duration;

/// The typed content rendered by a [`TaskRow`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskSnapshot {
    id: SharedString,
    title: SharedString,
    detail: Option<SharedString>,
    elapsed: Option<Duration>,
}

impl TaskSnapshot {
    /// Creates a task snapshot with a stable application-level identifier.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            detail: None,
            elapsed: None,
        }
    }

    /// Adds a muted trailing detail such as a count or target.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Adds caller-owned elapsed time; the component never owns a clock.
    pub fn elapsed(mut self, elapsed: Duration) -> Self {
        self.elapsed = Some(elapsed);
        self
    }

    /// Returns the stable task identifier.
    pub fn id(&self) -> &str {
        self.id.as_ref()
    }
}

/// One row in a list of agent tasks, driven by the shared progressive model.
///
/// # Example
///
/// ```
/// # use gpui_ai::prelude::*;
/// # use std::time::Duration;
/// let task = Progressive::running(
///     TaskSnapshot::new("index", "Index repository")
///         .detail("3,214 files")
///         .elapsed(Duration::from_secs(12)),
/// );
/// TaskRow::new(&task);
/// ```
#[derive(IntoElement)]
pub struct TaskRow {
    style: StyleRefinement,
    task: TaskSnapshot,
    state: ProgressState,
}

impl TaskRow {
    /// Creates a row from a progressive task snapshot.
    pub fn new(task: &Progressive<TaskSnapshot>) -> Self {
        Self {
            style: StyleRefinement::default(),
            task: task.content().clone(),
            state: task.state().clone(),
        }
    }
}

impl Styled for TaskRow {
    fn style(&mut self) -> &mut StyleRefinement {
        &mut self.style
    }
}

impl RenderOnce for TaskRow {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = cx.theme().semantic_tokens();
        // The shared lifecycle glyph: durable marks settle in once through
        // its acknowledgment, never on a re-render, and never for the
        // state the row mounts with.
        let indicator = crate::status::progress_glyph(
            &self.state,
            ElementId::from((ElementId::from(self.task.id.clone()), "task-glyph")),
            crate::sizing::slot_md(cx),
            window,
            cx,
        )
        .into_any_element();
        let failed_reason = match &self.state {
            ProgressState::Failed(reason) => Some(reason.clone()),
            _ => None,
        };
        let state_label = match &self.state {
            ProgressState::Pending => "pending",
            ProgressState::Running => "in progress",
            ProgressState::Complete => "complete",
            ProgressState::Failed(_) => "failed",
        };
        let accessibility_label = format!("{}, {state_label}", self.task.title);
        let accessibility_description = failed_reason.clone().or_else(|| self.task.detail.clone());

        h_flex()
            .id(self.task.id.clone())
            .role(Role::ListItem)
            .aria_label(accessibility_label)
            .when_some(accessibility_description, |this, description| {
                this.aria_description(description)
            })
            .w_full()
            .items_center()
            .gap(tokens.spacing.sm)
            .py(tokens.spacing.xs)
            .text_token(tokens.typography.sm)
            // The lifecycle glyph arrives already seated in its slot, so a
            // state change never nudges the title sideways.
            .child(indicator)
            .child(
                div()
                    .flex_1()
                    .truncate()
                    .text_color(match self.state {
                        ProgressState::Pending => cx.theme().muted_foreground,
                        _ => cx.theme().foreground,
                    })
                    .child(self.task.title),
            )
            .when_some(self.task.elapsed, |this, elapsed| {
                this.child(
                    div()
                        .flex_none()
                        .text_token(tokens.typography.xs)
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_color(cx.theme().muted_foreground)
                        .child(format_elapsed(elapsed)),
                )
            })
            .when_some(failed_reason.or(self.task.detail), |this, detail| {
                // The trailing detail shrinks and truncates under pressure
                // instead of pushing the row wide; the title keeps its room.
                this.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_token(tokens.typography.xs)
                        .text_color(match self.state {
                            ProgressState::Failed(_) => cx.theme().danger,
                            _ => cx.theme().muted_foreground,
                        })
                        .child(detail),
                )
            })
            .refine_style(&self.style)
    }
}

fn format_elapsed(elapsed: Duration) -> String {
    let secs = elapsed.as_secs_f64();
    if secs < 10.0 {
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
    use gpui::{Context, Render, TestAppContext};

    struct LifecycleProbe {
        state: ProgressState,
    }

    impl Render for LifecycleProbe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            TaskRow {
                task: TaskSnapshot::new("task", "Index repository"),
                state: self.state.clone(),
                style: StyleRefinement::default(),
            }
        }
    }

    #[gpui::test]
    fn running_to_complete_acknowledges_the_controlled_change(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (probe, cx) = cx.add_window_view(|_, _| LifecycleProbe {
            state: ProgressState::Running,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        crate::motion::take_reveal_frame_requests();

        probe.update(cx, |probe, cx| {
            probe.state = ProgressState::Complete;
            cx.notify();
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            crate::motion::take_reveal_frame_requests() > 0,
            "completion is a state change, not the glyph's first mount"
        );
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::from_millis(400)), "0.4s");
        assert_eq!(format_elapsed(Duration::from_secs(12)), "12s");
        assert_eq!(format_elapsed(Duration::from_secs(125)), "2m 05s");
    }

    #[test]
    fn row_maps_the_shared_lifecycle() {
        let task = TaskSnapshot::new("index", "Index repository");
        for (progress, state) in [
            (Progressive::pending(task.clone()), ProgressState::Pending),
            (Progressive::running(task.clone()), ProgressState::Running),
            (Progressive::complete(task.clone()), ProgressState::Complete),
            (
                Progressive::failed(task.clone(), "disk unavailable"),
                ProgressState::Failed("disk unavailable".into()),
            ),
        ] {
            let row = TaskRow::new(&progress);
            assert_eq!(row.state, state);
            assert_eq!(row.task.id(), "index");
        }
    }
}
