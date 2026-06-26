use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    widgets::{Block, BorderType, Borders, List, ListItem, Widget},
};

use crate::theme::Theme;

// Base popup widget, used to display a list of items with a title.
pub struct PopupWidget<'a> {
    list: Vec<ListItem<'a>>,
    title: Option<&'a str>,
    theme: Option<&'a Theme>,
}

impl <'a> Default for PopupWidget<'a> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            title: None,
            theme: None,
        }
    }
}

impl<'a> PopupWidget<'a> {
    pub fn with_list(mut self, list: Vec<ListItem<'a>>) -> Self {
        self.list = list;
        self
    }

    pub fn with_title(mut self, title: &'a str) -> Self {
        self.title = Some(title);
        self
    }

    pub fn with_theme(mut self, theme: &'a Theme) -> Self {
        self.theme = Some(theme);
        self
    }
}

impl<'a> Widget for PopupWidget<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title = self.title.unwrap_or("");
        let border_color = self.theme.map(|theme| theme.border).unwrap_or(Color::White);
        let list = List::new(self.list).block(
            Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(border_color))
                .title(title),
        );
        list.render(area, buf);
    }
}
