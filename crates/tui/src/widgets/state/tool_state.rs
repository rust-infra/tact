use std::time::Instant;

use crate::widgets::tool_widget::ToolRenderOutput;
use ratatui::text::Line;

/// Tool state: active invocations, completed blocks, and diff popup preview.
pub(crate) struct ToolState {
    /// Currently running tool blocks (live elapsed time in meta row).
    pub(crate) active: Vec<ActiveToolBlock>,
    /// Completed tool blocks rendered as title + meta + optional detail cards.
    pub(crate) blocks: Vec<ToolBlock>,
    /// Popup preview state for file write/read content.
    pub(crate) popup: Option<DiffPopup>,
}

/// A tool invocation that has started but not yet finished.
#[derive(Debug, Clone)]
pub(crate) struct ActiveToolBlock {
    pub phys_idx: usize,
    pub tool_id: String,
    pub output: ToolRenderOutput,
    pub started_at: Instant,
}

/// A completed tool invocation's range in messages and its pre-built render output.
#[derive(Debug, Clone)]
pub(crate) struct ToolBlock {
    /// Physical index of the first placeholder row in `messages` / `raw_messages`.
    pub phys_idx: usize,
    pub output: ToolRenderOutput,
}

/// Popup preview state for tool detail (file content or command output).
#[derive(Debug, Clone)]
pub(crate) struct DiffPopup {
    pub title: String,
    /// Read content from disk when set.
    pub file_path: Option<String>,
    /// Use in-memory content directly (command output, fallback for files).
    pub inline_content: Option<String>,
    pub lang: String,
    pub use_diff_gutter: bool,
    pub scroll: u16,
    pub cached_content: Option<String>,
    pub highlighted_lines: Vec<Line<'static>>,
}

impl ToolState {
    pub(crate) fn new() -> Self {
        Self {
            active: Vec::new(),
            blocks: Vec::new(),
            popup: None,
        }
    }
}
