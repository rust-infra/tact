use ratatui::layout::Rect;

/// Mouse interaction state: manages panel areas, selection ranges, and drag flags.
#[derive(Default)]
pub(crate) struct MouseState {
    pub(crate) plan_area: Rect,
    pub(crate) log_area: Rect,
    pub(crate) plan_selection: Option<(usize, usize)>,
    pub(crate) dragging_plan: bool,
    pub(crate) log_selection: Option<(usize, usize)>,
    pub(crate) dragging_log: bool,
    /// thinking popup area (used to determine if click is inside the popup).
    pub(crate) thinking_popup_area: Rect,
    /// diff popup area (used to determine if click is inside the popup).
    pub(crate) diff_popup_area: Rect,
    /// code block popup area (used to determine if click is inside the popup).
    pub(crate) code_popup_area: Rect,
    /// Double/triple click detection: time and position of the last left click.
    pub(crate) last_click_time: Option<std::time::Instant>,
    pub(crate) last_click_pos: Option<(u16, u16)>,
    /// Consecutive click count (1=single, 2=double, 3=triple).
    pub(crate) click_count: u8,
    /// Double-click word selection: records (word_start_byte, word_end_byte), used alongside log_selection's line.
    pub(crate) log_word_selection: Option<(usize, usize)>,
    /// Index of the thinking block hit by the last click (used for double-click popup open).
    pub(crate) last_click_card: Option<usize>,
    /// Index of the diff block hit by the last click (used for double-click popup open).
    pub(crate) last_click_diff: Option<usize>,
    /// Index of the code block hit by the last click (used for double-click popup open).
    pub(crate) last_click_code: Option<usize>,
}

impl MouseState {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}
