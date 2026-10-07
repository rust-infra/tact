use ratatui::layout::Rect;

use crate::state::StickyTab;

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

/// A screen surface the mouse routes against.
///
/// One key per hit-testable area, replacing a bag of independent `*_area`
/// fields. Adding a surface used to cost four edits — a field, a renderer
/// write, a hit-ladder arm and a scroll arm — and forgetting one was silent:
/// the palette and the file picker scrolled the log behind them for as long as
/// they existed. It is now one variant, and the array [`MouseState::areas`]
/// makes the table exhaustive by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum SurfaceId {
    /// The Log panel (and the sticky strip's parent split).
    Log,
    /// The sticky task/subagent/background strip under the Log.
    TaskPanel,
    CodePopup,
    MermaidPopup,
    ThinkingPopup,
    SubagentPopup,
    DiffPopup,
    TaskDagPopup,
    SlashPopup,
    SelectPopup,
    PalettePopup,
    FilePickerPopup,
}

impl SurfaceId {
    /// Number of surfaces; also the length of [`MouseState::areas`].
    pub const COUNT: usize = 12;

    /// Every surface, in hit-test priority order: the most modal overlay first,
    /// so a popup drawn over the log wins the point test.
    ///
    /// Kept in sync with the enum by `surface_ids_are_exhaustive` — a variant
    /// missing here would be a surface no click could ever reach.
    pub const HIT_ORDER: &'static [SurfaceId] = &[
        SurfaceId::SelectPopup,
        SurfaceId::SlashPopup,
        SurfaceId::PalettePopup,
        SurfaceId::FilePickerPopup,
        SurfaceId::DiffPopup,
        SurfaceId::ThinkingPopup,
        SurfaceId::SubagentPopup,
        SurfaceId::CodePopup,
        SurfaceId::MermaidPopup,
        SurfaceId::TaskDagPopup,
        SurfaceId::TaskPanel,
        SurfaceId::Log,
    ];

    /// The list surfaces a wheel scroll moves a cursor on, in hit order.
    ///
    /// A subset of [`SurfaceId::HIT_ORDER`]: a new list popup joins both tables
    /// by adding its variant (plus `scroll_surface`'s exhaustive match), not by
    /// remembering this one too.
    pub const LIST_POPUPS: &'static [SurfaceId] = &[
        SurfaceId::SelectPopup,
        SurfaceId::SlashPopup,
        SurfaceId::PalettePopup,
        SurfaceId::FilePickerPopup,
    ];
}

/// Mouse interaction state: manages panel areas, selection ranges, and drag flags.
#[derive(Default)]
pub struct MouseState {
    /// Hit rectangles of every routable surface, indexed by [`SurfaceId`].
    ///
    /// A plain array, not a map: `SurfaceId` is a closed enum, so lookup is an
    /// index and a whole-table reset is a `fill`.
    areas: [Rect; SurfaceId::COUNT],
    /// Whether the cursor is hovering over the task panel (used for keyboard scrolling).
    pub in_task_panel: bool,
    /// Which sticky domain (Tasks / Subagent) is currently active when the
    /// shared host strip is visible. Drives which body renders when expanded
    /// and which panel's `scroll` the wheel / jk keys move.
    pub active_sticky_tab: StickyTab,
    /// Hit rectangles for each visible sticky tab label, refreshed every
    /// frame by the host renderer (mirrors `subagent_cancel_btn_areas`).
    pub sticky_tab_areas: Vec<(StickyTab, Rect)>,
    pub log_selection: Option<LogSelection>,
    pub dragging_log: bool,
    /// Cancel buttons for live async-subagent tool cards: `(child_id, rect)`.
    /// Refreshed every frame by the log renderer; a click sends
    /// `UserCommand::CancelSubagent { child_id }`.
    pub subagent_cancel_btn_areas: Vec<(String, Rect)>,
    /// Footer `[󰜼 Open]` button rects of the Thinking cards on screen, refreshed
    /// every frame by the log renderer. A Thinking card draws text of its own
    /// on rows that do not map to that text, so its button is the only glyph a
    /// click may open the popup from.
    pub thinking_open_btn_areas: Vec<Rect>,
    /// Selectable body area inside the active text popup border.
    ///
    /// Deliberately *not* a [`SurfaceId`]: thinking / diff / subagent share one
    /// text-selection surface because only one of them is ever open, so a
    /// single slot is the honest shape.
    pub popup_text_body_area: Rect,
    /// Hit maps for rows currently visible in the active text popup body.
    pub popup_text_hit_rows: Vec<PopupHitRow>,
    /// Source grapheme where the active text-popup drag began.
    pub popup_text_drag_origin: Option<PopupTextHit>,
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

