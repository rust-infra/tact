use std::time::{Duration, Instant};

/// Thinking state: manages reasoning content buffer, title markers, active/completed blocks, and popups.
pub(crate) struct ThinkingState {
    /// Reasoning content buffer.
    pub(crate) buffer: String,
    /// Whether the title has been added.
    pub(crate) title_added: bool,
    /// Active block start position.
    pub(crate) active_start: Option<usize>,
    /// Active block end position.
    pub(crate) active_end: Option<usize>,
    /// Reasoning block list.
    pub(crate) blocks: Vec<ThinkingBlock>,
    /// Popup state.
    pub(crate) popup: Option<ThinkingPopup>,
    /// When the current thinking block started streaming.
    pub(crate) thinking_start: Option<Instant>,
}

/// A completed Thinking block's range in messages and its scroll state.
/// After completion, only the last 3 lines are shown by default; scroll_offset controls the visible window start row.
// messages[] 布局（一个已完成 thinking block 的物理索引范围）:

// [blank_idx]   ""                              ← 隔离空行
// [title_idx]   "🧠 Thinking (8 lines)…"        ← title_idx
// [title_idx+1] "│ Let me analyze…"              ← 思考内容行 1
//   ...
// [end_idx]     "│ Solution: use B…"             ← end_idx ← 最后一行
// [end_idx+1]   ""                              ← 隔离空行（close 时插入）
#[derive(Debug, Clone)]
pub(crate) struct ThinkingBlock {
    pub title_idx: usize,
    pub end_idx: usize,
    /// Current visible window start row offset (relative to title_idx+1), auto-scrolls to bottom by default.
    pub scroll_offset: usize,
    /// Cached plain text lines ("│ " prefix stripped), used for card preview and copy.
    pub(crate) cached_preview: Vec<String>,
    /// Cached Markdown rendered lines, used for popup display, avoiding per-frame re-rendering.
    pub(crate) cached_markdown: Vec<ratatui::text::Line<'static>>,
    /// Duration of the thinking phase.
    pub(crate) elapsed: Duration,
}

/// Thinking popup state.
#[derive(Debug, Clone)]
pub(crate) struct ThinkingPopup {
    pub block_idx: usize,
    pub title: String,
    /// Popup internal scroll offset (line number, relative to the first thinking content line).
    pub scroll: u16,
}

impl ThinkingState {
    pub(crate) fn new() -> Self {
        Self {
            buffer: String::new(),
            title_added: false,
            active_start: None,
            active_end: None,
            thinking_start: None,
            blocks: Vec::new(),
            popup: None,
        }
    }
}
