//! Shared scrollable popup skeleton.
//!
//! One place for the three things every scrollable popup repeated: chrome,
//! scroll window, and scrollbar. Callers supply the title and content lines;
//! everything else is here.

use ratatui::{
    Frame,
    layout::Rect,
    text::{Line, Text},
    widgets::{Paragraph, Wrap},
};

use super::{FooterHint, centered_popup_area, render_popup_chrome};

/// A scrollable text popup: shared chrome, scroll window, and scrollbar.
///
/// Callers supply the title and content lines; the component owns geometry,
/// the bordered chrome, the visible slice, and the scrollbar.
pub struct ScrollableTextPopup<'a> {
    theme: &'a crate::theme::Theme,
    title: &'a str,
    lines: &'a [Line<'static>],
    scroll: usize,
    footer_hints: &'a [FooterHint],
    footer_note: Option<&'a str>,
    copy_done: Option<&'a str>,
}

impl<'a> ScrollableTextPopup<'a> {
    pub fn new(theme: &'a crate::theme::Theme, title: &'a str, lines: &'a [Line<'static>]) -> Self {
        Self {
            theme,
            title,
            lines,
            scroll: 0,
            footer_hints: &[],
            footer_note: None,
            copy_done: None,
        }
    }

    /// The body rect [`Self::render`] will draw into for `area`.
    ///
    /// Callers that must build their content at the body width before they can
    /// hand it over — anything pre-rendering markdown or mermaid — need the
    /// geometry before the render pass. This is the one definition of it; a
    /// caller that recomputed the rect itself would be a second one that could
    /// disagree after a chrome change.
    pub fn body_area(area: Rect) -> Rect {
        super::popup_inner(centered_popup_area(area))
    }

    pub fn scroll(mut self, scroll: usize) -> Self {
        self.scroll = scroll;
        self
    }

    pub fn footer_hints(mut self, hints: &'a [FooterHint]) -> Self {
        self.footer_hints = hints;
        self
    }

    pub fn footer_note(mut self, note: &'a str) -> Self {
        self.footer_note = Some(note);
        self
    }

    pub fn copy_done(mut self, label: Option<&'a str>) -> Self {
        self.copy_done = label;
        self
    }

    /// Render the popup and return the outer popup area.
    pub fn render(self, frame: &mut Frame, area: Rect) -> Rect {
        let popup_area = centered_popup_area(area);
        let inner = render_popup_chrome(
            frame,
            popup_area,
            self.theme,
            self.title,
            self.footer_note,
            Some(self.footer_hints),
            self.copy_done,
        );

        let content_height = inner.height as usize;
        let total = self.lines.len().max(1);
        let max_scroll = total.saturating_sub(content_height);
        let scroll = self.scroll.min(max_scroll);
        let end = (scroll + content_height).min(total);

        let mut text = Text::default();
        if self.lines.is_empty() {
            text.push_line(Line::from(""));
        } else {
            text.extend(self.lines[scroll..end].iter().cloned());
        }
        let para = Paragraph::new(text).wrap(Wrap { trim: false });
        frame.render_widget(para, inner);

        super::render_popup_scrollbar(frame, popup_area, total, content_height, scroll);

        popup_area
    }
}
