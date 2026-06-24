use ratatui::{
    buffer::Buffer,
    layout::Rect,
    text::Line,
    widgets::Widget,
};

use super::super::renderable::Renderable;

/// A blank line separator drawn between message groups of different
/// categories (user ↔ system ↔ assistant).
pub(crate) struct MessageSeparator {
    label: String,
    fg: ratatui::style::Color,
}

impl MessageSeparator {
    pub(crate) fn new(label: String, fg: ratatui::style::Color) -> Self {
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
        ratatui::widgets::Paragraph::new(blank_line).render(gap_area, buf);
    }

    fn height(&self, _width: u16) -> u16 {
        1
    }
}
