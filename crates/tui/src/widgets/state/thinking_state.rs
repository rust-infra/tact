use std::{
    collections::VecDeque,
    time::{Duration, Instant},
};

use super::tool_state::PopupTextSelection;

/// Streaming thinking content anchored at one shared-log placeholder row.
#[derive(Debug, Clone)]
pub(crate) struct ActiveThinkingBlock {
    pub(crate) phys_idx: usize,
    pub(crate) content: String,
    pending_line: String,
    completed_tail: VecDeque<String>,
    pub(crate) started_at: Instant,
}

impl ActiveThinkingBlock {
    const MAX_TAIL_LINES: usize = 3;

    pub(crate) fn new(phys_idx: usize, started_at: Instant) -> Self {
        Self {
            phys_idx,
            content: String::new(),
            pending_line: String::new(),
            completed_tail: VecDeque::new(),
            started_at,
        }
    }

    pub(crate) fn push_delta(&mut self, delta: &str) {
        self.content.push_str(delta);
        self.pending_line.push_str(delta);

        while let Some(newline) = self.pending_line.find('\n') {
            let line = self.pending_line[..newline].to_string();
            self.pending_line.drain(..=newline);
            self.completed_tail.push_back(line);
            if self.completed_tail.len() > Self::MAX_TAIL_LINES {
                self.completed_tail.pop_front();
            }
        }
    }

    pub(crate) fn display_tail(&self) -> Vec<String> {
        let mut tail: Vec<_> = self.completed_tail.iter().cloned().collect();
        if !self.pending_line.is_empty() {
            tail.push(self.pending_line.clone());
        }
        if tail.len() > Self::MAX_TAIL_LINES {
            tail.drain(..tail.len() - Self::MAX_TAIL_LINES);
        }
        tail
    }

    pub(crate) fn body_line_count(&self) -> usize {
        self.display_tail().len().clamp(1, Self::MAX_TAIL_LINES)
    }

    pub(crate) fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }
}

/// Thinking state: one active direct card, completed cards, and the detail popup.
#[derive(Default)]
pub(crate) struct ThinkingState {
    /// Reasoning card currently receiving streaming deltas.
    pub(crate) active: Option<ActiveThinkingBlock>,
    /// Completed reasoning cards, retained for rendering and detail popups.
    pub(crate) blocks: Vec<ThinkingBlock>,
    /// Detail popup state.
    pub(crate) popup: Option<ThinkingPopup>,
}

/// A completed reasoning card anchored at one shared-log placeholder row.
#[derive(Debug, Clone)]
pub(crate) struct ThinkingBlock {
    pub(crate) phys_idx: usize,
    pub(crate) content: String,
    pub(crate) summary: String,
    /// Cached Markdown rendered lines, used for popup display, avoiding per-frame re-rendering.
    pub(crate) cached_markdown: Vec<ratatui::text::Line<'static>>,
    /// Duration of the thinking phase.
    pub(crate) elapsed: Duration,
}

/// Thinking popup state.
#[derive(Debug, Clone)]
pub(crate) struct ThinkingPopup {
    /// Stable shared-log placeholder index for active or completed content.
    pub phys_idx: usize,
    pub title: String,
    /// Popup internal scroll offset (line number, relative to the first thinking content line).
    pub scroll: u16,
    /// Byte selection into `selection_text`.
    pub selection: Option<PopupTextSelection>,
    /// Plain text currently presented as selectable Thinking content.
    pub selection_text: String,
}

impl ThinkingPopup {
    pub(crate) fn copy_content(&self, full_content: &str) -> String {
        self.selection
            .and_then(|selection| selection.normalized_non_empty(&self.selection_text))
            .map(|range| self.selection_text[range].to_string())
            .unwrap_or_else(|| full_content.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn active_thinking_tail_grows_then_keeps_latest_three_lines() {
        let mut active = ActiveThinkingBlock::new(8, Instant::now());
        active.push_delta("one\ntwo\nthree\nfour\n");

        assert_eq!(active.display_tail(), vec!["two".to_string(), "three".to_string(), "four".to_string()]);
    }

    #[test]
    fn active_thinking_tail_includes_unterminated_fragment() {
        let mut active = ActiveThinkingBlock::new(8, Instant::now());
        active.push_delta("one\ntwo");

        assert_eq!(active.display_tail(), vec!["one".to_string(), "two".to_string()]);
    }
}
