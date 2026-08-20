use ratatui::{text::Line, widgets::ScrollbarState};

use crate::theme::ThemeName;

/// Log panel scroll state: manages scroll offset, scrollbar, panel height,
/// visible-index caches, and visual line mapping.
pub struct LogScroll {
    /// Authoritative scroll position: the first visible *visual* line.
    ///
    /// Unlike the logical `offset`, this can point anywhere inside a cell
    /// taller than the viewport (e.g. a long Markdown table), so the middle
    /// of such cells stays reachable. `usize::MAX` is the pre-render
    /// "pin to bottom" sentinel; render clamps it to `total - height`.
    pub visual_top: usize,
    /// Derived logical offset mirror (row containing `visual_top`), kept in
    /// sync by render and scroll handlers for read-only consumers such as
    /// mouse hit-testing, the code-card popup, and tests.
    pub offset: u16,
    /// Scrollbar state.
    pub state: ScrollbarState,
    /// Panel height.
    pub height: u16,
    /// Last-known panel content width (set on render; used at table-build time
    /// so streamed tables are laid out to fit before the panel wraps them).
    pub width: u16,
    /// Visual line starting index list.
    pub visual_start: Vec<usize>,
    /// Cached visual lines (wrap_line results, excluding selection styles).
    pub visual_cache: Vec<Line<'static>>,
    /// Cached logical→visual mapping (visual_cache start indices).
    pub visual_start_cache: Vec<usize>,
    /// Cached visual line width.
    pub visual_cache_width: u16,
    /// `log_items.len()` when cache was last built; invalidated on change.
    pub visual_cache_ver: usize,
    /// Theme active when cache was last built.
    pub visual_cache_theme: ThemeName,
    /// `log_items.len()` when visible_indices was last built.
    pub visible_indices_ver: usize,
    /// Visible index cache: logical line → physical msg index.
    pub visible_indices: Vec<usize>,
    /// physical → logical reverse mapping cache (uses Option for invisible lines).
    pub phys_to_logical_cache: Vec<Option<usize>>,
}

impl LogScroll {
    pub fn new() -> Self {
        Self {
            visual_top: 0,
            offset: 0,
            state: ScrollbarState::new(0),
            height: 10,
            width: 80,
            visible_indices: Vec::new(),
            visual_start: Vec::new(),
            visual_cache: Vec::new(),
            visual_start_cache: Vec::new(),
            visual_cache_width: 0,
            visual_cache_ver: 0,
            visual_cache_theme: ThemeName::Retro,
            visible_indices_ver: 0,
            phys_to_logical_cache: Vec::new(),
        }
    }
}

impl Default for LogScroll {
    fn default() -> Self {
        Self::new()
    }
}
