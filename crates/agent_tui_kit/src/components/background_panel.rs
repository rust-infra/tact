//! Background sticky-overview panel component.
//!
//! Owns a [`BackgroundPanelState`] so the shell can reach it through the same
//! component registry as the Tasks / Subagent stickies. Unlike those two there
//! is **no `AgentUpdate` that carries this domain's rows** — they are derived
//! from the live `background_run` cards each tick
//! ([`crate::state::background_panel::running_background_tasks`], applied by
//! `App::sync_background_sticky`). So `on_update` never claims an update; this
//! type exists for state ownership, not for update handling.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx, protocol::AgentUpdate, state::BackgroundPanelState};

pub struct BackgroundPanelComponent {
    state: BackgroundPanelState,
}

impl BackgroundPanelComponent {
    pub fn new() -> Self {
        Self {
            state: BackgroundPanelState::new(),
        }
    }

    pub fn state(&self) -> &BackgroundPanelState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut BackgroundPanelState {
        &mut self.state
    }
}

impl Default for BackgroundPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

/// Transparent field access: hosts keep `app.<field>…` working after the field
/// type becomes the component (no mechanical churn at call sites).
impl std::ops::Deref for BackgroundPanelComponent {
    type Target = BackgroundPanelState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for BackgroundPanelComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for BackgroundPanelComponent {
    /// Never claims an update: nothing in the protocol pushes this domain.
    fn on_update(&mut self, _update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn render(&self, _area: Rect, _buf: &mut Buffer, _ctx: &Ctx<'_>) -> u16 {
        0
    }

    fn priority(&self) -> u8 {
        41
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_panel_starts_hidden_and_is_driven_by_apply_running() {
        let mut component = BackgroundPanelComponent::new();
        assert!(!component.visible && !component.expanded);

        component.state_mut().apply_running(1);
        assert!(component.visible && component.expanded);

        // Deref reaches the state for the read side.
        assert!(component.max_visible > 0);
    }

    #[test]
    fn no_update_claims_the_component() {
        let mut component = BackgroundPanelComponent::new();
        let (mut log, mut pending, mut events, mut tool_events) = (
            crate::state::LogCoordinator::default(),
            Default::default(),
            Vec::new(),
            Vec::new(),
        );
        let mut ctx = Ctx {
            log: &mut log,
            input_mode: crate::InputMode::Normal,
            pending: &mut pending,
            stream_events: &mut events,
            tool_events: &mut tool_events,
        };
        assert!(!component.on_update(
            &AgentUpdate::ToolMeta {
                tool_id: "bg1".into(),
                model: None,
                token_usage: None,
                task_id: Some("018f3a2c".into()),
            },
            &mut ctx,
        ));
    }
}
