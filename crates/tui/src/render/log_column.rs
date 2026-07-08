use super::renderable::Renderable;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::widgets::Widget;

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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Style;

    /// Minimal Renderable emitting `id` repeated across `rows` lines.
    struct StubCell {
        id: char,
        rows: usize,
    }

    impl Renderable for StubCell {
        fn render(&self, area: Rect, buf: &mut Buffer) {
            self.render_partial(area, buf, 0);
        }

        fn height(&self, _width: u16) -> u16 {
            self.rows as u16
        }

        fn render_partial(&self, area: Rect, buf: &mut Buffer, skip_lines: usize) {
            for i in skip_lines..self.rows {
                let y = area.y + (i - skip_lines) as u16;
                if y >= area.y + area.height {
                    break;
                }
                buf.set_string(area.x, y, self.id.to_string(), Style::default());
            }
        }
    }

    fn buffer_text(buf: &Buffer) -> String {
        let mut out = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                out.push_str(buf[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_visible_cells_at_correct_offset() {
        let mut r = LogColumnRenderer::new().with_viewport(0, 5);
        r.push(0, StubCell { id: 'A', rows: 2 });
        r.push(2, StubCell { id: 'B', rows: 2 });

        let area = Rect::new(0, 0, 3, 5);
        let mut buf = Buffer::empty(area);
        r.render(area, &mut buf);

        let text = buffer_text(&buf);
        assert!(text.contains('A'), "cell A should render: {text}");
        assert!(text.contains('B'), "cell B should render: {text}");
    }

    #[test]
    fn skips_cells_above_viewport() {
        let mut r = LogColumnRenderer::new().with_viewport(10, 5);
        r.push(0, StubCell { id: 'X', rows: 3 });

        let area = Rect::new(0, 0, 3, 5);
        let mut buf = Buffer::empty(area);
        r.render(area, &mut buf);

        assert!(
            !buffer_text(&buf).contains('X'),
            "cell entirely above viewport must not render"
        );
    }

    #[test]
    fn partially_visible_cell_is_clipped() {
        // Cell spans visual rows 0..4 but viewport starts at row 2.
        let mut r = LogColumnRenderer::new().with_viewport(2, 5);
        r.push(0, StubCell { id: 'C', rows: 4 });

        let area = Rect::new(0, 0, 3, 5);
        let mut buf = Buffer::empty(area);
        r.render(area, &mut buf);

        // Two rows (indices 2,3) remain visible → 'C' appears on first two buffer rows.
        let text = buffer_text(&buf);
        assert!(
            text.starts_with("C"),
            "clipped cell should draw from top: {text}"
        );
    }
}
