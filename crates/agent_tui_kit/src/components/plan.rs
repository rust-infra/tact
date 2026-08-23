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

    /// Resolve a step index the way the shell does: prefer the first plan
    /// position for this `tool_id` (recorded from `StepAdded` in arrival
    /// order — restarts keep the original position), fall back to `idx`.
    fn resolve_step_idx(&self, tool_id: &str, idx: usize) -> usize {
        self.state
            .steps
            .iter()
            .position(|s| s.tool_id == tool_id)
            .unwrap_or(idx)
    }
}

impl Default for PlanComponent {
    fn default() -> Self {
        Self::new()
    }
}

/// Transparent field access: hosts keep `app.<field>…` working after the field
/// type becomes the component (no mechanical churn at call sites).
impl std::ops::Deref for PlanComponent {
    type Target = PlanPanel;
    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl std::ops::DerefMut for PlanComponent {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl Component for PlanComponent {
    fn on_update(&mut self, update: &AgentUpdate, _ctx: &mut Ctx<'_>) -> bool {
        match update {
            // These branches write side state (the plan snapshot) but do not
            // *consume* the update: `ToolComponent` also handles `StepAdded` /
            // `StepFinished` / `StepFailed` (its card lifecycle), and the
            // registry dispatches to every component until one claims.
            // Returning `false` lets the dispatch continue to the tool
            // component; the plan write still happens.
            AgentUpdate::StepAdded(step) => {
                self.state.steps.push(step.clone());
                self.state
                    .steps_set
                    .insert(step.tool_id.clone(), step.clone());
                false
            }
            // Keep the plan's step outputs in sync on completion/failure so the
            // status bar's progress derivation stays correct. The agent's raw
            // `idx` is not a reliable plan index (out-of-order/restarted tool
            // ids), so resolve to the first plan position like the shell does.
            AgentUpdate::StepFinished {
                idx,
                tool_id,
                result,
            } => {
                let idx = self.resolve_step_idx(tool_id, *idx);
                if let Some(step) = self.state.steps.get_mut(idx) {
                    step.output = Some(result.message.clone());
                }
                false
            }
            AgentUpdate::StepFailed {
                idx,
                tool_id,
                error,
                ..
            } => {
                let idx = self.resolve_step_idx(tool_id, *idx);
                if let Some(step) = self.state.steps.get_mut(idx) {
                    step.output = Some(error.clone());
                }
                false
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
    use crate::components::tool::ToolEvent;
    use crate::{
        InputMode, PendingQueue,
        protocol::{PlanStep, StepResult, StepStatus},
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
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
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
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
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
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert_eq!(comp.state().steps[0].output.as_deref(), Some("done"));
    }

    #[test]
    fn step_finished_resolves_divergent_raw_idx() {
        // The agent's raw `idx` is not a reliable plan index: when it differs
        // from the tool_id's plan position, the output must land on the
        // RESOLVED step (first plan position), never on the raw-index slot.
        let mut comp = PlanComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        comp.on_update(
            &AgentUpdate::StepAdded(step("t1")),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        comp.on_update(
            &AgentUpdate::StepAdded(step("t2")),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        // Raw idx 0 would hit steps[0] (t1); resolved must be steps[1] (t2).
        comp.on_update(
            &AgentUpdate::StepFinished {
                idx: 0,
                tool_id: "t2".into(),
                result: StepResult {
                    tool: "read_file".into(),
                    arg_summary: "r".into(),
                    arg_full: None,
                    status: StepStatus::Success,
                    message: "done2".into(),
                    detail: None,
                    duration_us: Some(1),
                    permission_label: None,
                    presentation: crate::protocol::ToolPresentationInfo::generic("read_file"),
                },
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert_eq!(
            comp.state().steps[0].output.as_deref(),
            None,
            "t1 untouched"
        );
        assert_eq!(comp.state().steps[1].output.as_deref(), Some("done2"));
    }

    #[test]
    fn step_failed_resolves_divergent_raw_idx() {
        let mut comp = PlanComponent::new();
        let (mut log, mut pending, mut events) = (
            LogCoordinator::default(),
            PendingQueue::default(),
            Vec::new(),
        );
        comp.on_update(
            &AgentUpdate::StepAdded(step("t1")),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        comp.on_update(
            &AgentUpdate::StepAdded(step("t2")),
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        comp.on_update(
            &AgentUpdate::StepFailed {
                idx: 0,
                tool_id: "t2".into(),
                arg_summary: String::new(),
                error: "boom2".into(),
            },
            &mut ctx(&mut log, &mut pending, &mut events, &mut Vec::new()),
        );
        assert_eq!(
            comp.state().steps[0].output.as_deref(),
            None,
            "t1 untouched"
        );
        assert_eq!(comp.state().steps[1].output.as_deref(), Some("boom2"));
    }
}
