//! Byte-range text selection shared by thinking / diff / subagent popups.

/// A byte-range selection into popup text (anchor + active edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PopupTextSelection {
    pub anchor: usize,
    pub active: usize,
}

impl PopupTextSelection {
    pub fn new(anchor: usize, active: usize) -> Self {
        Self { anchor, active }
    }

    /// Normalize to a non-empty byte range clamped to `content`, preserving
    /// UTF-8 char boundaries.
    pub fn normalized_non_empty(&self, content: &str) -> Option<std::ops::Range<usize>> {
        let mut start = self.anchor.min(self.active).min(content.len());
        let mut end = self.anchor.max(self.active).min(content.len());
        while start > 0 && !content.is_char_boundary(start) {
            start -= 1;
        }
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        (start < end).then_some(start..end)
    }
}
