//! Persistent-task sticky-panel component: `TasksChanged` snapshots.
//!
//! Owns a [`TaskPanelState`]; on `TasksChanged` it applies the snapshot (the
//! visibility/expand logic lives on `apply_snapshot`). Self-contained — the
//! app layer handles the task-DAG popup sync separately.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx, protocol::AgentUpdate, state::TaskPanelState};

pub struct TaskPanelComponent {
    state: TaskPanelState,
}

impl TaskPanelComponent {
    pub fn new() -> Self {
        Self {
            state: TaskPanelState::default(),
        }
    }

    pub fn state(&self) -> &TaskPanelState {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut TaskPanelState {
        &mut self.state
    }
}

impl Default for TaskPanelComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for TaskPanelComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        if let AgentUpdate::TasksChanged { tasks, .. } = update {
            self.state.apply_snapshot(tasks.clone());
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
    use crate::{
        InputMode, PendingQueue,
        protocol::{TaskSnapshot, TaskStatusSnapshot, TasksChangeReason},
        state::{LogCoordinator, StreamEvent},
    };

    fn ctx<'a>(
        log: &'a mut LogCoordinator,
        pending: &'a mut PendingQueue,
        events: &'a mut Vec<StreamEvent>,
    ) -> Ctx<'a> {
        Ctx {
            log,
            input_mode: InputMode::Normal,
            pending,
            stream_events: events,
        }
    }

    fn pending_task(id: u64) -> TaskSnapshot {
        TaskSnapshot {
            id,
            subject: format!("task {id}"),
            status: TaskStatusSnapshot::InProgress,
            ..Default::default()
        }
    }

    #[test]
    fn tasks_changed_applies_snapshot_and_shows_panel() {
        let mut comp = TaskPanelComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let dirty = comp.on_update(
            &AgentUpdate::TasksChanged {
                tasks: vec![pending_task(1)],
                reason: TasksChangeReason::Created,
            },
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(dirty);
        assert!(comp.state().visible);
        assert!(comp.state().expanded);
        assert_eq!(comp.state().snapshot.len(), 1);
    }

    #[test]
    fn unrelated_updates_are_ignored() {
        let mut comp = TaskPanelComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        let dirty = comp.on_update(
            &AgentUpdate::TaskComplete("done".into()),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert!(!dirty);
    }
}
