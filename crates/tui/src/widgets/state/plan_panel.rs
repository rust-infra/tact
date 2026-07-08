use std::collections::BTreeMap;

use super::PlanStep;
use ratatui::widgets::{ListState, ScrollbarState};

/// Execution Plan panel state.
pub(crate) struct PlanPanel {
    pub(crate) steps: Vec<PlanStep>,
    pub(crate) steps_set: BTreeMap<String, PlanStep>,
    pub(crate) collapsed: Vec<bool>,
    pub(crate) selected: usize,
    pub(crate) list_state: ListState,
    pub(crate) scroll_state: ScrollbarState,
    pub(crate) visible: bool,
}

impl PlanPanel {
    pub(crate) fn new() -> Self {
        Self {
            steps: Vec::new(),
            steps_set: BTreeMap::new(),
            collapsed: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            scroll_state: ScrollbarState::new(0),
            visible: false,
        }
    }
    pub(crate) fn reset(&mut self) {
        self.steps_set.clear();
        self.steps.clear();
        self.collapsed.clear();
        self.selected = 0;
        self.list_state = ListState::default();
        self.scroll_state = ScrollbarState::new(0);
    }
}
