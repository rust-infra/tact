//! Typed accessors into the [`App::registry`] component registry.
//!
//! Whole-App switch (plan step 9): the six kit components own the UI state
//! (`PlanPanel`, `ThinkingState`, `StreamState`, `ToolState`, `StatusBarState`,
//! `TaskPanelState`); the shell keeps its rich handlers but reaches the state
//! through these accessors instead of bare fields. Read contexts use
//! `field()`, write contexts `field_mut()`; both return the component, whose
//! `Deref` exposes the underlying state type directly.

use agent_tui_kit::components::{
    PlanComponent, StatusBarComponent, StreamComponent, TaskPanelComponent, ThinkingComponent,
    ToolComponent,
};

use crate::widgets::state::App;

impl App {
    pub(crate) fn plan(&self) -> &PlanComponent {
        self.registry
            .get::<PlanComponent>()
            .expect("plan component registered")
    }

    pub(crate) fn plan_mut(&mut self) -> &mut PlanComponent {
        self.registry
            .get_mut::<PlanComponent>()
            .expect("plan component registered")
    }

    pub(crate) fn thinking(&self) -> &ThinkingComponent {
        self.registry
            .get::<ThinkingComponent>()
            .expect("thinking component registered")
    }

    pub(crate) fn thinking_mut(&mut self) -> &mut ThinkingComponent {
        self.registry
            .get_mut::<ThinkingComponent>()
            .expect("thinking component registered")
    }

    pub(crate) fn stream(&self) -> &StreamComponent {
        self.registry
            .get::<StreamComponent>()
            .expect("stream component registered")
    }

    pub(crate) fn stream_mut(&mut self) -> &mut StreamComponent {
        self.registry
            .get_mut::<StreamComponent>()
            .expect("stream component registered")
    }

    pub(crate) fn tools(&self) -> &ToolComponent {
        self.registry
            .get::<ToolComponent>()
            .expect("tool component registered")
    }

    pub(crate) fn tools_mut(&mut self) -> &mut ToolComponent {
        self.registry
            .get_mut::<ToolComponent>()
            .expect("tool component registered")
    }

    pub(crate) fn status_bar(&self) -> &StatusBarComponent {
        self.registry
            .get::<StatusBarComponent>()
            .expect("status-bar component registered")
    }

    pub(crate) fn status_bar_mut(&mut self) -> &mut StatusBarComponent {
        self.registry
            .get_mut::<StatusBarComponent>()
            .expect("status-bar component registered")
    }

    pub(crate) fn task_panel(&self) -> &TaskPanelComponent {
        self.registry
            .get::<TaskPanelComponent>()
            .expect("task-panel component registered")
    }

    pub(crate) fn task_panel_mut(&mut self) -> &mut TaskPanelComponent {
        self.registry
            .get_mut::<TaskPanelComponent>()
            .expect("task-panel component registered")
    }
}
