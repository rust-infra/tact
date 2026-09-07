//! Subagent sticky-overview panel component: `SubagentsChanged` snapshots.
//!
//! Owns a [`SubagentPanelState`]; on `SubagentsChanged` it applies the
//! snapshot (visibility/expand logic lives on `apply_snapshot`). Self-contained
//! — the app layer handles the shared sticky host layout / mouse routing.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx, protocol::AgentUpdate, state::SubagentPanelState};

pub struct SubagentPanelComponent {
    state: SubagentPanelState,
}

impl SubagentPanelComponent {
    pub fn new() -> Self {
        Self {
            state: SubagentPanelState::default(),
        }
    }

    pub fn state(&self) -> &SubagentPanelState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut SubagentPanelState {
        &mut self.state
    }
}

impl Default for SubagentPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

/// Transparent field access: hosts keep `app.<field>…` working after the field
/// type becomes the component (no mechanical churn at call sites).
impl std::ops::Deref for SubagentPanelComponent {
    type Target = SubagentPanelState;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for SubagentPanelComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for SubagentPanelComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        if let AgentUpdate::SubagentsChanged { runs } = update {
            self.state.apply_snapshot(runs.clone());
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
        40
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
    use crate::components::tool::ToolEvent;
    use crate::{
        InputMode, PendingQueue,
        protocol::SubagentRunSnapshot,
        state::{LogCoordinator, StreamEvent},
    };

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        events: &'a mut Vec<StreamEvent>,
        tool_events: &'a mut Vec<ToolEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events: events,
            tool_events,
        }
    }

    fn run(status: tact_protocol::SubagentStatusSnapshot) -> SubagentRunSnapshot {
        SubagentRunSnapshot {
            child_id: "child-1".into(),
            status,
            summary_first: "summary".into(),
            started_at: Some(1),
            finished_at: None,
        }
    }

    #[test]
    fn subagents_changed_applies_snapshot_and_shows_panel() {
        let mut comp = SubagentPanelComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let dirty = comp.on_update(
            &AgentUpdate::SubagentsChanged {
                runs: vec![run(tact_protocol::SubagentStatusSnapshot::Running)],
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert!(dirty);
        assert!(comp.state().session_seen);
        assert!(comp.state().visible);
        assert!(comp.state().expanded);
        assert_eq!(comp.state().snapshot.len(), 1);
    }

    #[test]
    fn unrelated_updates_are_ignored() {
        let mut comp = SubagentPanelComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let dirty = comp.on_update(
            &AgentUpdate::TaskComplete("done".into()),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert!(!dirty);
        assert!(!comp.state().session_seen);
    }
}