    /// The rect recorded for `id` this frame (`Rect::default()` when the
    /// surface is hidden — a zero-size rect no point can fall inside).
    pub fn area(&self, id: SurfaceId) -> Rect {
        self.areas[id as usize]
    }

    /// Record `id`'s rect for this frame.
    pub fn set_area(&mut self, id: SurfaceId, rect: Rect) {
        self.areas[id as usize] = rect;
    }

    /// Hide `id` for this frame.
    ///
    /// The renderers own this contract: each one records its rect while it is
    /// drawn and clears it when it is not, so a surface's area is meaningful
    /// even when a renderer is exercised on its own (the render tests do
    /// exactly that). There is deliberately no frame-start wipe of the whole
    /// table — a second mechanism would let the two disagree.
    pub fn clear_area(&mut self, id: SurfaceId) {
        self.areas[id as usize] = Rect::default();
    }

    /// Whether `(column, row)` falls inside `id`'s rect.
    ///
    /// A hidden surface has a zero-size rect, so this is also the "is it
    /// active" test every hit ladder needs — no separate `input_mode` check.
    pub fn hits(&self, id: SurfaceId, column: u16, row: u16) -> bool {
        let area = self.area(id);
        column >= area.x
            && column < area.x + area.width
            && row >= area.y
            && row < area.y + area.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The enum's discriminants must stay a dense `0..COUNT` range: the
    /// registry indexes by `id as usize`, so a gap or an off-by-one would
    /// silently alias two surfaces onto one slot.
    #[test]
    fn surface_ids_are_dense_and_counted() {
        let mut seen = [false; SurfaceId::COUNT];
        for id in SurfaceId::HIT_ORDER {
            let idx = *id as usize;
            assert!(idx < SurfaceId::COUNT, "{id:?} is outside the table");
            assert!(!seen[idx], "{id:?} is listed twice");
            seen[idx] = true;
        }
        assert!(
            seen.iter().all(|s| *s),
            "HIT_ORDER must list every surface or a click can never reach it"
        );
    }

    /// The list-popup table drives the wheel; an entry outside `HIT_ORDER`
    /// would be a surface that never wins the point test anyway.
    #[test]
    fn list_popups_are_a_subset_of_the_hit_order() {
        for id in SurfaceId::LIST_POPUPS {
            assert!(SurfaceId::HIT_ORDER.contains(id), "{id:?} is not hittable");
        }
    }

    /// A surface nobody recorded this frame must not swallow the pointer —
    /// this is what lets the hit ladders drop their `input_mode` checks.
    #[test]
    fn a_cleared_surface_never_hits() {
        let mut mouse = MouseState::new();
        mouse.set_area(SurfaceId::PalettePopup, Rect::new(10, 10, 20, 8));
        assert!(mouse.hits(SurfaceId::PalettePopup, 15, 12));

        mouse.clear_area(SurfaceId::PalettePopup);
        assert!(!mouse.hits(SurfaceId::PalettePopup, 15, 12));
        assert!(
            !mouse.hits(SurfaceId::PalettePopup, 15, 12),
            "a cleared surface is invisible to the hit test"
        );
    }

    /// The right/bottom edges are exclusive: a rect at x=10 w=20 covers 10..30,
    /// so column 30 belongs to the surface behind it.
    #[test]
    fn hit_testing_uses_half_open_bounds() {
        let mut mouse = MouseState::new();
        mouse.set_area(SurfaceId::Log, Rect::new(10, 5, 20, 4));
        assert!(mouse.hits(SurfaceId::Log, 10, 5), "top-left is inside");
        assert!(mouse.hits(SurfaceId::Log, 29, 8), "last cell is inside");
        assert!(!mouse.hits(SurfaceId::Log, 30, 8), "right edge is outside");
        assert!(!mouse.hits(SurfaceId::Log, 29, 9), "bottom edge is outside");
        assert!(!mouse.hits(SurfaceId::Log, 9, 5), "left edge is outside");
    }
}
