use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;
use super::renderable::Renderable;

/// Log column layout renderer: arranges and draws Renderable units by visual offset.
pub(crate) struct LogColumnRenderer<'a> {
    /// List of (visual starting row, renderable unit), sorted by ascending visual row.
    cells: Vec<(usize, Box<dyn Renderable + 'a>)>,
    /// Viewport top visual row number.
    viewport_top: usize,
    /// Number of visible lines in the viewport.
    viewport_height: usize,
}

impl<'a> LogColumnRenderer<'a> {
    pub(crate) fn new() -> Self {
        LogColumnRenderer {
            cells: Vec::new(),
            viewport_top: 0,
            viewport_height: 0,
        }
    }

    pub(crate) fn with_viewport(mut self, top: usize, height: usize) -> Self {
        self.viewport_top = top;
        self.viewport_height = height;
        self
    }

    pub(crate) fn push(&mut self, vis_start: usize, cell: impl Renderable + 'a) {
        self.cells.push((vis_start, Box::new(cell)));
    }
}

impl Widget for LogColumnRenderer<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let viewport_bottom = self.viewport_top + self.viewport_height;
        for (vis_start, cell) in &self.cells {
            let cell_height = cell.height(area.width) as usize;
            let vis_end = vis_start + cell_height;
            if vis_end <= self.viewport_top || *vis_start >= viewport_bottom {
                continue;
            }

            // Calculate visible portion
            let visible_start = (*vis_start).max(self.viewport_top);
            let visible_end = vis_end.min(viewport_bottom);
            let skip_lines = visible_start - vis_start;
            let visible_lines = visible_end - visible_start;

            let y = area.y + (visible_start - self.viewport_top) as u16;
            let cell_area = Rect::new(area.x, y, area.width, visible_lines as u16);

            // Only render rows within the viewport: from skip_lines, at most visible_lines rows
            cell.render_partial(cell_area, buf, skip_lines);
        }
    }
}
