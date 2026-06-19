use super::PlanStep;
use ratatui::widgets::{ListState, ScrollbarState};

/// Execution Plan panel state.
pub(crate) struct PlanPanel {
    pub(crate) steps: Vec<PlanStep>,
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
            collapsed: Vec::new(),
            selected: 0,
            list_state: ListState::default(),
            scroll_state: ScrollbarState::new(0),
            visible: false,
        }
    }
}
