use std::time::Instant;

use crate::protocol::{SubagentTranscriptEntry, ToolOutputBuffer};
use ratatui::text::Line;

use crate::render::popups::subagent_popup::SubagentPopupLayout;
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
    /// Subagent conversations, keyed by the parent `spawn_subagent` tool id.
    ///
    /// Kept beside `active`/`blocks` rather than inside
    /// [`ToolRenderOutput`]: the log renderer clones every output once per
    /// frame, and a transcript is the whole child conversation. It outlives
    /// the card (a finalized subagent's popup can be re-opened), so it is
    /// never pruned on finalize.
    pub transcripts: std::collections::HashMap<String, SubagentTranscript>,
}

/// One subagent's structured conversation.
#[derive(Debug, Clone, Default)]
pub struct SubagentTranscript {
    pub entries: Vec<SubagentTranscriptEntry>,
    /// Append counter — the popup layout cache's invalidation key, so frames
    /// between two appends reuse the built layout instead of re-wrapping the
    /// whole conversation.
    pub revision: u64,
}

impl SubagentTranscript {
    /// Append an entry, folding the second half of a tool call into the first.
    ///
    /// `StepStarted` and `StepFinished` / `StepFailed` share the child's local
    /// `tool_id`, so the terminal half **rewrites** the in-flight entry rather
    /// than appending a second card — the same shape as the main log's
    /// `ActiveToolBlock` → `ToolBlock` transition.
    pub fn push(&mut self, entry: SubagentTranscriptEntry) {
        if let SubagentTranscriptEntry::Step(step) = &entry
            && let Some(existing) = self.entries.iter_mut().rev().find(|existing| {
                matches!(existing, SubagentTranscriptEntry::Step(s) if s.tool_id == step.tool_id)
            })
        {
            *existing = entry;
            self.revision += 1;
            return;
        }
        self.entries.push(entry);
        self.revision += 1;
    }

    /// Plain text of the whole conversation, for `y` copy.
    pub fn selection_text(&self) -> String {
        self.entries
            .iter()
            .map(SubagentTranscriptEntry::selection_text)
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

/// A tool invocation that has started but not yet finished.
#[derive(Debug, Clone)]
pub struct ActiveToolBlock {
    pub phys_idx: usize,
    pub tool_id: String,
    pub output: ToolRenderOutput,
    pub live_output: ToolOutputBuffer,
    pub started_at: Instant,
    /// For `spawn_subagent` with `run_in_background`, the child session id
    /// parsed from the `async_launched { id }` result — lets the TUI render a
    /// cancel button while the subagent card is live.
    pub subagent_child_id: Option<String>,
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
    /// Raw tool id (`read_image`, `bash`, …) printed at the front of the bottom
    /// border, so the popup names the tool it came from even when the title is
    /// a path or an argument. `None` for popups not opened from a tool block.
    pub tool_name: Option<String>,
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

/// Popup preview state for a subagent's structured transcript.
///
/// The body is rebuilt as **segments** (meta row, prompt, assistant Markdown,
/// thinking cards, tool cards) and drawn through the log's own
/// `LogColumnRenderer`, so a child conversation renders with the same cells as
/// the main log — and renders the same way whether the child is still running
/// or has finished.
#[derive(Debug, Clone)]
pub struct SubagentPopup {
    pub title: String,
    pub scroll: u16,
    /// Tool id of the spawn_subagent invocation this popup belongs to.
    pub tool_id: String,
    /// Current text selection (mouse-drag), if any.
    pub selection: Option<PopupTextSelection>,
    /// Built segments + selection text, reused while `revision`, width, theme
    /// and live-ness are unchanged.
    pub layout: Option<SubagentPopupLayout>,
    /// Child step ids whose tool block shows the raised cap.
    ///
    /// Per block, not per view: a child that ran 34 steps with every block at
    /// 50 lines is unreadable, and the reader expands the block they are
    /// actually looking at. Persists across re-opens, so re-entering the view
    /// keeps the reader's place.
    pub expanded_steps: std::collections::HashSet<String>,
    /// Bumped on every expansion toggle — the layout cache key's expansion
    /// component (a set has no cheap identity, and the layout only needs to
    /// know *that* the set changed).
    pub expanded_rev: u64,
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
