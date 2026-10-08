use std::{
    collections::VecDeque,
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

static NEXT_THINKING_ID: AtomicU64 = AtomicU64::new(1);

fn next_thinking_id() -> String {
    format!(
        "thinking-{}",
        NEXT_THINKING_ID.fetch_add(1, Ordering::Relaxed)
    )
}

use super::{log_scroll::LogScroll, selection::PopupTextSelection};
/// Streaming thinking content anchored at one shared-log placeholder row.
#[derive(Debug, Clone)]
pub struct ActiveThinkingBlock {
    pub block_id: String,
    pub phys_idx: usize,
    pub content: String,
    pending_line: String,
    completed_tail: VecDeque<String>,
    pub started_at: Instant,
}

impl ActiveThinkingBlock {
    const MAX_TAIL_LINES: usize = 3;

    pub fn new(phys_idx: usize, started_at: Instant) -> Self {
        Self {
            block_id: next_thinking_id(),
            phys_idx,
            content: String::new(),
            pending_line: String::new(),
            completed_tail: VecDeque::new(),
            started_at,
        }
    }

    pub fn push_delta(&mut self, delta: &str) {
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

    pub fn display_tail(&self) -> Vec<String> {
        let mut tail: Vec<_> = self.completed_tail.iter().cloned().collect();
        if !self.pending_line.is_empty() {
            tail.push(self.pending_line.clone());
        }
        if tail.len() > Self::MAX_TAIL_LINES {
            tail.drain(..tail.len() - Self::MAX_TAIL_LINES);
        }
        tail
    }

    pub fn body_line_count(&self) -> usize {
        self.display_tail().len().clamp(1, Self::MAX_TAIL_LINES)
    }

    pub fn is_blank(&self) -> bool {
        self.content.trim().is_empty()
    }
}

/// Thinking state: one active direct card, completed cards, and the detail popup.
#[derive(Default)]
pub struct ThinkingState {
    pub active: Option<ActiveThinkingBlock>,
    pub blocks: Vec<ThinkingBlock>,
    pub popup: Option<ThinkingPopup>,
}

/// A completed reasoning card anchored at one shared-log placeholder row.
#[derive(Debug, Clone)]
pub struct ThinkingBlock {
    pub block_id: String,
    pub phys_idx: usize,
    pub content: String,
    pub summary: String,
    pub cached_markdown: Vec<ratatui::text::Line<'static>>,
    pub elapsed: Duration,
}

impl ThinkingBlock {
    #[must_use]
    pub fn new(
        block_id: String,
        phys_idx: usize,
        content: String,
        summary: String,
        cached_markdown: Vec<ratatui::text::Line<'static>>,
        elapsed: Duration,
    ) -> Self {
        Self {
            block_id,
            phys_idx,
            content,
            summary,
            cached_markdown,
            elapsed,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ThinkingPopup {
    pub block_id: String,
    pub phys_idx: usize,
    pub title: String,
    pub scroll: u16,
    pub selection: Option<PopupTextSelection>,
    pub selection_text: String,
}

impl ThinkingPopup {
    #[must_use]
    pub fn new(block_id: String, phys_idx: usize, title: String) -> Self {
        Self {
            block_id,
            phys_idx,
            title,
            scroll: 0,
            selection: None,
            selection_text: String::new(),
        }
    }

    pub fn copy_content(&self, full_content: &str) -> String {
        self.selection
            .and_then(|selection| selection.normalized_non_empty(&self.selection_text))
            .map(|range| self.selection_text[range].to_string())
            .unwrap_or_else(|| full_content.to_string())
    }
}

/// Locate the thinking card (active or completed) that spans a logical line.
/// Returns `(phys_idx, logical_start, rows)` for the first card whose range contains `line_idx`.
pub fn find_thinking_at_logical(
    log_scroll: &LogScroll,
    thinking: &ThinkingState,
    line_idx: usize,
) -> Option<(usize, usize, usize)> {
    let find = |phys_idx: usize, rows: usize| {
        let logical_start = log_scroll
            .phys_to_logical_cache
            .get(phys_idx)
            .copied()
            .flatten()?;
        (line_idx >= logical_start && line_idx < logical_start + rows).then_some((
            phys_idx,
            logical_start,
            rows,
        ))
    };
    if let Some(active) = thinking.active.as_ref()
        && let Some(found) = find(
            active.phys_idx,
            crate::render::cells::thinking::thinking_visual_rows(active.body_line_count()),
        )
    {
        return Some(found);
    }
    thinking.blocks.iter().find_map(|block| {
        find(
            block.phys_idx,
            crate::render::cells::thinking::thinking_visual_rows(1),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn active_thinking_ids_are_distinct() {
        let a = ActiveThinkingBlock::new(1, Instant::now());
        let b = ActiveThinkingBlock::new(1, Instant::now());
        assert_ne!(a.block_id, b.block_id);
    }
}
