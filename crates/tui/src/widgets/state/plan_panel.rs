use std::collections::BTreeMap;

use super::PlanStep;

/// Internal execution step store (no dedicated UI panel — see `book/23_chapter_tui.md`).
#[derive(Default)]
pub(crate) struct PlanPanel {
    pub(crate) steps: Vec<PlanStep>,
    pub(crate) steps_set: BTreeMap<String, PlanStep>,
}

impl PlanPanel {
    pub(crate) fn reset(&mut self) {
        self.steps_set.clear();
        self.steps.clear();
    }
}
