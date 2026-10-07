//! The shared list-popup component.
//!
//! One place for the four things every list popup repeated: geometry, chrome,
//! the scroll window, and the focused-row style. Callers supply the rows and
//! the sizing policy; everything else is here.
//!
//! See `docs/superpowers/specs/2026-10-04-list-popup-component-design.md`.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};

use crate::{
    render::popups::{centered_list_popup_area, popup_inner},
    theme::Theme,
};

/// One row of a list popup.
///
/// Build the row *as if it were not focused*: in [`SelectionStyle::Highlight`]
/// the component owns the focused row's colors. Focus-dependent *text* (the
/// `▶ ` / two-space prefix) stays the caller's job, because only the caller
/// knows the row's column layout.
pub struct ListRow<'a> {
    pub spans: Vec<Span<'a>>,
    /// Group headers (palette categories, slash sections) draw but never focus;
    /// the window treats them as ordinary rows.
    pub selectable: bool,
}

impl<'a> ListRow<'a> {
    /// A focusable row.
    pub fn item(spans: Vec<Span<'a>>) -> Self {
        Self {
            spans,
            selectable: true,
        }
    }

    /// A non-focusable group header.
    pub fn header(spans: Vec<Span<'a>>) -> Self {
        Self {
            spans,
            selectable: false,
        }
    }
}

/// How the focused row is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionStyle {
    /// `theme.highlight` across the row's full inner width, text in
    /// `theme.fg`. Used by the palette, the file picker and the select popup.
    Highlight,
    /// No background and no color override: the caller's spans already carry
    /// the focus styling. Used by the slash-command list, which draws its
    /// focused row in `theme.accent`.
    CallerStyled,
}

/// Where the popup landed and which slice of the rows it shows.
pub struct ListPopupLayout {
    /// Outer popup rect, borders included.
    pub popup_area: Rect,
    /// Content rect inside the borders.
    pub inner: Rect,
    /// Number of rows visible inside the window.
    pub visible: usize,
    /// Index of the first visible row.
    pub offset: usize,
}

/// The one scroll-window rule: keep `selected_row` inside
/// `[offset, offset + visible)`, pinned near the bottom with ~2 rows of
/// context below it once the list overflows.
pub fn window_offset(row_count: usize, selected_row: usize, visible: usize) -> usize {
    if row_count <= visible || visible == 0 {
        return 0;
    }
    let max_offset = row_count - visible;
    let pin = visible.saturating_sub(3).min(visible.saturating_sub(1));
    selected_row.saturating_sub(pin).min(max_offset)
}

/// A centered list popup: `Clear`, border, title, optional bottom footer,
/// optional header rows above the list, a scrolled window of rows, and a
/// muted empty hint.
///
/// `width` and `height` are the caller's sizing policy — the component only
/// caps them to `area`. `height` is the desired *outer* height.
pub struct ListPopup<'a> {
    theme: &'a Theme,
    rows: &'a [ListRow<'a>],
    selected_row: usize,
    title: Line<'static>,
    footer: Option<Line<'static>>,
    header: Vec<Line<'static>>,
    empty_text: &'a str,
    selection: SelectionStyle,
    border_type: BorderType,
    accent_border: bool,
    bg: Option<Color>,
    width: u16,
    height: u16,
}

impl<'a> ListPopup<'a> {
    pub fn new(theme: &'a Theme, rows: &'a [ListRow<'a>], width: u16, height: u16) -> Self {
        Self {
            theme,
            rows,
            selected_row: 0,
            title: Line::default(),
            footer: None,
            header: Vec::new(),
            empty_text: "",
            selection: SelectionStyle::Highlight,
            border_type: theme.block_border_type(),
            accent_border: false,
            bg: None,
            width,
            height,
        }
    }

    /// Index of the focused row (a row index, not an item index — group
    /// headers occupy rows).
    pub fn selected_row(mut self, selected_row: usize) -> Self {
        self.selected_row = selected_row;
        self
    }

    /// Top-border title.
    pub fn title(mut self, title: impl Into<Line<'static>>) -> Self {
        self.title = title.into();
        self
    }

    /// Bottom-border hint line.
    pub fn footer(mut self, footer: Line<'static>) -> Self {
        self.footer = Some(footer);
        self
    }

    /// Rows drawn above the list, followed by one blank separator row.
    pub fn header(mut self, header: Vec<Line<'static>>) -> Self {
        self.header = header;
        self
    }

    /// Hint drawn in the first content row when there are no rows.
    pub fn empty_text(mut self, empty_text: &'a str) -> Self {
        self.empty_text = empty_text;
        self
    }

    pub fn selection(mut self, selection: SelectionStyle) -> Self {
        self.selection = selection;
        self
    }

    pub fn border_type(mut self, border_type: BorderType) -> Self {
        self.border_type = border_type;
        self
    }

    /// Draw the border in `theme.accent` (the slash-command list).
    pub fn accent_border(mut self, accent: bool) -> Self {
        self.accent_border = accent;
        self
    }

    /// Popup interior background. `None` leaves whatever `Clear` wrote.
    pub fn bg(mut self, bg: Option<Color>) -> Self {
        self.bg = bg;
        self
    }

    /// Rows drawn above the list, including the blank separator.
    fn header_rows(&self) -> u16 {
        if self.header.is_empty() {
            0
        } else {
            self.header.len() as u16 + 1
        }
    }

    /// Resolve the popup rect and the visible window for `area`.
    pub fn layout(&self, area: Rect) -> ListPopupLayout {
        let popup_area = centered_list_popup_area(area, self.width, self.height);
        let inner = popup_inner(popup_area);
        let available = inner.height.saturating_sub(self.header_rows()) as usize;
        let visible = self.rows.len().min(available);
        let offset = window_offset(self.rows.len(), self.selected_row, visible);
        ListPopupLayout {
            popup_area,
            inner,
            visible,
            offset,
        }
    }
}

impl Widget for ListPopup<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let ListPopupLayout {
            popup_area,
            inner,
            visible,
            offset,
        } = self.layout(area);

