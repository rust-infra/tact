use std::time::Instant;

use crate::protocol::{ChunkKind, ToolOutputBuffer};
use ratatui::text::Line;

use crate::widgets::tool_widget::ToolRenderOutput;

use super::{PopupHitRow, PopupTextSelection};

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
    /// Current text selection (mouse-drag), if any.
    pub selection: Option<PopupTextSelection>,
    /// Whether the popup view is bottom-following (live tail). Toggled with `f`.
    pub follow_bottom: bool,
    /// Stream still growing (`true`) vs ended (`false` — end of `StepFinished`/
    /// `StepFailed`). Only affects cursor pulse, never closes the popup.
    pub live: bool,
    /// Block indexes collapsed to a 3-line thinking card (or tool title row).
    pub collapsed_blocks: std::collections::HashSet<usize>,
    /// Block-model layout cache (incremental append).
    pub layout_cache: Option<SubagentLayoutCache>,
    /// Cached hit table, keyed by a (content, width, scroll) stamp so it is
    /// only rebuilt when the view actually moved.
    pub hit_cache: Option<(u64, Vec<PopupHitRow>)>,
}

/// Role of a rendered row inside a block (plain / title / footer).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowRole {
    Plain,
    Title,
    Footer,
}

/// One committed source line of the transcript (spans folded to plain text).
#[derive(Debug, Clone)]
pub struct SubagentSourceLine {
    pub text: String,
    pub kind: Option<ChunkKind>,
    /// Byte offset of this line in `SubagentLayoutCache::raw_text`.
    pub source_start: usize,
}

/// One flat, renderable row of the popup body (chrome rows included).
///
/// `line_idx` indexes `SubagentLayoutCache::lines` for content rows; `None`
/// marks synthetic block chrome (title / footer) or the in-progress tail.
/// `source_start`/`source_end` map the row into `raw_text` so mouse selection
/// stays byte-accurate. `block_id` is the stable block identity — the starting
/// `lines` index of the run this row belongs to (tail rows use `lines.len()`).
#[derive(Debug, Clone)]
pub struct SubagentRow {
    pub text: String,
    pub kind: Option<ChunkKind>,
    pub role: RowRole,
    /// Index into `lines` for content rows; `None` for block chrome / tail rows.
    pub line_idx: Option<usize>,
    pub source_start: usize,
    pub source_end: usize,
    /// Stable block id (= starting `lines` index of the block's kind run).
    pub block_id: usize,
}

/// Block-model layout cache for the subagent popup.
///
/// - `lines` grows append-only (incremental layout): new committed source lines
///   from the watermark are folded to text and pushed; the in-progress tail is
///   kept separately.
/// - `rows` is the flat visible list (block chrome interleaved with content).
///   It is rebuilt in full only when a committed line is appended (low
///   frequency — once per newline) or on a collapse toggle; the per-frame hot
///   path (tail growth) only replaces the final tail row in O(1).
/// - `raw_text` is the byte-accurate concatenation of committed lines (for
///   copy & selection).
#[derive(Debug, Clone)]
pub struct SubagentLayoutCache {
    pub width: u16,
    /// Chrome labels snapshot (all `&'static str`, cheap to copy per frame).
    pub labels: SubagentLabels,
    /// Watermark: number of committed source lines already folded into `lines`.
    pub laid_out_committed: usize,
    /// Raw transcript text (for copy / selection), append-only.
    pub raw_text: String,
    /// Committed source lines in transcript order.
    pub lines: Vec<SubagentSourceLine>,
    /// In-progress (unterminated) line, replaced each frame while live.
    pub tail: Option<SubagentSourceLine>,
    /// Number of committed lines the current `rows` reflect. A mismatch forces
    /// a full rebuild (append or collapse); equality lets the tail-only fast
    /// path replace the final row.
    pub rows_built_for: usize,
    /// Flat visible rows (content + block chrome) for the current state.
    pub rows: Vec<SubagentRow>,
    /// Line count shown in the header (transcript logical lines).
    pub line_count: usize,
}

/// Chrome labels for the subagent popup, copied from i18n `Messages` once per
/// frame (all `&'static str` — no allocation).
#[derive(Debug, Clone)]
pub struct SubagentLabels {
    pub live_header: &'static str,
    pub done_header: &'static str,
    pub thinking_title: &'static str,
    pub lines_footer: &'static str,
    pub tool_footer: &'static str,
}

impl SubagentLabels {
    pub fn from_messages(m: &crate::i18n::Messages) -> Self {
        Self {
            live_header: m.subagent_popup_live_header,
            done_header: m.subagent_popup_done_header,
            thinking_title: m.subagent_popup_thinking_title,
            lines_footer: m.subagent_popup_lines_footer,
            tool_footer: m.subagent_popup_tool_footer,
        }
    }
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
