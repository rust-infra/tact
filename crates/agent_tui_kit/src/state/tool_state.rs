use std::time::Instant;

use crate::protocol::ToolOutputBuffer;
use ratatui::text::Line;

use crate::render::selectable_text::PopupLayoutCache;
use crate::widgets::tool_widget::ToolRenderOutput;

use super::PopupTextSelection;

/// Tool state: active invocations, completed blocks, and diff popup preview.
#[derive(Default)]
pub struct ToolState {
    /// Currently running tool blocks (live elapsed time in meta row).
    pub active: Vec<ActiveToolBlock>,
    /// Completed tool blocks rendered as title + meta + optional detail cards.
    pub blocks: Vec<ToolBlock>,
    /// Popup preview state for file write/read content.
    pub popup: Option<DiffPopup>,
}

/// A tool invocation that has started but not yet finished.
#[derive(Debug, Clone)]
pub struct ActiveToolBlock {
    pub phys_idx: usize,
    pub tool_id: String,
    pub output: ToolRenderOutput,
    pub live_output: ToolOutputBuffer,
    pub started_at: Instant,
}

/// A completed tool invocation's range in messages and its pre-built render output.
#[derive(Debug, Clone)]
pub struct ToolBlock {
    /// Physical index of the first placeholder row in `App::log_items`.
    pub phys_idx: usize,
    pub tool_id: String,
    pub output: ToolRenderOutput,
}

impl DiffPopup {
    pub fn copy_content(&self) -> Option<String> {
        self.cached_content
            .as_deref()
            .or(self.inline_content.as_deref())
            .map(|content| self.copy_content_from(content))
    }

    pub fn copy_content_from(&self, content: &str) -> String {
        self.selection
            .and_then(|selection| selection.normalized_non_empty(content))
            .map(|range| content[range].to_string())
            .unwrap_or_else(|| content.to_string())
    }
}

/// Popup preview state for tool detail (file content or command output).
#[derive(Debug, Clone)]
pub struct DiffPopup {
    pub title: String,
    /// Read content from disk when set.
    pub file_path: Option<String>,
    /// Run `git diff -- <path>` when set (lazy-loaded into cached_content).
    pub git_diff_path: Option<String>,
    /// Working directory in which to run `git diff`.
    pub workspace_dir: Option<String>,
    /// Use in-memory content directly (command output, fallback for files).
    pub inline_content: Option<String>,
    pub lang: String,
    pub use_diff_gutter: bool,
    /// Content is a unified diff (git diff output); render -/+ lines natively.
    pub is_diff: bool,
    pub scroll: u16,
    pub selection: Option<PopupTextSelection>,
    pub cached_content: Option<String>,
    pub highlighted_lines: Vec<Line<'static>>,
}

/// Popup preview state for subagent live output / markdown summary.
#[derive(Debug, Clone)]
pub struct SubagentPopup {
    pub title: String,
    pub scroll: u16,
    /// Tool id of the spawn_subagent invocation this popup belongs to.
    pub tool_id: String,
    /// Cached rendered markdown lines (populated once on first render after completion).
    pub cached_markdown: Option<Vec<Line<'static>>>,
    /// Current text selection (mouse-drag), if any.
    pub selection: Option<PopupTextSelection>,
    /// Wrapped-layout cache reused while content length and width are stable.
    pub layout_cache: Option<PopupLayoutCache>,
}

#[cfg(test)]
mod tests {
    use super::PopupTextSelection;

    #[test]
    fn popup_selection_normalizes_forward_and_backward_utf8_ranges() {
        let text = "a界z";
        let forward = PopupTextSelection::new(1, text.len());
        let backward = PopupTextSelection::new(text.len(), 1);

        assert_eq!(forward.normalized_non_empty(text), Some(1..5));
        assert_eq!(backward.normalized_non_empty(text), Some(1..5));
    }

    #[test]
    fn popup_selection_ignores_empty_range() {
        assert_eq!(
            PopupTextSelection::new(2, 2).normalized_non_empty("text"),
            None
        );
    }

    #[test]
    fn popup_selection_clamps_offsets_to_content_length() {
        assert_eq!(
            PopupTextSelection::new(0, usize::MAX).normalized_non_empty("text"),
            Some(0..4)
        );
    }

    #[test]
    fn popup_selection_floors_multibyte_offsets_to_character_boundaries() {
        assert_eq!(
            PopupTextSelection::new(4, 2).normalized_non_empty("a界z"),
            Some(1..4)
        );
    }
}
