//! Durable Goal sticky-overview component.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx, protocol::AgentUpdate, state::GoalPanelState};

pub struct GoalPanelComponent {
    state: GoalPanelState,
}

impl GoalPanelComponent {
    pub fn new() -> Self {
        Self {
            state: GoalPanelState::default(),
        }
    }

    pub fn state(&self) -> &GoalPanelState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut GoalPanelState {
        &mut self.state
    }
}

impl Default for GoalPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl std::ops::Deref for GoalPanelComponent {
    type Target = GoalPanelState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for GoalPanelComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for GoalPanelComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        if let AgentUpdate::GoalsChanged { goals } = update {
            self.state.apply_snapshot(goals.clone());
            true
        } else {
            false
        }
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn render(&self, _area: Rect, _buf: &mut Buffer, _ctx: &Ctx<'_>) -> u16 {
        0
    }

    fn priority(&self) -> u8 {
        39
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
    use crate::{
        InputMode, PendingQueue,
        components::tool::ToolEvent,
        protocol::{AgentUpdate, GoalSnapshot, GoalStatusSnapshot},
        state::{LogCoordinator, StreamEvent},
    };

    #[test]
    fn goals_changed_applies_full_snapshot_and_opens_panel() {
        let mut component = GoalPanelComponent::new();
        let mut log = LogCoordinator::default();
        let mut pending = PendingQueue::default();
        let mut stream_events: Vec<StreamEvent> = Vec::new();
        let mut tool_events: Vec<ToolEvent> = Vec::new();
        let mut ctx = Ctx {
            log: &mut log,
            input_mode: InputMode::Normal,
            pending: &mut pending,
            stream_events: &mut stream_events,
            tool_events: &mut tool_events,
        };

        let dirty = component.on_update(
            &AgentUpdate::GoalsChanged {
                goals: vec![GoalSnapshot {
                    id: "goal-1".into(),
                    title: "Ship".into(),
                    status: GoalStatusSnapshot::Running,
                    ..Default::default()
                }],
            },
            &mut ctx,
        );

        assert!(dirty);
        assert!(component.visible);
        assert!(component.expanded);
        assert_eq!(component.selected_goal_id(), Some("goal-1"));
    }
}
