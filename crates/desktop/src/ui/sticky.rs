use tact_desktop_core::{GateCard, InspectorView, TaskView};

pub fn active_task(inspector: &InspectorView) -> Option<&TaskView> {
    inspector
        .tasks
        .iter()
        .find(|task| task.status.marker() != "[x]")
}

pub fn pending_gate(inspector_pending: &[GateCard]) -> Option<&GateCard> {
    inspector_pending.first()
}
