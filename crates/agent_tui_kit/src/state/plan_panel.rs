use std::collections::BTreeMap;

use crate::protocol::PlanStep;

/// Internal execution step store (no dedicated UI panel — see `book/23_chapter_tui.md`).
#[derive(Default)]
pub struct PlanPanel {
    pub steps: Vec<PlanStep>,
    pub steps_set: BTreeMap<String, PlanStep>,
}

impl PlanPanel {
    pub fn reset(&mut self) {
        self.steps_set.clear();
        self.steps.clear();
    }
}
