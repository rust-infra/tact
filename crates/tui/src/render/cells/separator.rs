use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
};

use super::super::renderable::Renderable;

/// Sentinel stored in `raw_messages` for task-end rules (rendered dynamically).
pub(crate) const TASK_END_SEPARATOR: &str = "\x07tact-task-end";

pub(crate) fn is_task_end_separator(raw: &str) -> bool {
    raw == TASK_END_SEPARATOR
}

/// Full-width accent-colored rule appended after a completed task response.
pub(crate) struct TaskEndSeparator {
    fg: Color,
}

impl TaskEndSeparator {
    pub(crate) fn new(fg: Color) -> Self {
        Self { fg }
    }

    fn solid_line(width: u16) -> String {
        "─".repeat(width as usize)
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
        let style = Style::default().fg(self.fg);
        let line = Line::from(Span::styled(Self::solid_line(area.width), style));
        Paragraph::new(line).render(area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}

/// A blank line separator drawn between message groups of different
/// categories (user ↔ system ↔ assistant).
pub(crate) struct MessageSeparator {
    _label: String,
    _fg: Color,
}

impl MessageSeparator {
    pub(crate) fn new(label: String, fg: Color) -> Self {
        Self {
            _label: label,
            _fg: fg,
        }
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

#[cfg(test)]
mod render_tests {
    use super::*;

    #[test]
    fn task_end_separator_renders_solid_line() {
        let sep = TaskEndSeparator::new(Color::Gray);
        let area = Rect::new(0, 0, 20, 1);
        let mut buf = Buffer::empty(area);
        sep.render(area, &mut buf);
        let text: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert_eq!(
            text, "────────────────────",
            "task end separator should draw solid line, got: {text}"
        );
    }

    #[test]
    fn message_separator_renders_blank_gap_line() {
        let sep = MessageSeparator::new("💬 user".into(), Color::Cyan);
        let area = Rect::new(0, 0, 10, 1);
        let mut buf = Buffer::empty(area);
        sep.render(area, &mut buf);
        assert_eq!(sep.height(10), 1);
        let rendered: String = (0..area.width)
            .map(|x| buf[(x, 0)].symbol().to_string())
            .collect();
        assert!(
            rendered.trim().is_empty(),
            "message separator row should stay visually blank"
        );
    }
}
