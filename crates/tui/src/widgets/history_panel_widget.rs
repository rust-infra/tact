use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, List, ListItem, Widget};
use crate::state::HistoryEntry;

/// Task history panel widget, showing history entries in reverse chronological order, with Enter to retry.
pub struct HistoryPopupWidget<'a> {
    history: &'a [HistoryEntry],
    /// Entry foreground color.
    accent_color: Color,
    /// Border color.
    border_color: Color,
    /// Panel title (i18n).
    title: &'static str,
}

impl<'a> HistoryPopupWidget<'a> {
    pub fn new(
        history: &'a [HistoryEntry],
        accent_color: Color,
        border_color: Color,
        title: &'static str,
    ) -> Self {
        HistoryPopupWidget {
            history,
            accent_color,
            border_color,
            title,
        }
    }
}

impl Widget for HistoryPopupWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized
    {
        let items: Vec<ListItem> = self
            .history
            .iter()
            .rev()
            .map(|entry| {
                let mut text = format!("[{}] {}", entry.timestamp, entry.task);
                if !entry.summary.is_empty() {
                    text.push_str(&format!(" -> {}", entry.summary));
                }
                ListItem::new(text).style(Style::default().fg(self.accent_color))
            })
            .collect();

        let list = List::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.border_color))
                .title(self.title),
        );
        list.render(area, buf);
    }
}
