use ratatui::layout::Rect;

/// Source byte range represented by one popup screen cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupTextHit {
    pub start: usize,
    pub end: usize,
}

impl PopupTextHit {
    pub fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub fn empty(offset: usize) -> Self {
        Self::new(offset, offset)
    }
}

/// Hit-test data for one visible row in the tool popup body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PopupHitRow {
    pub screen_y: u16,
    pub text_x: u16,
    pub line_start: usize,
    pub line_end: usize,
    pub cells: Vec<PopupTextHit>,
}

impl PopupHitRow {
    pub fn hit(&self, screen_x: u16) -> PopupTextHit {
        if screen_x < self.text_x {
            return PopupTextHit::empty(self.line_start);
        }
        self.cells
            .get(usize::from(screen_x - self.text_x))
            .copied()
            .unwrap_or_else(|| PopupTextHit::empty(self.line_end))
    }
}

/// A position within a specific physical log message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextPosition {
    pub phys_idx: usize,
    pub byte_offset: usize,
}

impl TextPosition {
    pub fn new(phys_idx: usize, byte_offset: usize) -> Self {
        Self {
            phys_idx,
            byte_offset,
        }
    }
}

/// A character-level selection in the Log panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LogSelection {
    pub start: TextPosition,
    pub end: TextPosition,
}

impl LogSelection {
    pub fn new(start: TextPosition, end: TextPosition) -> Self {
        Self { start, end }
    }

    /// Select an entire physical message (`[0, len)`).
    pub fn full_message(phys_idx: usize, len: usize) -> Self {
        Self::new(
            TextPosition::new(phys_idx, 0),
            TextPosition::new(phys_idx, len),
        )
    }

    /// Select a byte span within a single physical message.
    pub fn span(phys_idx: usize, start: usize, end: usize) -> Self {
        Self::new(
            TextPosition::new(phys_idx, start),
            TextPosition::new(phys_idx, end),
        )
    }

    /// Normalize so that start <= end (by physical index, then byte offset).
    pub fn normalized(&self) -> (TextPosition, TextPosition) {
        if self.start.phys_idx < self.end.phys_idx
            || (self.start.phys_idx == self.end.phys_idx
                && self.start.byte_offset <= self.end.byte_offset)
        {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    /// Byte range of this selection within one physical message, if any.
    pub fn byte_range_for(&self, phys: usize, msg_len: usize) -> Option<(usize, usize)> {
        let (start, end) = self.normalized();
        if phys < start.phys_idx || phys > end.phys_idx {
            return None;
        }
        if start.phys_idx == end.phys_idx {
            Some((start.byte_offset, end.byte_offset))
        } else if phys == start.phys_idx {
            Some((start.byte_offset, msg_len))
        } else if phys == end.phys_idx {
            Some((0, end.byte_offset))
        } else {
            Some((0, msg_len))
        }
    }
}

/// Mouse interaction state: manages panel areas, selection ranges, and drag flags.
#[derive(Default)]
pub struct MouseState {
    pub log_area: Rect,
    /// Sticky task progress strip under the Log (empty when hidden).
    pub task_panel_area: Rect,
    /// Whether the cursor is hovering over the task panel (used for keyboard scrolling).
    pub in_task_panel: bool,
    pub log_selection: Option<LogSelection>,
    pub dragging_log: bool,
    /// thinking popup area (used to determine if click is inside the popup).
    pub thinking_popup_area: Rect,
    /// diff popup area (used to determine if click is inside the popup).
    pub diff_popup_area: Rect,
    /// subagent popup area (used to determine if click is inside the popup).
    pub subagent_popup_area: Rect,
    /// slash-command popup area (used to route mouse-wheel scrolls to the
    /// popup's selection list instead of the log behind it).
    pub slash_popup_area: Rect,
    /// selection popup area (used to route mouse-wheel scrolls to the popup's
    /// option list instead of the log behind it).
    pub select_popup_area: Rect,
    /// Selectable body area inside the active text popup border.
    pub popup_text_body_area: Rect,
    /// Hit maps for rows currently visible in the active text popup body.
    pub popup_text_hit_rows: Vec<PopupHitRow>,
    /// Source grapheme where the active text-popup drag began.
    pub popup_text_drag_origin: Option<PopupTextHit>,
    /// code block popup area (used to determine if click is inside the popup).
    pub code_popup_area: Rect,
    /// Mermaid source popup area.
    pub mermaid_popup_area: Rect,
    /// `/tasks-dag` popup area.
    pub task_dag_popup_area: Rect,
    /// Double/triple click detection: time and position of the last left click.
    pub last_click_time: Option<std::time::Instant>,
    pub last_click_pos: Option<(u16, u16)>,
    /// Consecutive click count (1=single, 2=double, 3=triple).
    pub click_count: u8,
    /// Index of the thinking block hit by the last click (used for double-click popup open).
    pub last_click_card: Option<usize>,
    /// Index of the diff block hit by the last click (used for double-click popup open).
    pub last_click_tool: Option<usize>,
    /// Index of the code block hit by the last click (used for double-click popup open).
    pub last_click_code: Option<usize>,
    /// Index of the Mermaid block hit by the last click (used for double-click popup open).
    pub last_click_mermaid: Option<usize>,
}

impl MouseState {
    pub fn new() -> Self {
        Self::default()
    }
}