        Clear.render(popup_area, buf);

        let mut block = Block::default()
            .borders(Borders::ALL)
            .border_type(self.border_type)
            .title(self.title.clone());
        if let Some(bg) = self.bg {
            block = block.style(Style::default().bg(bg));
        }
        if self.accent_border {
            block = block.border_style(Style::default().fg(self.theme.accent));
        }
        if let Some(footer) = self.footer.clone() {
            block = block.title_bottom(footer);
        }
        block.render(popup_area, buf);

        let mut y = inner.y;
        for line in &self.header {
            Paragraph::new(line.clone()).render(Rect::new(inner.x, y, inner.width, 1), buf);
            y += 1;
        }
        if !self.header.is_empty() {
            y += 1;
        }

        if self.rows.is_empty() {
            Paragraph::new(Line::from(Span::styled(
                self.empty_text,
                Style::default().fg(self.theme.muted),
            )))
            .render(Rect::new(inner.x, y, inner.width, 1), buf);
            return;
        }

        let end = (offset + visible).min(self.rows.len());
        for (i, row) in self.rows[offset..end].iter().enumerate() {
            // A group header never lights up, even if the caller's row index
            // lands on one.
            let focused = offset + i == self.selected_row
                && row.selectable
                && self.selection == SelectionStyle::Highlight;
            let row_area = Rect::new(inner.x, y + i as u16, inner.width, 1);

            let spans: Vec<Span<'_>> = if focused {
                // The focused row's text is the theme's foreground whatever the
                // caller colored it: a per-extension color on a highlight band
                // is where the light themes became unreadable.
                row.spans
                    .iter()
                    .cloned()
                    .map(|span| span.patch_style(Style::default().fg(self.theme.fg)))
                    .collect()
            } else {
                row.spans.clone()
            };

            if focused {
                // Row-wide, not per-glyph: a span's background only covers its
                // own columns, which leaves the row tail looking torn.
                buf.set_style(row_area, Style::default().bg(self.theme.highlight));
            }

            Paragraph::new(Line::from(spans)).render(row_area, buf);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;

    fn theme() -> Theme {
        Theme::from(crate::theme::ThemeName::Dark)
    }

