use ratatui::layout::Rect;

/// Source byte range represented by one popup screen cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PopupTextHit {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl PopupTextHit {
    pub(crate) fn new(start: usize, end: usize) -> Self {
        Self { start, end }
    }

    pub(crate) fn empty(offset: usize) -> Self {
        Self::new(offset, offset)
    }
}

/// Hit-test data for one visible row in the tool popup body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PopupHitRow {
    pub(crate) screen_y: u16,
    pub(crate) text_x: u16,
    pub(crate) line_start: usize,
    pub(crate) line_end: usize,
    pub(crate) cells: Vec<PopupTextHit>,
}

impl PopupHitRow {
    pub(crate) fn hit(&self, screen_x: u16) -> PopupTextHit {
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
pub(crate) struct TextPosition {
    pub phys_idx: usize,
    pub byte_offset: usize,
}

impl TextPosition {
    pub(crate) fn new(phys_idx: usize, byte_offset: usize) -> Self {
        Self {
            phys_idx,
            byte_offset,
        }
    }
}

/// A character-level selection in the Log panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LogSelection {
    pub start: TextPosition,
    pub end: TextPosition,
}

impl LogSelection {
    pub(crate) fn new(start: TextPosition, end: TextPosition) -> Self {
        Self { start, end }
    }

    /// Select an entire physical message (`[0, len)`).
    pub(crate) fn full_message(phys_idx: usize, len: usize) -> Self {
        Self::new(
            TextPosition::new(phys_idx, 0),
            TextPosition::new(phys_idx, len),
        )
    }

    /// Select a byte span within a single physical message.
    pub(crate) fn span(phys_idx: usize, start: usize, end: usize) -> Self {
        Self::new(
            TextPosition::new(phys_idx, start),
            TextPosition::new(phys_idx, end),
        )
    }

    /// Normalize so that start <= end (by physical index, then byte offset).
    pub(crate) fn normalized(&self) -> (TextPosition, TextPosition) {
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
    pub(crate) fn byte_range_for(&self, phys: usize, msg_len: usize) -> Option<(usize, usize)> {
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
pub(crate) struct MouseState {
    pub(crate) plan_area: Rect,
    pub(crate) log_area: Rect,
    pub(crate) plan_selection: Option<(usize, usize)>,
    pub(crate) dragging_plan: bool,
    pub(crate) log_selection: Option<LogSelection>,
    pub(crate) dragging_log: bool,
    /// thinking popup area (used to determine if click is inside the popup).
    pub(crate) thinking_popup_area: Rect,
    /// diff popup area (used to determine if click is inside the popup).
    pub(crate) diff_popup_area: Rect,
    /// Selectable body area inside the active text popup border.
    pub(crate) popup_text_body_area: Rect,
    /// Hit maps for rows currently visible in the active text popup body.
    pub(crate) popup_text_hit_rows: Vec<PopupHitRow>,
    /// Source grapheme where the active text-popup drag began.
    pub(crate) popup_text_drag_origin: Option<PopupTextHit>,
    /// code block popup area (used to determine if click is inside the popup).
    pub(crate) code_popup_area: Rect,
    /// Double/triple click detection: time and position of the last left click.
    pub(crate) last_click_time: Option<std::time::Instant>,
    pub(crate) last_click_pos: Option<(u16, u16)>,
    /// Consecutive click count (1=single, 2=double, 3=triple).
    pub(crate) click_count: u8,
    /// Index of the thinking block hit by the last click (used for double-click popup open).
    pub(crate) last_click_card: Option<usize>,
    /// Index of the diff block hit by the last click (used for double-click popup open).
    pub(crate) last_click_tool: Option<usize>,
    /// Index of the code block hit by the last click (used for double-click popup open).
    pub(crate) last_click_code: Option<usize>,
    /// Divider area between Plan and Log panels (for drag-to-resize).
    pub(crate) divider_area: Rect,
    /// Whether the user is currently dragging the panel divider.
    pub(crate) is_resizing_panel: bool,
}

impl MouseState {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}
