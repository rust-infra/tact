use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::super::renderable::Renderable;

/// Sentinel stored in `raw_messages` for task-end rules (rendered dynamically).
pub(crate) const TASK_END_SEPARATOR: &str = "\x07tact-task-end";

pub(crate) fn is_task_end_separator(raw: &str) -> bool {
    raw == TASK_END_SEPARATOR
}

/// Full-width dim dashed rule appended after a completed task response.
pub(crate) struct TaskEndSeparator {
    fg: Color,
}

impl TaskEndSeparator {
    pub(crate) fn new(fg: Color) -> Self {
        Self { fg }
    }

    fn dashed_line(width: u16) -> String {
        (0..width as usize)
            .map(|i| if i % 2 == 0 { '─' } else { ' ' })
            .collect()
    }
}

impl Renderable for TaskEndSeparator {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if skip_lines >= 1 || area.height == 0 || area.width == 0 {
            return;
        }
        let style = Style::default().fg(self.fg).add_modifier(Modifier::DIM);
        let line = Line::from(Span::styled(
            Self::dashed_line(area.width),
            style,
        ));
        Paragraph::new(line).render(area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}

/// A blank line separator drawn between message groups of different
/// categories (user ↔ system ↔ assistant).
pub(crate) struct MessageSeparator {
    label: String,
    fg: Color,
}

impl MessageSeparator {
    pub(crate) fn new(label: String, fg: Color) -> Self {
        Self { label, fg }
    }
}

impl Renderable for MessageSeparator {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        self.render_partial(area, buf, 0);
    }

    fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
        if skip_lines >= 1 || area.height == 0 {
            return;
        }
        // Single blank line to separate message groups
        let blank_line = Line::from("");
        let gap_area = Rect::new(area.x, area.y, area.width, 1);
        Paragraph::new(blank_line).render(gap_area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}
