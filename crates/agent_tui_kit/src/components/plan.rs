//! Execution-plan component: `StepAdded` bookkeeping.
//!
//! Owns a [`PlanPanel`]; on `StepAdded` it records the step. Status
//! transitions (`Executing`) are shell concerns (the coordinator pre-pass),
//! so this component stays self-contained.

use crossterm::event::KeyEvent;
use ratatui::{buffer::Buffer, layout::Rect};

use crate::{Component, Ctx, protocol::AgentUpdate, state::PlanPanel};

pub struct PlanComponent {
    state: PlanPanel,
}

impl PlanComponent {
    pub fn new() -> Self {
        Self {
            state: PlanPanel::default(),
        }
    }

    pub fn state(&self) -> &PlanPanel {
        &self.state
    }

    pub fn state_mut(&mut self) -> &mut PlanPanel {
        &mut self.state
    }
}

impl Default for PlanComponent {
    fn default() -> Self {
        Self::new()
    }
}

impl Component for PlanComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        match update {
            AgentUpdate::StepAdded(step) => {
                self.state.steps.push(step.clone());
                self.state
                    .steps_set
                    .insert(step.tool_id.clone(), step.clone());
                true
            }
            // Keep the plan's step outputs in sync on completion/failure so the
            // status bar's progress derivation stays correct.
            AgentUpdate::StepFinished { idx, result, .. } => {
                if let Some(step) = self.state.steps.get_mut(*idx) {
                    step.output = Some(result.message.clone());
                }
                true
            }
            AgentUpdate::StepFailed { idx, error, .. } => {
                if let Some(step) = self.state.steps.get_mut(*idx) {
                    step.output = Some(error.clone());
                }
                true
            }
            _ => false,
        }
    }

    fn on_key(&mut self, _key: KeyEvent, _ctx: &mut Ctx<'_>) -> bool {
        false
    }

    fn render(&self, _area: Rect, _buf: &mut Buffer, _ctx: &Ctx<'_>) -> u16 {
        0
    }

    fn priority(&self) -> u8 {
        5
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
        protocol::{PlanStep, StepResult, StepStatus},
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

    fn step(tool_id: &str) -> PlanStep {
        PlanStep::new(
            "read",
            "read_file",
            tool_id,
            std::collections::HashMap::<String, String>::new(),
        )
    }

    #[test]
    fn step_added_records_step() {
        let mut comp = PlanComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        comp.on_update(
            &AgentUpdate::StepAdded(step("t1")),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert_eq!(comp.state().steps.len(), 1);
        assert!(comp.state().steps_set.contains_key("t1"));
    }

    #[test]
    fn step_finished_updates_step_output() {
        let mut comp = PlanComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        comp.on_update(
            &AgentUpdate::StepAdded(step("t1")),
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        comp.on_update(
            &AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "t1".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "r".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "done".into(),
                    detail: None,
                    duration_us: Some(1),
                    permission_label: None,
                    presentation: crate::protocol::ToolPresentationInfo::generic("read_file"),
                },
            },
            &mut ctx(&mut log, &mut pending, &mut events),
        );
        assert_eq!(comp.state().steps[0].output.as_deref(), Some("done"));
    }
}
