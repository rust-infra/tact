use ratatui::text::Line;
use ratatui::widgets::ScrollbarState;

/// Log panel scroll state: manages scroll offset, scrollbar, panel height, and visual line mapping.
pub(crate) struct LogScroll {
    /// Current scroll offset.
    pub(crate) offset: u16,
    /// Scrollbar state.
    pub(crate) state: ScrollbarState,
    /// Panel height.
    pub(crate) height: u16,
    /// Visual line starting index list.
    pub(crate) visual_start: Vec<usize>,
    /// Cached visual lines (wrap_line results, excluding search/selection styles).
    pub(crate) visual_cache: Vec<Line<'static>>,
    /// Cached logical→visual mapping (visual_cache start indices).
    pub(crate) visual_start_cache: Vec<usize>,
    /// Cached visual line width.
    pub(crate) visual_cache_width: u16,
    /// messages.len() when cache was last built; invalidated on change.
    pub(crate) visual_cache_ver: usize,
    /// messages.len() when visible_indices was last built.
    pub(crate) visible_indices_ver: usize,
    /// physical → logical reverse mapping cache (uses Option for invisible lines).
    pub(crate) phys_to_logical_cache: Vec<Option<usize>>,
}

impl LogScroll {
    pub(crate) fn new() -> Self {
        Self {
            offset: 0,
            state: ScrollbarState::new(0),
            height: 10,
            visual_start: Vec::new(),
            visual_cache: Vec::new(),
            visual_start_cache: Vec::new(),
            visual_cache_width: 0,
            visual_cache_ver: 0,
            visible_indices_ver: 0,
            phys_to_logical_cache: Vec::new(),
        }
    }
}
