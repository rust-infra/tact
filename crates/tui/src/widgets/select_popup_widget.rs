use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Widget};

/// Select popup state: independently manages prompt, options, selected index, and response channel.
pub(crate) struct SelectPopup {
    /// Popup prompt text.
    pub(crate) prompt: String,
    /// Option list.
    pub(crate) options: Vec<String>,
    /// Index of the currently selected option.
    pub(crate) selected: usize,
    /// Response channel for sending the selected option index back to the caller.
    pub(crate) respond: Option<tokio::sync::oneshot::Sender<Option<usize>>>,
}

impl SelectPopup {
    pub(crate) fn new() -> Self {
        Self {
            prompt: String::new(),
            options: Vec::new(),
            selected: 0,
            respond: None,
        }
    }

    /// Set popup content and activate.
    pub(crate) fn set(
        &mut self,
        prompt: String,
        options: Vec<String>,
        respond: tokio::sync::oneshot::Sender<Option<usize>>,
    ) {
        self.prompt = prompt;
        self.options = options;
        self.selected = 0;
        self.respond = Some(respond);
    }

    /// Confirm current selection: send the selected index and clear respond.
    pub(crate) fn confirm(&mut self) -> Option<usize> {
        let respond = self.respond.take();
        let idx = self.selected.min(self.options.len().saturating_sub(1));
        if let Some(tx) = respond {
            let _ = tx.send(Some(idx));
        }
        Some(idx)
    }

    /// Cancel selection: send None and clear respond.
    pub(crate) fn cancel(&mut self) {
        if let Some(tx) = self.respond.take() {
            let _ = tx.send(None);
        }
    }

    /// Move selection down.
    pub(crate) fn move_down(&mut self) {
        if self.selected + 1 < self.options.len() {
            self.selected += 1;
        }
    }

    /// Move selection up.
    pub(crate) fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }
}

/// Selection popup widget: displays prompt and option list centered, supports keyboard/mouse selection.
pub struct SelectPopupWidget<'a> {
    state: &'a SelectPopup,
    /// Highlight background color for selected item.
    highlight_color: Color,
    /// Normal option foreground color.
    fg_color: Color,
    /// Popup background color.
    bg_color: Color,
    /// Hint text when there are no options.
    empty_text: &'static str,
    /// Selected item prefix arrow.
    arrow: &'static str,
}

impl<'a> SelectPopupWidget<'a> {
    pub fn new(
        state: &'a SelectPopup,
        highlight_color: Color,
        fg_color: Color,
        bg_color: Color,
        empty_text: &'static str,
        arrow: &'static str,
    ) -> Self {
        SelectPopupWidget {
            state,
            highlight_color,
            fg_color,
            bg_color,
            empty_text,
            arrow,
        }
    }
}

impl Widget for SelectPopupWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized
    {
        let count = self.state.options.len().max(1) as u16;
        let popup_width = 50u16.min(area.width.saturating_sub(4));
        let popup_height = (count + 4).min(area.height.saturating_sub(4));
        let popup_x = (area.width.saturating_sub(popup_width)) / 2;
        let popup_y = (area.height.saturating_sub(popup_height)) / 2;
        let popup_area = Rect::new(popup_x, popup_y, popup_width, popup_height);

        // Clear existing popup area content
        Clear.render(popup_area, buf);

        // Bordered popup outer frame
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.state.prompt))
            .style(Style::default().bg(self.bg_color));
        block.render(popup_area, buf);

        // Popup inner area
        let inner = Rect::new(
            popup_area.x + 1,
            popup_area.y + 1,
            popup_area.width.saturating_sub(2),
            popup_area.height.saturating_sub(2),
        );

        // Build option list
        let items: Vec<ListItem> = if self.state.options.is_empty() {
            vec![ListItem::new(Span::styled(
                self.empty_text,
                Style::default().fg(Color::Gray),
            ))]
        } else {
            let selected = self
                .state
                .selected
                .min(self.state.options.len().saturating_sub(1));
            self.state
                .options
                .iter()
                .enumerate()
                .map(|(i, opt)| {
                    let is_selected = i == selected;
                    let style = if is_selected {
                        Style::default()
                            .bg(self.highlight_color)
                            .fg(Color::White)
                    } else {
                        Style::default().fg(self.fg_color)
                    };
                    let prefix = if is_selected { self.arrow } else { "  " };
                    ListItem::new(Span::styled(
                        format!("{}{}", prefix, opt),
                        style,
                    ))
                })
                .collect()
        };

        let list = List::new(items).block(Block::default());
        list.render(inner, buf);
    }
}