    fn rows(count: usize) -> Vec<ListRow<'static>> {
        (0..count)
            .map(|i| ListRow::item(vec![Span::raw(format!("row-{i:02}"))]))
            .collect()
    }

    #[test]
    fn window_offset_keeps_selection_visible() {
        // 30 rows in a 10-row window, selection near the end.
        let offset = window_offset(30, 25, 10);
        assert!(
            offset <= 25 && 25 < offset + 10,
            "selected row must be inside the window (offset={offset})"
        );
    }

    #[test]
    fn window_offset_is_zero_when_everything_fits() {
        assert_eq!(window_offset(5, 4, 5), 0);
        assert_eq!(window_offset(5, 4, 10), 0);
    }

    #[test]
    fn window_offset_clamps_at_the_end() {
        assert_eq!(window_offset(30, 29, 10), 20);
    }

    #[test]
    fn window_offset_handles_an_empty_window() {
        assert_eq!(window_offset(30, 25, 0), 0);
    }

    #[test]
    fn window_offset_pins_with_two_rows_of_context_below() {
        // pin = visible - 3, so the selection sits 3 rows above the last.
        assert_eq!(window_offset(30, 25, 10), 18);
    }

    fn render(popup: ListPopup<'_>, width: u16, height: u16) -> Buffer {
        let area = Rect::new(0, 0, width, height);
        let mut buf = Buffer::empty(area);
        popup.render(area, &mut buf);
        buf
    }

    #[test]
    fn layout_caps_the_requested_size_to_the_area() {
        let theme = theme();
        let rows = rows(3);
        let popup = ListPopup::new(&theme, &rows, 200, 200);
        let layout = popup.layout(Rect::new(0, 0, 40, 12));
        assert_eq!(layout.popup_area.width, 40);
        assert_eq!(layout.popup_area.height, 12);
    }

    #[test]
    fn layout_subtracts_the_header_and_its_separator() {
        let theme = theme();
        let rows = rows(10);
        let popup = ListPopup::new(&theme, &rows, 20, 10).header(vec![Line::from("prompt")]);
        let layout = popup.layout(Rect::new(0, 0, 40, 20));
        // 10 outer - 2 borders - 1 header - 1 separator = 6 rows.
        assert_eq!(layout.visible, 6);
    }

    #[test]
    fn focused_row_band_covers_the_full_inner_width() {
        let theme = theme();
        let rows = rows(3);
        let popup = ListPopup::new(&theme, &rows, 20, 5)
            .selected_row(1)
            .bg(Some(theme.bottom_bar_bg));
        let layout = popup.layout(Rect::new(0, 0, 40, 12));
        let buf = render(popup, 40, 12);

        let band_y = layout.inner.y + 1;
        for x in layout.inner.left()..layout.inner.right() {
            assert_eq!(
                buf[(x, band_y)].bg,
                theme.highlight,
                "focused row must be painted row-wide (x={x})"
            );
        }
        // The unfocused rows keep the popup background.
        assert_eq!(
            buf[(layout.inner.left(), layout.inner.y)].bg,
            theme.bottom_bar_bg
        );
    }

    #[test]
    fn focused_row_text_is_the_theme_foreground() {
        let theme = theme();
        let rows = vec![ListRow::item(vec![Span::styled(
            "main.rs",
            Style::default().fg(Color::Rgb(239, 146, 65)),
        )])];
        let buf = render(ListPopup::new(&theme, &rows, 20, 3), 40, 12);
        let layout = ListPopup::new(&theme, &rows, 20, 3).layout(Rect::new(0, 0, 40, 12));
        assert_eq!(buf[(layout.inner.x, layout.inner.y)].fg, theme.fg);
    }

    #[test]
    fn caller_styled_selection_leaves_the_row_alone() {
        let theme = theme();
        let rows = vec![ListRow::item(vec![Span::styled(
            "▶ /theme",
            Style::default().fg(theme.accent),
        )])];
        let popup = ListPopup::new(&theme, &rows, 20, 3)
            .selection(SelectionStyle::CallerStyled)
            .bg(Some(theme.bottom_bar_bg));
        let layout = popup.layout(Rect::new(0, 0, 40, 12));
        let buf = render(popup, 40, 12);
        assert_eq!(buf[(layout.inner.x, layout.inner.y)].fg, theme.accent);
        assert_eq!(
            buf[(layout.inner.x, layout.inner.y)].bg,
            theme.bottom_bar_bg,
            "no highlight band in caller-styled mode"
        );
    }

    #[test]
    fn a_group_header_at_the_selected_row_is_never_highlighted() {
        let theme = theme();
        let rows = vec![
            ListRow::header(vec![Span::raw("  Tools")]),
            ListRow::item(vec![Span::raw("row-00")]),
        ];
        let popup = ListPopup::new(&theme, &rows, 20, 4).selected_row(0);
        let layout = popup.layout(Rect::new(0, 0, 40, 12));
        let buf = render(popup, 40, 12);
        assert_ne!(
            buf[(layout.inner.x, layout.inner.y)].bg,
            theme.highlight,
            "a group header must not carry the focus band"
        );
    }

    #[test]
    fn empty_list_draws_the_muted_hint() {
        let theme = theme();
        let rows: Vec<ListRow> = Vec::new();
        let popup = ListPopup::new(&theme, &rows, 20, 3).empty_text("No options");
        let layout = popup.layout(Rect::new(0, 0, 40, 12));
        let buf = render(popup, 40, 12);
        assert_eq!(buf[(layout.inner.x, layout.inner.y)].fg, theme.muted);
        assert_eq!(buf[(layout.inner.x, layout.inner.y)].symbol(), "N");
    }

    #[test]
    fn window_scrolls_a_deep_selection_into_view() {
        let theme = theme();
        let rows = rows(30);
        let popup = ListPopup::new(&theme, &rows, 20, 6).selected_row(25);
        let area = Rect::new(0, 0, 40, 20);
        let layout = popup.layout(area);
        let buf = render(popup, 40, 20);

        let text: String = (0..40)
            .map(|x| buf[(x, layout.inner.y)].symbol().to_string())
            .collect();
        assert!(
            !text.contains("row-00"),
            "the top of the list must scroll out of view: {text}"
        );

        let mut found = false;
        for y in layout.inner.top()..layout.inner.bottom() {
            let row: String = (0..40).map(|x| buf[(x, y)].symbol().to_string()).collect();
            if row.contains("row-25") {
                found = true;
            }
        }
        assert!(found, "the selected row must be visible");
    }
}
