use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// Renderable unit that knows its visual height and how to draw.
pub(crate) trait Renderable {
    /// Draw all visual lines within the specified area.
    fn render(&self, area: Rect, buf: &mut Buffer);

    /// Draw starting from the specified line offset; default impl delegates to render (ignoring offset).
    fn render_partial(&self, area: Rect, buf: &mut Buffer, _skip_lines: usize) {
        self.render(area, buf);
    }

    /// Number of visual lines at the given width (height after wrapping).
    fn height(&self, width: u16) -> u16;
}
